## Context

emfor-check-dns 建立了 `network-checks` 規格、`net.rs` 的 UDP 收發（connect 到後端、比對函式篩選回應、逾時一半重送一次）與各外掛自行驗證選項的分流方式。本 change 在其上加 RADIUS。

RADIUS（RFC 2865 認證、RFC 2866 計費）走 UDP 1812 / 1813。每個請求帶 16 位元組的 Request Authenticator；回應的 Response Authenticator 是 `MD5(Code + Identifier + Length + Request Authenticator + 屬性 + 共享密鑰)`，可以確認回應來自知道密鑰的伺服器。RFC 5997 的 Status-Server（Code 12）是專為健康檢查設計的請求：伺服器以 Access-Accept（認證埠）或 Accounting-Response（計費埠）回覆，不會碰使用者資料庫，也不會寫計費記錄。2024 年的 BlastRADIUS 漏洞之後，建議所有請求與回應都帶 Message-Authenticator（RFC 3579，HMAC-MD5）且放在第一個屬性。

## Goals / Non-Goals

**Goals:**

- 以 Status-Server 檢查認證與計費服務，以及可選的 Access-Request（PAP）端到端認證檢查。
- 驗證每個回應的 Response Authenticator 與（有帶時的）Message-Authenticator，密鑰錯誤時給出明確原因。
- 密鑰與密碼只從檔案讀取，每次檢查重讀，不出現在任何原因或 log。

**Non-Goals:**

- RadSec（RFC 6614 TLS、RFC 6613 TCP）、DTLS。
- CHAP、MS-CHAP、EAP；Accounting-Request（會寫計費記錄）。
- CoA / Disconnect（RFC 5176）。
- 比對回應的屬性內容（例如必須回某個 Framed-IP-Address）。
- Proxy 情境下的 Proxy-State 處理。

## Decisions

### Status-Server as the default request

Status-Server 不需要測試帳號、不寫計費記錄、不觸發認證失敗告警，是伺服器專為監控提供的請求，所以是預設值。FreeRADIUS 需要在 `radiusd.conf` 的 `security` 區段開 `status_server = yes`，文件會寫。

替代方案：預設 Access-Request。需要每台伺服器都建測試帳號，失敗還會出現在認證 log 與告警裡。

### Access-Request with PAP for end-to-end checks

`request = "access-request"` 送 User-Name 與以 RFC 2865 5.2 方式隱藏的 User-Password，Access-Accept 為 up。這能確認伺服器背後的使用者資料庫（LDAP、SQL）也正常。`reject_is_up = true` 讓 Access-Reject 也算 up，給只想確認伺服器有在處理認證、但不想維護有效測試帳號的情境。Access-Challenge 一律為 down，因為 PAP 不該被要求挑戰。

替代方案：支援 CHAP 或 EAP。複雜度高、對健康檢查沒有額外價值。

### Always verify the response authenticator

每個與請求 Identifier 相同的回應都必須通過 Response Authenticator 驗證，否則為 down，原因 `auth: response authenticator does not match (wrong secret?)`。回應帶 Message-Authenticator 時一併驗證（以 Request Authenticator 代入計算）。`require_message_authenticator`（預設 `false`）為 `true` 時，沒帶 Message-Authenticator 的回應為 down。

預設不強制，是因為 2024 年以前的伺服器多數不在回應中帶它；強制會讓這些伺服器全部判為 down。

### Message-Authenticator first in every request

每個請求的第一個屬性都是 Message-Authenticator，符合 BlastRADIUS 之後的建議，也讓要求它的伺服器（FreeRADIUS 3.2.5 以後的 `require_message_authenticator = yes`）接受。

### Secrets from secret_file only

共享密鑰只能用 `secret_file`，規則與 `password_file` 相同：絕對路徑、去掉一個結尾換行後非空、每次檢查重讀；讀不到為 error，原因 `secret_file: <I/O 錯誤種類>`。`secret`、`password` 都是 `unknown key`。

### MD5 and HMAC from crates already in Cargo.lock

`md-5` 0.11（client 的 sticky hashing 已在用）與 `hmac` 0.13 都在 `Cargo.lock`。實作時以 `cargo tree -p emfor-check -e normal` 確認 `Cargo.lock` 沒有新增套件；若版本組合需要新套件，改為在 `radius.rs` 內以 `md-5` 自行實作 HMAC（RFC 2104，約 20 行）並在完成紀錄說明。

## Implementation Contract

**Behavior**

