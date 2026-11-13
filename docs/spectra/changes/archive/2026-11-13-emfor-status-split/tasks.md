## 1. status 計算與輸出

- [x] 1.1 crates/emfor-daemon/src/status.rs 讀 `meta/peers` 的 `listen`，在 daemon 執行中、`listen` 與所有成員位址都是合法 `ip:port` 時，以 `listen` 為本機、`live` 為 true 的成員為存活成員，用 `quorum::assigned` 算出顯示的每台後端的負責主機；本機不在其中的後端為 voted。依 status-command「Status text output」：voted 後端的那一行加 `voted` 標記（位置在 `local-up` 之後、原因之前），顯示的後端中有 voted 時 `peers:` 行尾加 `, checks <n>/<m> backends here`。驗證：新單元測試 `voted_backends_table` 照規格範例「voted backends of api.svc.emfor at 10.0.0.5:5302」逐列，並重現情境「Backends this daemon is not an assigned checker of」；既有的 `peer_group_lines` 與 `peers_line_table` 不改仍通過（沒有 voted 時輸出不變）。
- [x] 1.2 `emfor status --json` 新增 `assigned_checkers`，依 status-command「Status filtering and JSON output」：條件不符時為 `null`，否則列出列出服務的每台後端與依排名的負責主機（沒有時為空陣列），受 `--service` 限制。驗證：新單元測試 `json_assigned_checkers` 重現情境「JSON assigned checkers」，含 `meta/peers` 不存在與 daemon 停止時為 `null`；既有 `json_fields` 測試補上新欄位後通過。[after: 1.1]

## 2. 文件與完整檢查

- [x] 2.1 docs/manual/08-operations.md 的聯防群組一節說明 `voted` 標記、`checks <n>/<m> backends here` 與 `assigned_checkers`，並註明它們依 `meta/peers` 的存活狀態計算（最多落後約 1 秒）；CHANGELOG.md 加一條。驗證：人工審閱與規格一致。[after: 1.2]
- [x] 2.2 完整檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過，Cargo.lock 沒有變更。驗證：三個指令結束碼為 0 並記錄測試套件數。[after: 2.1]

## 3. 驗證與審查後的修正

- [x] 3.1 規格「Status filtering and JSON output」把 `checkers` 的寫法改為「與 `meta/peers` 相同的標準 `ip:port` 寫法（IPv6 為 `[addr]:port`）」，與實作一致。驗證：`spectra analyze emfor-status-split` 沒有 Critical 或 Warning，`spectra validate emfor-status-split` 通過。
- [x] 3.2 status.rs 的文字輸出每次只計算一次負責主機：`services_lines` 算出後交給 `peer_lines` 與後端標記共用；`json_assigned_checkers` 補上 `listen` 為 `"bad"` 與成員位址不合法時 `assigned_checkers` 為 `null`。驗證：`cargo test -p emfor-daemon --lib status` 通過，既有 `voted_backends_table`、`peer_group_lines`、`peers_line_table` 不改仍通過。[after: 3.1]
- [x] 3.3 docs/manual/08-operations.md 加一段 `emfor status --json` 中 `assigned_checkers` 的簡短範例（`10.0.0.11:8080` 那一筆）。驗證：人工審閱與規格情境「JSON assigned checkers」一致。
- [x] 3.4 完整檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過，Cargo.lock 沒有變更。驗證：三個指令結束碼為 0 並記錄測試套件數。[after: 3.2, 3.3]
