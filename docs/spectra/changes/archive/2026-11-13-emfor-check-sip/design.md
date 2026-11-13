## Context

emfor-check-dns 建立了 `network-checks` 規格與 `net.rs` 的 UDP 收發；`net.rs` 也已有 TCP 連線與 emfor-tls 的 TLS 握手（ldap 的 LDAPS 在用）。SIP（RFC 3261）是文字協定，格式類似 HTTP；OPTIONS 請求向伺服器詢問能力，伺服器不需要建立任何狀態就能回應，是 SIP 世界的健康檢查慣例。

## Goals / Non-Goals

**Goals:**

- 以 OPTIONS 檢查 SIP 伺服器，UDP、TCP、TLS 三種傳輸。
- 以狀態碼判斷 up / down，預設把伺服器「有在處理請求」的回應都算 up，把 5xx、6xx（過載、維護中、拒絕服務）算 down。
- 不新增套件。

**Non-Goals:**

- REGISTER、INVITE 等其他方法；Digest 認證（401 / 407 在預設的 `expect_status` 內即為 up，不回應挑戰）。
- SCTP、WebSocket 傳輸。
- 比對回應標頭（Allow、Supported）或內容。
- 追蹤重新導向（3xx 只依 `expect_status` 判斷）。

## Decisions

### Default expect_status 200-499

很多 SIP proxy 對發給自己的 OPTIONS 回 404、405、483 或 401 / 407（要求認證），伺服器其實正常運作；Kamailio dispatcher 等工具的慣例也是只把 5xx / 6xx 或無回應視為失敗，而 503 正是 SIP 伺服器表示過載或維護中的方式。所以預設 `["200-499"]`。只想接受 200 的使用者可設 `expect_status = ["200"]`。

替代方案：預設 `["200-299"]`。會讓大量正常的 proxy 被判為 down，使用者第一次設定就碰壁。

### Hand-written SIP message handling

只需要組一個固定格式的 OPTIONS 請求，以及解析回應的狀態列與少數標頭（Via、Call-ID、CSeq、Content-Length），自己寫比引入完整的 SIP 堆疊簡單得多，也不新增套件。標頭名稱不分大小寫，並接受 RFC 3261 的縮寫（`v` 為 Via、`i` 為 Call-ID、`l` 為 Content-Length）。

### Match responses by Via branch, Call-ID and CSeq

回應的第一個 Via 標頭的 `branch` 參數、Call-ID、CSeq（`1 OPTIONS`）都要與請求相同才算這次請求的回應。UDP 上不相符的封包丟掉繼續等（沿用 UDP 收發的規則）；TCP 上收到不相符的回應為 down `query: response does not match the request`。1xx 暫時回應在兩種傳輸上都忽略，繼續等最終回應。

### TLS through the existing net.rs helpers

`tls = true` 時以 `net.rs` 的 TCP 連線加 emfor-tls 握手，選項驗證與訊息和其他外掛相同（`requires tls = true`）；`tls = true` 需要 `transport = "tcp"`，否則為 `transport` 的錯誤 `must be tcp when tls = true`（`transport` 不是 udp 或 tcp 時兩個錯誤都列出）。TCP 上狀態列前的空行（CRLF keepalive）略過（RFC 3261 7.5）。Via 的傳輸欄位依序為 `UDP`、`TCP`、`TLS`。

## Implementation Contract

**Behavior**

- `emfor-check sip` 握手回 `name` `sip`。
- 每次 check 產生隨機的 branch（以 `z9hG4bK` 開頭）、From tag 與 Call-ID，送出：

  ```
  OPTIONS <uri> SIP/2.0
  Via: SIP/2.0/<UDP|TCP|TLS> <本機 IP>:<本機埠>;branch=<branch>;rport
  Max-Forwards: 70
  From: <from>;tag=<tag>
  To: <uri>
  Call-ID: <call-id>
  CSeq: 1 OPTIONS
  User-Agent: emfor-check/<版本>
  Accept: application/sdp
  Content-Length: 0
  ```

  上面的 `<uri>`、`<from>` 等是代入值；實際送出時 From 與 To 以角括號包住 URI（name-addr 形式），例如 `From: <sip:emfor@emfor.invalid>;tag=t1`、`To: <sip:10.0.9.11:5060>`。行尾一律 CRLF，IPv6 位址在 Via 與預設 URI 中以 `[...]` 表示。`<uri>` 預設為 `sip:<後端 IP>:<後端埠>`（`tls = true` 時為 `sips:`）。
