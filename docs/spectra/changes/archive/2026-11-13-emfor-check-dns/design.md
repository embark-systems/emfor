## Context

`emfor-check` 是 emfor 內附的外掛程式，以第一個引數選擇 redis、mysql、pgsql 或 ldap。它們都走 TCP：`net.rs` 提供連線、TLS 與 I/O 錯誤分類（`connect: ` 為 down，本機資源不足為 error）；`options.rs` 以 `Kind` 分支驗證一份共用的選項集合（`user`、`password_file`、`role`、TLS 鍵）。

電信等封閉機房要檢查 DNS、RADIUS、SIP、Diameter。前三者主要走 UDP，選項也和資料庫外掛完全不同。本 change 是四個中的第一個：做 DNS，並建立後續共用的 UDP 收發與選項驗證方式。後續 change 依序為 emfor-check-radius、emfor-check-sip、emfor-check-diameter。

## Goals / Non-Goals

**Goals:**

- `emfor-check dns` 對後端送一個查詢，依 RCODE 與（可選的）答案判斷 up / down，UDP 與 TCP 皆可。
- 共用的 UDP 收發函式，RADIUS 與 SIP 直接沿用。
- 網路協定外掛的選項各自驗證，不再擠進資料庫外掛的共用選項集合。
- 新 capability `network-checks` 承接四個協定的需求。

**Non-Goals:**

- DNS over TLS（853）、DNS over HTTPS、DNSSEC 驗證、TSIG。
- 比對答案內容（例如必須回某個 IP），或比對多筆記錄。
- EDNS（查詢不帶 OPT；大的回應在 UDP 上會被截斷，請用 `transport = "tcp"`）。
- 變更 daemon 內建的 `tcp`、`http`、`exec` 檢查，或外掛協定。

## Decisions

### New network-checks capability

`database-checks` 的名稱與內容都是資料庫（LDAP 是例外，因為它和資料庫外掛共用帳密與 TLS 選項）。DNS、RADIUS、SIP、Diameter 的選項、結果分類與傳輸方式都不同，放進新的 `network-checks`。`database-checks` 的「Plugin selection by argument」仍是 `emfor-check` 引數的唯一清單，每個後續 change 都會修改它；「Check options validation」與「Check outcome classification」在本 change 改成只適用四個既有外掛，之後不再動。

替代方案：全部加進 `database-checks`。規格會混在一起，而且「每個外掛都只接受下列鍵」的句子必須每次重寫。

### Use hickory-proto for DNS messages

daemon 已以 `hickory-proto` 0.26.3 編解碼 DNS 訊息，`Cargo.lock` 已有它。emfor-check 以相同版本、只開必要的 feature 引用，不新增套件（以 `cargo tree -p emfor-check -e normal` 確認新增的只有 hickory-proto 本身及它已在 lock 中的依賴）。

替代方案：像 ldap 的 BER 一樣自己寫。DNS 的名稱壓縮與各種 RR 格式比 bind 用到的 BER 多得多，自己寫的風險比引用現成且已在用的 crate 高。

### Shared UDP exchange in net.rs

`net.rs` 新增一個 UDP 交換函式：綁定與後端同家族的未指定位址、`connect` 到後端（只收後端送來的封包，並讓 ICMP port unreachable 以 `ConnectionRefused` 回報），送出請求後讀取回應，交給呼叫者提供的比對函式判斷是否為這次請求的回應；不相符的封包丟掉繼續等。到期限一半仍無相符回應時重送一次同一封包。每個封包最多讀 65535 位元組。

替代方案：不重送。UDP 掉一個封包就記一次 down；`fall` 能吸收，但在 interval 長的服務上會拖慢恢復，且 RADIUS、SIP 的慣例都是重送。只重送一次是因為 `timeout_ms` 通常不到一秒。

### Per-plugin option parsing for network plugins

`lib.rs` 的 `validate` 與 `check` 依 `Kind` 分流：四個資料庫外掛仍走 `options::parse` / `options::settings`；網路外掛由各自模組（本 change 為 `dns.rs`）解析成自己的設定型別。`options.rs` 裡的小工具（布林、非空字串、絕對路徑檔案檢查）改為 `pub(crate)` 讓各模組共用，訊息文字不變。

替代方案：把 DNS 的鍵加進 `options::Settings`。所有外掛的設定都會帶一堆不相干的欄位，驗證規則也要以 `Kind` 交錯判斷。

### No DNS over TLS in this change

DoT 用 emfor-tls 很容易接上，但封閉機房內的 DNS 幾乎都是明文 53 埠，先不做；`tls` 等鍵在 dns 外掛是 `unknown key`，之後要加時是新增鍵，不破壞相容性。

## Implementation Contract