- `emfor-check radius` 握手回 `name` `radius`。
- 每次 check 讀取 `secret_file`（與 access-request 時的 `password_file`），讀不到為 error。
- 請求：隨機 Identifier（0–255）與 16 位元組隨機 Request Authenticator；屬性依序為 Message-Authenticator（80，HMAC-MD5，金鑰為共享密鑰，計算時本屬性值為 16 個 0）、`request = "access-request"` 時的 User-Name（1）與 User-Password（2）、NAS-Identifier（32，`nas_identifier`）。Code：status-server 為 12，access-request 為 1。
- 經 UDP 收發送出；比對規則為「長度至少 20、Identifier 相同」。相符的回應依序判斷：
  1. 長度欄位與封包長度不符、或屬性長度不合法 → down `query: malformed response`。
  2. Response Authenticator 不符 → down `auth: response authenticator does not match (wrong secret?)`。
  3. 帶 Message-Authenticator 但不符 → down `auth: Message-Authenticator does not match`；沒帶且 `require_message_authenticator = true` → down `auth: response has no Message-Authenticator`。
  4. 依回應碼：`application = "auth"` 時 Access-Accept（2）為 up；Access-Reject（3）在 `reject_is_up = true` 時為 up，否則 down `auth: Access-Reject`；Access-Challenge（11）為 down `auth: Access-Challenge`。`application = "acct"` 時 Accounting-Response（5）為 up。其他回應碼為 down `query: unexpected response code <N>`。`auth: ` 原因在回應帶 Reply-Message（18）時加上 `: <內容>`（多個時以空格串接，非 UTF-8 位元組以 U+FFFD 取代）。
- 逾時、ICMP 拒絕依 `network-checks` 的「UDP request and response exchange」。

**Options（`check_options`）**

| 鍵 | 型別 | 預設 | 規則 |
| --- | --- | --- | --- |
| `secret_file` | 絕對路徑 | 必填 | 可讀、去掉一個結尾換行後非空 |
| `request` | 字串 | `status-server` | `status-server` 或 `access-request` |
| `application` | 字串 | `auth` | `auth` 或 `acct`；`acct` 需要 `request = "status-server"` |
| `user` | 字串 | 無 | 1–253 位元組；`access-request` 必填，`status-server` 時為錯誤 |
| `password_file` | 絕對路徑 | 無 | 同 `password_file` 既有規則，且密碼最多 128 位元組（RFC 2865 5.2）；`access-request` 必填，`status-server` 時為錯誤 |
| `reject_is_up` | 布林 | `false` | 需要 `request = "access-request"` |
| `nas_identifier` | 字串 | `emfor` | 1–253 位元組 |
| `require_message_authenticator` | 布林 | `false` | |

錯誤訊息：`missing required key`、`unknown key`、`must be a string`、`must be a boolean`、`not an absolute path`、`file not found`、`file is empty`、`must be status-server or access-request`、`must be auth or acct`、`requires request = "access-request"`、`requires request = "status-server"`、`must be 1 to 253 bytes`、`password longer than 128 bytes`。check 時密碼檔被改成超過 128 位元組仍為 error（`password_file: password longer than 128 bytes`），不送出請求。

**Acceptance**

- 單元測試以本機假 RADIUS 伺服器（以已知密鑰簽回應）涵蓋 spec 每個 Scenario 與 Example；以 RFC 2865 第 7.1 節的範例封包驗證 User-Password 隱藏與 Response Authenticator 計算。
- `tests/plugins/run.sh` 對 FreeRADIUS 跑實機測試：Status-Server auth 與 acct 為 up、錯誤密鑰為 `timeout`（FreeRADIUS 驗證請求的 Message-Authenticator 失敗後直接丟棄、不回應；`auth: response authenticator does not match (wrong secret?)` 由單元測試涵蓋）、Access-Request 正確帳密 up、錯誤密碼 `auth: Access-Reject`、`reject_is_up` 時錯誤密碼 up、`require_message_authenticator = true` 對 FreeRADIUS 3.2.1 為 down `auth: response has no Message-Authenticator`（該版本的回應不帶 Message-Authenticator，3.2.5 起才帶）、停掉的埠為 `connect: ` 或 `timeout`；原因與 stderr 都不含密鑰與密碼。

**Scope**

- 在範圍內：radius 外掛、`network-checks` 的兩個新需求、`database-checks` 的引數清單、文件與實機測試。
- 範圍外：上方 Non-Goals 列出的協定功能；daemon 端任何變更。

## Risks / Trade-offs

- [伺服器沒開 Status-Server 時不回應，只會看到 `timeout`，難以和網路問題區分] → 文件明列 FreeRADIUS 的設定；原因維持 `timeout`，不臆測。
- [每台 emfor 主機都要加進 RADIUS 伺服器的 clients 清單] → 文件說明可用網段方式設定 client。
- [Access-Request 每次檢查都在伺服器留下認證記錄] → 預設用 Status-Server；文件提醒 access-request 模式要調長 `interval`。
- [會驗證請求 Message-Authenticator 的伺服器（如 FreeRADIUS）在密鑰錯誤時直接丟棄請求，結果是 `timeout` 而不是 `auth: response authenticator does not match`] → 實機測試（2026-11-13，FreeRADIUS 3.2.1）證實；文件說明 `timeout` 也可能是密鑰錯誤，請先比對伺服器 log。
- [較舊的伺服器（FreeRADIUS 3.2.5 以前）回應不帶 Message-Authenticator，`require_message_authenticator = true` 會讓它們全部為 down] → 預設維持 `false`；文件說明只對會帶的伺服器開啟。
- [伺服器以 Identifier 去重，重送的封包可能被當成重複而只回一次] → 這正是預期行為，比對以 Identifier 為準，收到任一回應即可。
