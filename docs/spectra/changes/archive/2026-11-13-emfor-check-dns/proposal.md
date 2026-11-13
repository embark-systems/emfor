## Why

封閉機房（例如電信機房）裡的後端常是 DNS、RADIUS、SIP、Diameter 這類以 UDP 或二進位協定溝通的服務，emfor 目前只能檢查 TCP、HTTP、exec 與 emfor-check 的資料庫外掛，沒有 UDP 檢查；用 `exec` 跑 `dig` 每次都要起程序，也不能區分「伺服器回了 SERVFAIL」與「伺服器沒回」。DNS 是四個協定裡最單純、也最常見的一個，先做它，同時建立 RADIUS 與 SIP 之後要共用的 UDP 收發程式碼與新的 `network-checks` 規格。

## What Changes

- `emfor-check` 新增外掛引數 `dns`：`command = ["/usr/local/libexec/emfor-check", "dns"]`。
- 每次檢查送出一個 DNS 查詢：`name`（必填）、`type`（預設 `A`）、`transport`（`udp` 預設或 `tcp`）、`recursion_desired`（預設 `true`）。回應的 RCODE 在 `expect_rcode`（預設 `["NOERROR"]`）之內為 up；`expect_answer = true` 時答案區還必須有該類型的記錄。
- 原因：`rcode: SERVFAIL` 之類（RCODE 名稱）、`answer: no A records`（UDP 回應被截斷時提示改用 `transport = "tcp"`）、格式錯誤為 `query: `，其餘沿用 `connect: `、`timeout`。
- 新增共用的 UDP 收發：對後端建立已連線的 UDP socket，只接受 ID 與問題都相符的回應，逾時一半時重送一次同一封包，ICMP port unreachable 為 down（`connect: Connection refused`）。之後的 `emfor-check-radius` 與 `emfor-check-sip` 沿用。
- 新增 capability `network-checks`，放 DNS、RADIUS、SIP、Diameter 這類網路協定外掛的需求；`database-checks` 的「Check options validation」與「Check outcome classification」改為只約束 redis、mysql、pgsql、ldap 四個外掛。
- DNS 訊息用 `hickory-proto` 編解碼（daemon 已使用，Cargo.lock 已有 0.26.3），不新增套件。
- 實機測試：`emfor-plugins-test` 映像加入 unbound 作為測試伺服器；重建映像前先向使用者確認。
- 文件：`docs/deploy/plugins.md` 加入 dns 一節；`examples/emfor.toml` 加入註解掉的範例；`docs/spectra/config.yaml` 記錄 dns 外掛與後續三個 change 的順序。

## Non-Goals

詳見 design.md 的 Goals / Non-Goals：不做 DNS over TLS / HTTPS、DNSSEC 驗證、比對答案內容（特定 IP）、EDNS 與 TSIG。

## Capabilities

### New Capabilities

- `network-checks`：emfor-check 的網路協定外掛（本 change 為 dns），以及它們共用的 UDP 收發與結果分類規則。

### Modified Capabilities

- `database-checks`：「Plugin selection by argument」加入 `dns`；「Check options validation」與「Check outcome classification」改為只適用 redis、mysql、pgsql、ldap。

## Impact

- 程式：crates/emfor-check/src/lib.rs（`Kind` 加入 dns、各外掛自行驗證選項）、crates/emfor-check/src/net.rs（UDP 收發）、新增 crates/emfor-check/src/dns.rs、crates/emfor-check/Cargo.toml（`hickory-proto` 依賴，版本與 daemon 相同）。
- 測試：crates/emfor-check/tests/cli.rs、crates/emfor-check/tests/live.rs、tests/plugins/Dockerfile、tests/plugins/inside.sh。
- 文件：docs/deploy/plugins.md、examples/emfor.toml、docs/spectra/config.yaml。
- 不影響 daemon、LMDB、client、DNS 伺服器與外掛協定。
- 後續 change（依序實作）：emfor-check-radius、emfor-check-sip（兩者使用本 change 的 UDP 收發）、emfor-check-diameter。