- UDP：經共用的 UDP 收發；TCP / TLS：連線後寫入請求，讀取回應直到標頭結束（空行）並依 Content-Length 讀完內容，單一回應最多 65536 位元組，連線上可依序收到多個回應（先 1xx 再最終回應）。
- 判斷：狀態列不是 `SIP/2.0 <三位數> <原因片語>`（原因片語可以是空的，RFC 3261 25.1；此時原因為 `status: <碼>`）、缺少 Via / Call-ID / CSeq、或超過 65536 位元組 → down，原因以 `query: ` 開頭；狀態碼 100–199 → 忽略；最終回應的狀態碼在 `expect_status` 內 → up，否則 down `status: <碼> <原因片語>`。
- 逾時、ICMP 拒絕依 `network-checks` 的「UDP request and response exchange」；TCP 連線失敗為 `connect: `，TLS 失敗為 `tls: `。

**Options（`check_options`）**

| 鍵 | 型別 | 預設 | 規則 |
| --- | --- | --- | --- |
| `transport` | 字串 | `udp` | `udp` 或 `tcp` |
| `tls` | 布林 | `false` | 需要 `transport = "tcp"` |
| `tls_verify`、`server_name`、`ca_file` | 同其他外掛 | 同其他外掛 | 需要 `tls = true` |
| `uri` | 字串 | `sip:<後端>` | 以 `sip:` 或 `sips:` 開頭、不含空白與控制字元、最多 256 位元組 |
| `from` | 字串 | `sip:emfor@emfor.invalid` | 規則同 `uri` |
| `expect_status` | 字串陣列 | `["200-499"]` | 非空；每項為 200–699 的狀態碼或 `lo-hi` 範圍（lo ≤ hi） |

錯誤訊息：`unknown key`、`must be a string`、`must be a boolean`、`must be udp or tcp`、`must be tcp when tls = true`、`requires tls = true`、`must be a sip: or sips: URI`、`must be a non-empty array`、`must be a status code from 200 to 699 or a range lo-hi`。

**Acceptance**

- 單元測試以本機假 SIP 伺服器（UDP 與 TCP）涵蓋 spec 每個 Scenario 與 Example；請求文字以固定的 branch、tag、Call-ID 比對逐位元組內容。
- `tests/plugins/run.sh` 對 Kamailio 跑實機測試：UDP 與 TCP 的 OPTIONS 為 up、TLS 為 up、TLS 憑證名稱不符為 `tls: `、`expect_status = ["200"]` 而 Kamailio 回非 200 時的原因、停掉的 UDP 埠為 `connect: ` 或 `timeout`。

**Scope**

- 在範圍內：sip 外掛、`network-checks` 的兩個新需求、`database-checks` 的引數清單、文件與實機測試。
- 範圍外：Non-Goals 列出的功能；daemon 端任何變更。

## Risks / Trade-offs

- [伺服器以 IP 白名單或網域限制 OPTIONS，回 403] → 403 在預設 `expect_status` 內為 up；文件說明要嚴格判斷時設 `expect_status = ["200"]` 並在伺服器開白名單。
- [NAT 或多網卡時 Via 的本機位址不是伺服器看到的位址] → Via 帶 `rport`，伺服器會回到實際來源；比對不依賴 Via 的主機欄位。
- [SIP 伺服器把 OPTIONS 記入 log，頻繁檢查造成大量 log] → 文件提醒調整 `interval` 或伺服器的 log 設定。
- [回應超過 UDP 能承載的大小] → OPTIONS 回應通常數百位元組；超過時伺服器應改用 TCP，屬伺服器端設定，原因會是 `timeout`。