**Behavior**

- `emfor-check dns` 握手回 `{"type":"hello","protocol":1,"name":"dns","version":"<套件版本>"}`。
- 每次 check：產生隨機 16 位元 ID，送出一個查詢（opcode QUERY、RD = `recursion_desired`、一個問題 `name` / `type` / IN、無 EDNS）。
  - UDP：經共用 UDP 交換送出；回應須 QR = 1、ID 相同、問題區恰有一題且名稱（不分大小寫、忽略結尾點）、類型、類別都相同，否則忽略繼續等。
  - TCP：連線（`connect: ` 失敗為 down）、送出 2 位元組長度前綴加訊息、讀取一則回應（長度前綴，最多 65535 位元組）；ID 或問題不符為 down，原因 `query: response does not match the query`。
- 判斷順序：無法解析為 `query: <說明>`；RCODE 不在 `expect_rcode` 為 `rcode: <名稱>`（名稱取自 RFC 1035、2136、8490 的 0–11；其餘寫數字，如 `rcode: 23`）；`expect_answer = true` 且答案區沒有 `type` 類型的記錄為 `answer: no <TYPE> records`，若回應的 TC 為 1 再加上 `` (truncated; set transport = "tcp")``；其餘為 up。
- 期限內沒有相符回應為 down，原因 `timeout`。UDP 收到 ICMP port unreachable 為 down，原因 `connect: ` 加錯誤訊息。本機無法建立 socket（檔案描述元用盡等）為 error。

**Options（`check_options`）**

| 鍵 | 型別 | 預設 | 規則 |
| --- | --- | --- | --- |
| `name` | 字串 | 必填 | 網域名稱：最多 253 字元（不含結尾點），label 1–63 字元，由字母、數字、`-`、`_` 組成；可帶一個結尾點；`.` 為根 |
| `type` | 字串 | `A` | `A`、`AAAA`、`CNAME`、`MX`、`NS`、`PTR`、`SOA`、`SRV`、`TXT` 之一，不分大小寫 |
| `transport` | 字串 | `udp` | `udp` 或 `tcp` |
| `recursion_desired` | 布林 | `true` | |
| `expect_rcode` | 字串陣列 | `["NOERROR"]` | 非空；每個值為 `NOERROR`、`FORMERR`、`SERVFAIL`、`NXDOMAIN`、`NOTIMP`、`REFUSED` 之一，不分大小寫 |
| `expect_answer` | 布林 | `false` | |

其他鍵（含 `tls`、`user`、`password_file`）為 `unknown key`。錯誤訊息：`missing required key`、`must be a string`、`must be a boolean`、`not a valid domain name`、`must be one of A, AAAA, CNAME, MX, NS, PTR, SOA, SRV, TXT`、`must be udp or tcp`、`must be a non-empty array`、`must be one of NOERROR, FORMERR, SERVFAIL, NXDOMAIN, NOTIMP, REFUSED`。

**Acceptance**

- 單元測試以本機假 UDP / TCP 伺服器涵蓋 spec 的每個 Scenario 與 Example。
- `crates/emfor-check/tests/cli.rs` 的 dns 握手測試。
- `tests/plugins/run.sh` 對 unbound 跑實機測試：A 查詢 up、NXDOMAIN down、`expect_rcode = ["NOERROR","NXDOMAIN"]` up、TCP up、`expect_answer` 在沒有記錄時 down、停掉的埠為 `connect: ` 或 `timeout`。
- `cargo tree -p emfor-check -e normal` 只多出 hickory-proto 及其已在 `Cargo.lock` 的依賴；`Cargo.lock` 不新增套件。

**Scope**

- 在範圍內：dns 外掛、共用 UDP 交換、選項分流、`database-checks` 三個需求的修改、`network-checks` 新規格、文件與實機測試。
- 範圍外：RADIUS、SIP、Diameter（各自的 change）、DoT/DoH、DNSSEC、daemon 端任何變更。

## Risks / Trade-offs

- [unbound 等遞迴伺服器對未快取的名稱要往外查，在封閉機房可能 SERVFAIL] → 文件建議檢查伺服器自己負責的名稱（local-zone 或權威區），或把 SERVFAIL 以外的狀況接受為 up。
- [hickory-proto 的 feature 組合與 daemon 不同，可能拉進新的 crate] → 只開 daemon 已開的 feature；實作時以 `cargo tree` 比對，若會新增套件則改用 `default-features = false` 並在完成紀錄說明。
- [UDP 重送讓不支援重複查詢的伺服器多收一個查詢] → DNS 查詢是冪等的，影響可忽略。
- [`expect_answer` 只看類型、不看內容] → 內容比對列為 Non-Goal；需要時以新鍵擴充。
