## Context

Diameter（RFC 6733）在 TCP（3868）、TLS（5658）或 SCTP 上傳送二進位訊息：20 位元組標頭（版本、長度、旗標 R/P/E/T、Command Code、Application-Id、Hop-by-Hop 與 End-to-End 識別碼）加上一串 AVP（代碼、旗標 V/M/P、長度、可選的 Vendor-Id、資料並補齊到 4 位元組）。兩個節點連上後先交換能力（CER / CEA），之後以 Device-Watchdog（DWR / DWA）確認對方存活，結束時送 Disconnect-Peer（DPR / DPA）。

emfor-check 已有 TCP 連線、emfor-tls 握手與 I/O 錯誤分類（`net.rs`）；emfor-check-dns 建立了 `network-checks` 規格與各外掛自行驗證選項的方式。本 change 不需要 UDP 收發。

## Goals / Non-Goals

**Goals:**

- 以 CER/CEA 加 DWR/DWA 確認 Diameter 節點會接受 emfor 這個 peer，而且有在處理訊息。
- 失敗原因帶 Result-Code 與名稱，讓維運人員看得出是「不認得 peer」、「沒有共同 application」還是「過載」。
- 支援 3GPP 等以 Vendor-Specific-Application-Id 宣告的 application。

**Non-Goals:**

- SCTP（多數 Diameter 實作同時聽 TCP；Rust 端的 SCTP 需要額外套件與核心模組）、DTLS。
- application 層的請求（S6a AIR、Gx CCR、Gy CCR 等）與其內容判斷。
- 長連線與 RFC 6733 的 peer 狀態機（每次檢查開新連線、用完即關）。
- 自動從 CEA 學習對方的 application 或 realm。

## Decisions

### CER then DWR on a fresh connection

每次檢查開新連線、送 CER；CEA 成功後送 DWR，DWA 成功才算 up。只看 CEA 雖然已能證明對方接受 peer，但 DWR 是 Diameter 本身用來判斷 peer 存活的訊息，多一個來回能確認訊息處理迴圈真的在運作（有些實作在過載時仍會完成能力交換）。

替代方案：維持長連線、只定期送 DWR，像真正的 Diameter peer 一樣。要實作 peer 狀態機與重連邏輯，且每台 emfor 主機都會在對端佔一個常駐 peer，複雜度與副作用都大得多。

### Disconnect with DO_NOT_WANT_TO_TALK_TO_YOU

up 或 down 確定後，若 CER/CEA 已成功，送 DPR，Disconnect-Cause 為 2（`DO_NOT_WANT_TO_TALK_TO_YOU`，RFC 6733 對「預期近期不會再交換訊息」的定義），最多等 200 ms 或剩餘時間（較短者）讀 DPA，然後關閉連線。DPA 的內容與是否收到都不影響結果：結果先回給 daemon，DPR、等 DPA 與關閉在背景進行，避免等 DPA 拖過外掛執行框架自己的逾時，把已判定的結果變成 `timeout`。直接關閉 TCP 會讓部分實作記錄異常斷線告警。

### Result codes 2xxx are success

CEA 與 DWA 的 Result-Code 在 2000–2999 為成功（`DIAMETER_SUCCESS` 2001、`DIAMETER_LIMITED_SUCCESS` 2002）。其他值為 down，原因 `cea: <碼> <名稱>` 或 `dwa: <碼> <名稱>`；名稱取 RFC 6733 第 7.1 節定義的名稱（含 `DIAMETER_` 前綴），未定義的只寫數字。標頭帶 E 旗標的錯誤回應也以它的 Result-Code 判斷。

### Hand-written Diameter codec

只需要編碼 CER、DWR、DPR，解碼 CEA、DWA、DPA 的標頭與少數 AVP（Result-Code、Error-Message），格式固定，自己寫比引入完整的 Diameter 堆疊簡單，也不新增套件。單一訊息最多讀 65536 位元組，超過為 down `query: message longer than 65536 bytes`。

### Client certificate for TLS

