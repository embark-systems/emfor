## 1. 寫入 store

- [x] 1.1 實作 state-store「Metrics listen addresses in the store」，依 design「只在啟動交易寫入，由 spawn_live 帶入」：`crates/emfor-format/src/lib.rs` 新增 `KEY_METRICS_LISTEN`；`writer::spawn_live` 與 `spawn_inner` 新增 `metrics_listen: Option<Vec<String>>` 參數（`spawn_with_tick` 傳 `None`），writer 在 fresh start 的交易中寫入 JSON 字串陣列或刪除該鍵，reload 不動；`crates/emfor-daemon/src/cli.rs` 依 `[metrics] listen` 傳入。以 writer 單元測試驗證「Metrics table written at start」（值為 `["127.0.0.1:5301","[::1]:5301"]`）、「Key from an earlier run removed」，以及 reload 的 `SetLayout` 不改鍵。

## 2. 顯示

- [x] 2.1 [after: 1.1] 實作 status-command「Status text output」與「Status filtering and JSON output」的 metrics 部分：`crates/emfor-daemon/src/status.rs` 的 `Snapshot` 讀取 `meta/metrics_listen`（無法解析為字串陣列時視為沒有），`header` 在 running 時於 site 後加 `, metrics <以空白分隔的位址>`，`--json` 新增 `metrics_listen`。以 status 單元測試驗證「metrics part of the daemon line」表格五列、「Metrics addresses on the daemon line」的整行內容、「JSON metrics addresses」的有值與 `null`。
- [x] 2.2 [after: 2.1] 整合測試與文件：`crates/emfor-daemon/tests/daemon.rs` 以占住後釋放的空埠開啟 `[metrics]` 啟動 daemon，驗證 `emfor status` 的 daemon 行含 `, metrics 127.0.0.1:<port>`、`--json` 的 `metrics_listen` 為 `["127.0.0.1:<port>"]`；`docs/deploy/daemon.md` 的 status 範例、daemon line 說明與 `--json` 欄位加上 metrics 位址。以人工檢閱確認文件與 spec 一致。

## 3. 驗證

- [x] 3.1 [after: 2.2] 本機 gate 全數通過：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`，以及 Debian 映像 `emfor-clients-test` 中的同一組指令。
