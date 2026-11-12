## Why

LDAP（OpenLDAP、389 Directory Server、Active Directory）常被應用程式拿來做登入，emfor 目前沒有辦法確認它真的能處理 bind：LDAP 是二進位協定（BER 編碼），內建 `tcp` 的 `send` / `expect` 只處理文字，用 `exec` 則每次檢查都要起一個程序。HAProxy 以 `ldap-check`（匿名 bind）涵蓋這種情況。emfor-check 已有連線、TLS、`password_file` 與原因分類的共用程式碼，加一個 `ldap` 外掛成本低。

## What Changes

- `emfor-check` 新增第四個外掛參數 `ldap`：`command = ["/usr/local/libexec/emfor-check", "ldap"]`。
- 檢查內容是 LDAPv3 simple bind：沒設 `user` 時做匿名 bind；設了 `user`（bind DN，例如 `cn=monitor,dc=example,dc=org`）時用 `password_file` 的密碼 bind。BindResponse 的 resultCode 為 0 是 up，之後送 UnbindRequest 並關閉連線。
- 選項：`user`、`password_file`、`tls`、`starttls`、`tls_verify`、`server_name`、`ca_file`。`user` 必須搭配 `password_file`，`password_file` 必須搭配 `user`（避免空密碼的 bind 被伺服器當成匿名而誤判為 up）；`database`、`role` 對 `ldap` 是未知鍵。
- TLS 兩種方式：`tls = true` 為 LDAPS（一連上就 TLS），`starttls = true` 為先明文再以 StartTLS 延伸操作升級；兩者互斥，`tls_verify`、`server_name`、`ca_file` 在任一者為 `true` 時才可設定。StartTLS 被拒時為 down，原因以 `tls: ` 開頭，不退回明文。
- 原因：bind 被拒為 `auth: <resultCode> <名稱>`（例如 `auth: 49 invalidCredentials`），伺服器有 diagnosticMessage 時附在後面；回應格式不對為 `query: `；其餘沿用 emfor-check 的 `connect: `、`timeout`、`tls: `、`password_file: `、`ca_file: `。
- BER 編解碼自行實作（只處理 bind、StartTLS、unbind 所需的訊息），不加入 ldap3 等新相依。
- 實機測試：`emfor-plugins-test` 映像加入 slapd（OpenLDAP），涵蓋匿名、帳密、錯誤密碼、LDAPS、StartTLS 與 TLS 錯誤情境；重建映像前先向使用者確認。
- 文件：`docs/deploy/plugins.md` 加入 ldap 一節；`examples/emfor.toml` 加入註解掉的 ldap 範例；`docs/spectra/config.yaml` 記錄 ldap 外掛。

## Non-Goals

詳見 design.md 的 Goals / Non-Goals：不做 SASL（GSSAPI、EXTERNAL 等）、bind 後的搜尋或 WhoAmI、`role`（例如判斷唯讀副本）、LDAP referral 追蹤。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `database-checks`：「Plugin selection by argument」加入 `ldap`；「Check options validation」加入 `ldap` 的選項規則與 `starttls`；「TLS connections」加入 StartTLS；新增「LDAP check」需求。

## Impact

- 程式：crates/emfor-check/src/lib.rs（`Kind` 加入 `ldap`）、crates/emfor-check/src/options.rs（`starttls` 與 ldap 規則）、新增 crates/emfor-check/src/ldap.rs 與 BER 編解碼、crates/emfor-check/tests/live.rs（`ldap_live`）。
- 測試環境：tests/plugins/Dockerfile、tests/plugins/inside.sh（slapd 與憑證）。
- 文件：docs/deploy/plugins.md、examples/emfor.toml、docs/spectra/config.yaml。
- 不新增 Rust 相依；不影響 daemon、LMDB、client、DNS 與外掛協定。