RFC 6733 要求 Diameter 節點雙向認證，實作（例如 freeDiameter）在 TLS 時要求對方出示客戶端憑證，並檢查憑證名稱與 Origin-Host 相符。實機測試確認：沒有客戶端憑證時 freeDiameter 在握手後關閉連線，emfor 只看得到 `cea: connection closed by peer`。因此 diameter 外掛新增 `tls_cert_file`、`tls_key_file`（PEM 憑證鏈與私鑰，兩者一起設定，需要 `tls = true`），伺服器要求時出示。

兩個檔案和 `ca_file` 一樣每次檢查都重新讀取，換憑證不需要 reload；讀不到或內容不對是本機的問題，結果為 `error`（`tls_cert_file: <原因>` 或 `tls_key_file: <原因>`），不開連線。emfor-tls 新增 `ClientCertificate` 與帶客戶端憑證的 `client_config_with_client` 與 `handshake_with_config`（先組好設定、連線後握手），原本的 `handshake` / `client_config` 改為以「沒有客戶端憑證」呼叫它們；`TlsSettings` 不變，所以 daemon 的 `tcp` 檢查與其他外掛不受影響。

替代方案：把客戶端憑證放進 `TlsSettings`，讓所有外掛都能用。要修改 daemon 與每個外掛的建構處，且目前只有 Diameter 需要；之後其他外掛需要時再搬。

替代方案：不支援客戶端憑證，文件寫明限制。使用者選擇支援，因為依 RFC 6733 的 TLS 節點多半要求雙向認證，不支援等於 TLS 選項無法使用。

### Applications declared by the operator

CER 宣告的 application 由使用者設定，emfor 不猜：`auth_application_ids`、`acct_application_ids`（整數陣列）與 `vendor_specific_application_ids`（`{ vendor_id, auth_application_id }` 或 `{ vendor_id, acct_application_id }` 表格陣列），三者至少要有一個非空。對端若沒有共同 application，回 `DIAMETER_NO_COMMON_APPLICATION`，原因會直接說明。

替代方案：預設宣告 Relay（0xFFFFFFFF）。非 relay 節點多半會拒絕，使用者看到 5010 卻不知道要設什麼。

## Implementation Contract

**Behavior**

- `emfor-check diameter` 握手回 `name` `diameter`。
- 每次 check：TCP 連線到後端（失敗為 down `connect: `）；`tls = true` 時立即做 TLS 握手（失敗為 down `tls: `）。
- CER（Command Code 257、R 旗標、Application-Id 0、隨機 Hop-by-Hop 與 End-to-End）的 AVP 依序為：Origin-Host（264，M）、Origin-Realm（296，M）、Host-IP-Address（257，M；`host_ip_address`，未設時為本機連線位址）、Vendor-Id（266，M，值 0）、Product-Name（269，值 `emfor`）、每個 `supported_vendor_ids` 一個 Supported-Vendor-Id（265，M）、每個 `auth_application_ids` 一個 Auth-Application-Id（258，M）、每個 `acct_application_ids` 一個 Acct-Application-Id（259，M）、每個 `vendor_specific_application_ids` 一個 Vendor-Specific-Application-Id（260，M，內含 Vendor-Id 與 Auth- 或 Acct-Application-Id）。
- 讀取回應：Command Code 與 Hop-by-Hop 相同、R 旗標為 0 的訊息才是回應；對端送來的請求（R 旗標為 1，例如它自己的 DWR）忽略不回。CEA 前連線被對端關閉為 down `cea: connection closed by peer`；DWA 前關閉為 `dwa: connection closed by peer`。
- CEA 的 Result-Code 不是 2xxx → down `cea: <碼> <名稱>`；沒有 Result-Code → down `query: answer without Result-Code`。成功後送 DWR（280，R 旗標，Origin-Host、Origin-Realm）；DWA 依同樣規則產生 `dwa: ` 原因或 up。
- CER/CEA 成功後一律依「Disconnect with DO_NOT_WANT_TO_TALK_TO_YOU」送 DPR（282）再關閉。
- 標頭版本不是 1、長度欄位小於 20 或與 AVP 不符、AVP 長度不合法 → down，原因以 `query: ` 開頭。
- 整個檢查受 `timeout_ms` 限制，逾時為 `timeout`。
- 有 `tls_cert_file` 與 `tls_key_file` 時，連線前讀取兩個檔案；讀不到、沒有憑證或沒有私鑰為 error `tls_cert_file: ` / `tls_key_file: `，不開連線。TLS 握手時伺服器要求客戶端憑證就出示它。

