## Context

`emfor-check` 目前有 `redis`、`mysql`、`pgsql` 三個外掛（`Kind`），共用 `options.rs`（選項驗證、`password_file`、TLS 選項）與 `net.rs`（`connect`、`open` 含 TLS 握手時間預算、`io_failure` 原因分類）。redis 的協定（RESP）是自己寫的，約 156 行，驅動都只連結進 emfor-check。

LDAP 檢查只需要三種請求（BindRequest、StartTLS ExtendedRequest、UnbindRequest）和兩種回應（BindResponse、ExtendedResponse），全部是 BER 編碼的 LDAPMessage（RFC 4511）。ldap3 0.12.1 雖有 `tls-rustls-aws-lc-rs` feature，但它依 URL 主機連線並驗證憑證，尚未確認能做到「連到後端 IP、以 `server_name` 驗證」，而這是 emfor-check 其他外掛的既有行為。

`emfor-tcp-tls`（尚未實作）會把 emfor-check 的 TLS 程式碼搬到 `crates/emfor-tls`。兩個 change 互不依賴，誰先做都可以。

## Goals / Non-Goals

**Goals:**

- `emfor-check ldap` 以 LDAPv3 simple bind 判斷 LDAP 伺服器能否處理登入，支援匿名與帳密。
- 支援 LDAPS 與 StartTLS，TLS 選項與其他外掛一致。
- bind 被拒時的原因帶 resultCode 與名稱，方便判斷是密碼錯、帳號被鎖或伺服器要求加密。
- 不新增 Rust 相依。

**Non-Goals:**

- SASL bind（GSSAPI、EXTERNAL、DIGEST-MD5 等）。
- bind 後的搜尋、WhoAmI 或檢查特定 entry 是否存在。
- `role`（例如判斷唯讀副本或 consumer）。
- referral 追蹤、LDAP over UDP（CLDAP）、ldapi（Unix socket）。

## Decisions

### 只做 simple bind

up 的條件是 BindResponse resultCode 為 0，和 HAProxy `ldap-check` 相同。bind 能成功代表伺服器能讀取後端資料庫並處理認證，這是應用程式最在意的事。搜尋或 WhoAmI 需要額外選項（base DN、filter）且多數部署用不到，留待需要時再加。

### 自己寫 BER 編解碼

只實作所需的 BER：定長編碼的 SEQUENCE、INTEGER、ENUMERATED、OCTET STRING、`[0]` simple 密碼、`[APPLICATION n]` 標籤，長度支援短格式與長格式（最多 4 位元組）。回應先讀 tag 與長度，總長超過 65536 位元組即視為格式錯誤，避免惡意或錯誤的伺服器耗用記憶體。估計約 150 行（不含測試），和 redis 的 RESP 相近；實作後量測行數記在 tasks 的完成紀錄，結果不影響其他決定。替代方案 ldap3 會帶入 lber、nom、x509-parser 等相依，且連線與憑證驗證方式和其他外掛不同。

### LDAPS 與 StartTLS 都支援

`tls = true` 走 `net::open` 的 TLS（與其他外掛相同）；`starttls = true` 先以 `net::connect` 建立明文連線，送出 messageID 1 的 ExtendedRequest（requestName `1.3.6.1.4.1.1466.20037`），收到 resultCode 0 的 ExtendedResponse 後在同一條連線上做 TLS 握手，之後 bind 使用 messageID 2。為此把 `net::open` 中的握手部分拆成可單獨呼叫的函式（例如 `net::handshake(stream, tls, ip, remaining)`）；若 `emfor-tcp-tls` 已完成，則直接使用 emfor-tls 對應的握手函式。StartTLS 被拒或回應不是 ExtendedResponse 時為 down `tls: StartTLS refused: …`，絕不在明文上 bind。389 port 加 StartTLS 是 OpenLDAP 常見部署，因此兩種都要支援。

### user 與 password_file 必須一起設定

`user` 是 bind DN；只有 DN 沒有密碼的 bind 在 RFC 4513 是「unauthenticated bind」，許多伺服器會當成匿名而回成功，等於誤判為 up。因此 `user` 沒有 `password_file`、或 `password_file` 沒有 `user` 都是設定錯誤。密碼在明文連線上傳送是允許的，與 redis、mysql、pgsql 一致；文件建議搭配 `tls` 或 `starttls`。

### resultCode 名稱照 RFC 4511

