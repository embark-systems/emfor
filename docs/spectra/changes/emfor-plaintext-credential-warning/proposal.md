## Why

`emfor check-config` 與 daemon 啟動時，對「有 `password_file` 卻沒有 TLS」的檢查發出警告，但只在服務的 `check` 名稱剛好是 `redis` 或 `ldap` 時才發（`crates/emfor-daemon/src/cli.rs` 的 `credential_warnings`）。2026-11-21 的 Fable 審查指出兩個缺口，下面的說明來自審查者對第三方 driver 原始碼的閱讀，我只確認了 `cli.rs` 的條件，沒有自己重讀 driver：

- `amqp`（`emfor-check/src/amqp.rs` 在沒有 `tls` 時用 SASL PLAIN 送出 `\0user\0password`，一定是明文）、`pgsql`（`SslMode::Disable`：任何回 `AuthenticationCleartextPassword` 的對象都收到明文密碼，伺服器設成 SCRAM 也擋不住冒充者）與 `mysql`（沒有 TLS 時 mysql_async 向對方要 RSA 公鑰並送出加密後的密碼，冒充的伺服器可以給自己的金鑰）都沒有警告。`docs/deploy/plugins.md` 與程式碼註解把它們排除的理由是「是否明文取決於伺服器的驗證方式」，但驗證方式是由回答的一方選的，不是設定者。
- 警告用 plugin 的名稱比對，管理者把 plugin 宣告成別的名稱（`[plugins."redis-check"]`，指令仍是 `emfor-check redis`）就連 redis 與 ldap 的警告也消失。

## What Changes

- `Config` 多帶 `[plugins]` 的有效宣告（`plugins: Vec<PluginSpec>`，`config.rs` 本來就解析它，只是丟掉）。
- `credential_warnings` 先由 plugin 宣告判斷檢查的種類：指令的程式是 `emfor-check`（以檔名判斷）時，種類是指令的第一個參數；否則維持現狀，用 `check` 名稱。對 `redis`、`ldap`（任何來源，維持現狀）與經由 `emfor-check` 執行的 `mysql`、`pgsql`、`amqp`：有 `password_file` 且 `tls` 不是 `true`（`ldap` 另有 `starttls`）就警告。`mongodb` 不警告（SCRAM 不會把密碼送上網路，這是審查者的說法，沒有自己驗證）。
- 新種類的警告訊息說清楚原因：密碼在網路上沒有保護，能看到流量、或能在後端的位置上回應的人都能取得。redis 與 ldap 的訊息文字不變。
- 文件：`docs/deploy/plugins.md` 那一條，與 CHANGELOG 的 Unreleased。

## Non-Goals (optional)

- 不改預設值（`tls` 預設仍是 false），也不把警告升為錯誤：「設定能運作就繼續運作」的原則不變。
- 不處理 `emfor-check` 以外的第三方 plugin（只有名稱為 `redis` 或 `ldap` 時照舊警告）。
- 不處理 `mongodb`、`radius` 的共享密鑰、其他協定。
- 沒有 design.md：做法小，上面已寫出取捨。

## Impact

- Affected specs: none（警告不在任何 spec 裡）
- Affected code:
  - New: (none)
  - Modified: `crates/emfor-daemon/src/config.rs`（`Config.plugins`）、`crates/emfor-daemon/src/cli.rs`（`credential_warnings` 與測試）、`docs/deploy/plugins.md`、`CHANGELOG.md`
  - Removed: (none)
- Compatibility: 沒有新的錯誤；使用 `mysql`、`pgsql`、`amqp` 且有 `password_file` 卻沒有 `tls` 的設定，從這個版本起在 `check-config` 與啟動日誌多出警告。
