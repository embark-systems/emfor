## 1. 寫入 store

- [x] 1.1 實作 state-store「DNS listen addresses in the store」，依 design「writer 以 ListenKeys 結構帶入兩個位址」：`crates/emfor-format/src/lib.rs` 新增 `KEY_DNS_LISTEN`；`writer::spawn_live` 與 `spawn_inner` 的最後一個參數改為 `ListenKeys`，fresh start 時寫入或刪除 `meta/dns_listen` 與 `meta/metrics_listen`；`crates/emfor-daemon/src/cli.rs` 的 `metrics_listen` 改為 `listen_keys`。以 writer 單元測試驗證「DNS table written at start」、「DNS key from an earlier run removed」、reload 不改鍵，既有 `metrics_listen_written_at_start` 照樣通過；以 cli 單元測試驗證 `listen_keys` 的 IPv4、IPv6 字串與兩表都沒有時的 `None`。

## 2. 顯示

- [x] 2.1 [after: 1.1] 實作 status-command「Status text output」與「Status filtering and JSON output」的 DNS 部分，依 design「daemon 行順序：site、dns、metrics」：`crates/emfor-daemon/src/status.rs` 的 `Snapshot` 讀取 `meta/dns_listen`（共用解析函式，無法解析時視為沒有），`header` 依 site、dns、metrics 的順序輸出，`--json` 新增 `dns_listen`。以 status 單元測試驗證「DNS part of the daemon line」表格五列、「DNS and metrics addresses on the daemon line」整行內容、「JSON DNS addresses」的有值與 `null`，既有 metrics 測試照樣通過。
- [x] 2.2 [after: 2.1] 整合測試與文件：`crates/emfor-daemon/tests/daemon.rs` 的 `status_shows_metrics_addresses` 同時開 `[dns]`，驗證 daemon 行以 `, dns 127.0.0.1:<dns 埠>, metrics 127.0.0.1:<metrics 埠>` 結尾、`--json` 的 `dns_listen` 為該位址；`docs/deploy/daemon.md` 的 status 範例、daemon line 說明與 `--json` 欄位加上 DNS 位址。以人工檢閱確認文件與 spec 一致。

## 3. 驗證

- [x] 3.1 [after: 2.2] 本機 gate 全數通過：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`，以及 Debian 映像 `emfor-clients-test` 中的同一組指令。