原因寫成 `auth: <code> <name>`，名稱取自 RFC 4511 第 4.1.9 節的列表（0–80 中有定義者，例如 8 strongerAuthRequired、49 invalidCredentials、53 unwillingToPerform），未定義的碼只寫數字。伺服器的 diagnosticMessage 非空時以 `: ` 接在後面；它不會包含密碼（伺服器不回傳密碼），但仍由現有的「原因不含密碼」測試把關。

### 實機測試用 slapd

在 `emfor-plugins-test` 映像加入 Debian 的 slapd 與 ldap-utils，`inside.sh` 以 slapd.conf 啟動兩個實例：主實例聽 389（允許 StartTLS）與 636（LDAPS），使用 `db.test` 憑證，含 base `dc=example,dc=org` 與一個帶密碼的 `cn=monitor` entry；第二個實例聽 3389、不設定 TLS，用來驗證 StartTLS 被拒。重建映像前先向使用者確認。

## Implementation Contract

**行為**

- `emfor-check ldap` 握手回 `name` `ldap`。
- 每次檢查：連線（LDAPS 時含握手；StartTLS 時明文 → ExtendedRequest → 握手）→ BindRequest（version 3，`user` 或空字串，`password_file` 內容或空字串）→ 讀 BindResponse → resultCode 0 時送 UnbindRequest 並關閉、回 up；否則回 down。
- 原因：`auth: <code>[ <name>][: <diagnosticMessage>]`、`tls: StartTLS refused: <code>[ <name>][: <diagnosticMessage>]`、`query: <說明>`（格式錯誤、messageID 不符、非預期的操作、Notice of Disconnection、超過 65536 位元組）；`connect: `、`timeout`、`tls: `、`password_file: `、`ca_file: ` 與其他外掛相同。
- 選項驗證依 spec "Check options validation"（含 ldap 的八列範例）。

**介面 / 資料形狀**

- `Kind::Ldap`，`Kind::from_name("ldap")`。
- 新模組（例如 `ber.rs` 編解碼、`ldap.rs` 檢查）；BER 解析對外只提供讀出一個 LDAPMessage 並回傳 messageID、protocolOp 標籤、resultCode、diagnosticMessage。
- 匿名 BindRequest 的位元組必須是 `30 0c 02 01 01 60 07 02 01 03 04 00 80 00`。

**驗收**

- 單元測試：BER 編碼的固定位元組（匿名 bind、帶 DN 與密碼的 bind、StartTLS、unbind）、長格式長度、截斷與超長回應；以假伺服器驗證 resultCode 原因表四列、StartTLS 被拒 `tls: StartTLS refused: 2 protocolError`、格式錯誤的回應為 `query: `；選項驗證八列 ldap 範例。
- `crates/emfor-check/tests/cli.rs`：`emfor-check ldap` 的握手。
- `crates/emfor-check/tests/live.rs` 的 `ldap_live`（在 `tests/plugins/run.sh` 中執行）：匿名 up、帳密 up、錯誤密碼 `auth: 49 invalidCredentials`、沒有監聽的 port `connect: `、LDAPS 正確名稱 up、LDAPS 名稱不符 `tls: `、`tls_verify = false` up、StartTLS 正確 up、StartTLS 名稱不符 `tls: `、對 3389 StartTLS 為 `tls: StartTLS refused: `、`tls = true` 對 389 為 `tls: `、密碼輪替不需 reload。
- 本機檢查：macOS 與 Debian 映像的 fmt、clippy、test；`tests/plugins/run.sh`；`cargo tree -p emfor-check` 沒有新增套件。

**範圍**

- 範圍內：emfor-check 的 ldap 外掛、選項、BER、實機測試、文件。
- 範圍外：Non-Goals 所列項目；daemon、LDAP 以外的外掛行為。

## Risks / Trade-offs

- [自寫 BER 遇到少見的編碼（例如不定長度）] → LDAP 規定只能用定長編碼（RFC 4511 第 5.1 節），遇到不定長度回 `query: `；以 OpenLDAP 實機測試與 Active Directory 的已知回應格式（同為定長）把關，AD 本身未實測，文件註明。
- [匿名 bind 被伺服器允許時，down 的伺服器資料庫仍可能讓匿名 bind 成功] → 文件建議用專用帳號做帳密 bind，較能反映認證功能。
- [每次檢查都 bind 會在伺服器日誌留下大量紀錄] → 與 HAProxy 相同的取捨；文件提醒調整檢查間隔或伺服器的日誌等級。
- [映像變大] → slapd 與 ldap-utils 約數 MB（估計，重建後量測並記錄映像大小）。
