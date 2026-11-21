## 1. 程式

- [x] 1.1 `crates/emfor-daemon/src/config.rs`：`Config` 新增公開欄位 `plugins: Vec<PluginSpec>`，由 `root()` 目前被丟掉的 `self.plugins(root)` 結果填入。完成時：`cargo build` 無警告，既有測試不改就通過。
- [x] 1.2 [after: 1.1] `crates/emfor-daemon/src/cli.rs` 的 `credential_warnings`：依 plugin 宣告判斷種類（指令程式檔名為 `emfor-check` 時取第一個參數，否則用 `check` 名稱）；`redis`、`ldap` 的條件與訊息不變；經 `emfor-check` 執行的 `mysql`、`pgsql`、`amqp` 在有 `password_file` 且 `tls` 不是 `true` 時警告，訊息含 `password`、`tls is not true` 與「看到流量或在後端位置回應的人都能取得」。完成時：`mongodb` 與非 `emfor-check` 的 `pgsql` 名稱不警告。
- [x] 1.3 [after: 1.2] `cli.rs` 的測試：(a) `pgsql`、`mysql`、`amqp` 經 `emfor-check` 宣告、有 `password_file` 沒有 `tls`，各一行警告；加 `tls = true` 就沒有；(b) 宣告成別的名稱（`[plugins."redis-check"] command = ["/x/emfor-check", "redis"]`）的 redis 仍警告；(c) `mongodb` 經 `emfor-check` 宣告不警告；(d) 名稱叫 `pgsql` 但指令不是 `emfor-check` 不警告；(e) 既有 redis 與 ldap 測試不改就通過。完成時：測試在 Debian 容器通過，並說明對舊程式碼 (b) 與 (a) 失敗。

## 2. 文件與檢查

- [x] 2.1 [after: 1.3] `docs/deploy/plugins.md` 那一條改寫（redis、ldap、mysql、pgsql、amqp 會警告，mongodb 不會，種類由 plugin 的指令決定而非名稱）；`CHANGELOG.md` 的 Unreleased 加條目（含 Checked 與 Not verified：driver 行為是審查者讀原始碼所得，沒有對真實伺服器試過）。在 Debian 容器跑 fmt、clippy `-D warnings`、全部測試，把數字寫進條目。