**Options（`check_options`）**

| 鍵 | 型別 | 預設 | 規則 |
| --- | --- | --- | --- |
| `origin_host` | 字串 | 必填 | DiameterIdentity：1–255 字元，由 ASCII 字母、數字、`-`、`.` 組成，不以 `.` 開頭或結尾 |
| `origin_realm` | 字串 | 必填 | 同上 |
| `auth_application_ids` | 整數陣列 | `[]` | 每項 0–4294967295 |
| `acct_application_ids` | 整數陣列 | `[]` | 每項 0–4294967295 |
| `vendor_specific_application_ids` | 表格陣列 | `[]` | 每項恰有 `vendor_id` 與 `auth_application_id` 或 `acct_application_id` 其中之一，值 0–4294967295 |
| `supported_vendor_ids` | 整數陣列 | `[]` | 每項 0–4294967295 |
| `host_ip_address` | 字串 | 本機連線位址 | IPv4 或 IPv6 位址 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | 同其他外掛 | 同其他外掛 | 同其他外掛 |
| `tls_cert_file`、`tls_key_file` | 絕對路徑 | 無 | PEM 客戶端憑證鏈與私鑰；兩者一起設定，需要 `tls = true`；驗證時讀檔確認內容 |

三個 application 陣列都空時，錯誤在 `auth_application_ids`：`at least one application is required`。其他錯誤訊息：`missing required key`、`unknown key`、`must be a string`、`must be a boolean`、`not a valid DiameterIdentity`、`must be an array of integers from 0 to 4294967295`、`each entry needs vendor_id and one of auth_application_id, acct_application_id`、`not an IP address`、`requires tls = true`、`requires tls_key_file`、`requires tls_cert_file`、`not an absolute path`、`file not found`、`cannot read: <kind>`、`no certificate found`、`no private key found`。

**Acceptance**

- 單元測試以本機假 Diameter 節點涵蓋 spec 每個 Scenario 與 Example；以固定識別碼比對 CER 的逐位元組內容（含 AVP 補齊與 Vendor-Specific-Application-Id 的巢狀 AVP）。
- `tests/plugins/run.sh` 對 freeDiameter 跑實機測試：允許的 peer 為 up、帶客戶端憑證的 TLS 為 up、不帶客戶端憑證的 TLS 為 down、未列入允許清單的 Origin-Host 為 `cea: 3010 DIAMETER_UNKNOWN_PEER`（freeDiameter 1.2.1 的實際行為）、沒有共同 application 為 `cea: 5010 DIAMETER_NO_COMMON_APPLICATION`、停掉的埠為 `connect: `。Debian 的 freeDiameter 沒有宣告 application 的擴充（沒有 test_app），所以以 relay 模式的節點（與任何 application 都有交集）測 up，另一個 `NoRelay` 的節點（沒有任何 application）測 5010。

**Scope**

- 在範圍內：diameter 外掛、emfor-tls 的客戶端憑證支援（只由 diameter 使用）、`network-checks` 的兩個新需求、`database-checks` 的引數清單、文件與實機測試。
- 範圍外：Non-Goals 列出的功能；daemon 端任何變更。

## Risks / Trade-offs

- [每台 emfor 主機都要在 Diameter 節點上被允許為 peer，電信設備常要求逐一設定] → 文件說明多數實作可用萬用字元或 realm 允許整批 Origin-Host；每台主機用不同的 `origin_host` 方便在對端辨識。
- [頻繁的 CER 在對端留下 peer 連線與中斷記錄] → 預設 `interval` 對 Diameter 偏短，文件建議 10 秒以上。
- [只支援 TCP，SCTP-only 的節點無法檢查] → 列為 Non-Goal；需要時另開 change。
- [對端在能力交換中途送 DWR 或其他請求] → 忽略對端的請求，只等自己請求的回應；逾時則為 `timeout`。
