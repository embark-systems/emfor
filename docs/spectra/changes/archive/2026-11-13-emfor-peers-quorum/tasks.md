## 1. 訊息與計票（peers.rs）

- [x] 1.1 訊息種類 result：負責主機每次 up / down 檢查後送出——peers.rs 的 `Kind` 新增 `Result`（`service`、`digest`、`backend`、`outcome`、僅 down 時的 `reason`、`interval_ms`），編碼時 `reason` 截到 256 bytes；解碼時缺欄位、`outcome` 不是 up / down、`interval_ms` 不是正整數一律為 `malformed`。依「Peer message authentication and replay protection」修改後的範例表，`kind = "assign"` 仍只算存活。驗證：peers.rs 單元測試涵蓋 `result` 編解碼往返、三種 malformed 情形與 `assign` 只算存活。
- [x] 1.2 負責主機的選法：存活成員加本機，rendezvous hashing 取前 3——在 peers.rs 新增純函式，輸入服務、後端 id、本機 listen 與存活成員，回傳依排名的負責主機（少於 3 個候選回傳空）；emfor-daemon 的 Cargo.toml 加 `md-5 = "0.11"`（Cargo.lock 只多一行相依、不新增套件）。對應「Assigned checkers and check results」的選法部分。驗證：單元測試以規格「rendezvous values for api.svc.emfor」表格的 10 個值、`10.0.0.8:5302` 失聯後的選法、兩個候選回傳空、同值時 listen 字串小者勝。[after: 1.1]
- [x] 1.3 全區判斷：至少 2 票 down 才判全區 down，以及合成結果與原因前綴——新增 `Quorum`（`Arc` 共享、內部 `Mutex`）保存存活集合與票表（每則 `result` 的結果、原因、摘要、`interval_ms`、收到時間），並提供每次檢查後呼叫的方法，回傳合成結果、本機是否為負責主機、是否為不一致（down 票數、新鮮票數），完全依「Region verdict and combined result」。驗證：單元測試逐列跑規格「combined result」與「which kept results are votes」兩張表（以注入的 `Instant` 控制時間）。[after: 1.2]
- [x] 1.4 不一致與摘要不同的追蹤——`Quorum` 記錄不一致後端（開始時記 info `backend <id> of <service> is down at <d>/<f> peers but up here`、不再符合時移除）與摘要不同（收到 `down` 或 `result` 摘要不同時記錄、60 秒未再收到或成員移除時移除，debug log）；`Targets` 更新後刪除已不存在的服務與後端的票、不一致與摘要不同記錄。驗證：單元測試涵蓋加入、移除、60 秒到期、成員移除、服務移除五種情形。[after: 1.3]

## 2. 接到聯防 task 與後端 task（程式結構）

- [x] 2.1 程式結構：聯防 task 收送 result——`DownSender` 改為傳遞列舉（`down` 通知或 `result`），聯防 task 對每個其他成員送出 `result` 並累加 sent；收到通過驗證的 `result` 時累加 received，本機有該服務與後端才寫入 `Quorum` 票表；存活變化同步到 `Quorum`。驗證：peers.rs 以兩個 `start_on` 實例的單元測試確認 `result` 從 A 進到 B 的票表、`emfor_peer_results_total` 用的兩個計數各為預期值，未知服務的 `result` 只計 received。[after: 1.4]
- [x] 2.2 「Health state machine with rise and fall」吃合成結果——schedule.rs 的 `spawn_backend` 在每次本機檢查後經 `PeerLink` 的 `Quorum` 取得合成結果，用於 `BackendHealth::apply`、store 的 state / reason、狀態變化 log、`CheckOutcome`、快節奏連查的停止條件與「Backend failure notification」的 healthy → unhealthy 通知；延遲只在合成結果為 up 時計入；本機為負責主機且本機結果為 up / down 時送出 `result`（原始結果）。沒有 `[peers]` 時 `Quorum` 為 `None`，走原本的路徑。驗證：schedule.rs 單元測試以假的 `Quorum`（兩張 down 票）讓本機 up 的後端在 `fall` 次後變 unhealthy 且原因以 `peers 2/` 開頭；本機 down、只有一張 down 票時原因為 `local: ` 開頭；本機不是負責主機時不送 `result`；health-checking 既有測試不修改即通過。[after: 2.1]
- [x] 2.3 supervisor.rs 與 cli.rs 把 `Quorum` 交給每個後端 task 的 `PeerLink`，重載時 `publish_targets` 之後清掉已移除的服務與後端在 `Quorum` 中的資料。驗證：supervisor.rs 的 `peer_targets_follow_reloads` 擴充：重載移除一台後端後，其票與不一致記錄消失。[after: 2.2]

## 3. store、status 與指標

- [x] 3.1 meta/peers：聯防狀態寫進 store 給 status 讀——emfor-format 新增 `KEY_PEERS = "meta/peers"`；writer 在啟動交易（與 `meta/started_at` 同一交易）寫入初始內容或在沒有 `[peers]` 時刪除，並新增一種訊息帶新內容、於下一次 commit 寫入；聯防 task 每秒產生 JSON（形狀依「Peer group state in the store」），內容改變才送給 writer。驗證：writer.rs 單元測試確認啟動寫入、無 `[peers]` 刪除、內容更新進下一次 commit；peers.rs 單元測試確認 JSON 形狀與排序。[after: 2.3]
- [x] 3.2 emfor status 顯示聯防群組——status.rs 讀 `meta/peers`，依「Status text output」在 daemon 那行後印 `peers: <n>/<m> live[, not live …]` 與摘要不同行、後端行加 `local-up` 標記（僅 daemon running 時），依「Status filtering and JSON output」在 `--json` 加 `peers` 欄位（不受 `--service` 篩選）。結束碼不變。驗證：status.rs 單元測試涵蓋規格「Peer group lines」情境、「peers line」範例表每一列與「JSON peer state」情境；既有 status 測試不修改即通過。[after: 3.1]
- [x] 3.3 指標：「Daemon metrics」新增 `emfor_peer_results_total{direction="sent"|"received"}` 與 `emfor_peer_disagreements`，沒有 `[peers]` 時不輸出。驗證：metrics.rs 的 `peer_metrics` 測試擴充這兩組序列的輸出與數值。[after: 2.1]

## 4. 整合測試與文件

- [x] 4.1 crates/emfor-daemon/tests/peers.rs 新增三個 daemon 的整合測試：同一份 exec 檢查指令依各 daemon 行程的環境變數指向的旗標檔決定 up / down。兩台 down、一台 up 時，第三台在 `fall` 次檢查內判 unhealthy、原因以 `peers 2/` 開頭、store 的 `meta/peers` 列出不一致、`emfor status` 輸出含 `local-up`、`/metrics` 的 `emfor_peer_disagreements` 為 1；只有一台 down 時另兩台維持 healthy、該台原因以 `local: ` 開頭；兩個 daemon 的群組維持本機判斷。另確認每次檢查只送一輪 `result`（`emfor_peer_results_total{direction="sent"}` 與檢查次數相符）。驗證：`cargo test -p emfor-daemon --test peers` 通過；暫時讓合成結果直接回傳本機結果時，`peers 2/` 那個測試失敗。[after: 3.2, 3.3]
- [x] 4.2 文件說明第二階段：docs/manual/01-concepts.md 的聯防群組一節加入負責主機、多數決、全區 down 優先與退回本機模式；docs/manual/08-operations.md 加 `peers 2/3:`、`local:` 原因、`local-up`、`meta/peers`、新指標與排查步驟；docs/manual/03-configuration.md 註明多數決不需新設定；docs/deploy/daemon.md 指標表與聯防設定一節；CHANGELOG.md；docs/spectra/config.yaml 專案說明改為第二階段已完成；docs/spectra/LANGUAGE.md 加「負責主機」「全區判斷」。驗證：人工審閱文件與規格一致，`emfor check-config --config examples/emfor.toml` 仍通過。[after: 4.1]
- [x] 4.3 完整檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過，`git diff Cargo.lock` 只有 emfor-daemon 多一行 md-5 相依。驗證：三個指令結束碼為 0 並記錄測試套件數。[after: 4.2]

## 5. 驗證與審查後的修正

- [x] 5.1 規格與設計對齊實作：peer-group 規格「combined result」範例表加上「本機是否為負責主機」欄，每列可直接重現；design.md「程式結構」改寫為實際的 quorum.rs、`PeerHooks { out, quorum, targets }`、`PeerOut` / `PeerSender`、`StateOut` 與 writer 的 `PeerState` / `ListenKeys.peers`；proposal.md 影響範圍加 crates/emfor-daemon/src/quorum.rs 與 lib.rs；docs/manual/03-configuration.md 改為每個成員仍自己檢查每個後端、三個負責主機另外送出結果。驗證：`spectra analyze emfor-peers-quorum` 沒有 Critical 或 Warning，`spectra validate emfor-peers-quorum` 通過。
- [x] 5.2 quorum.rs 測試對照規格：`combined_result_table` 逐列照規格表（含本機非負責主機、本機結果為 error 且 down、down、up 投票得 `peers 2/3: refused` 那一列）；同分排序抽成 `rank_order`，`assigned()` 與 `equal_values_go_to_the_smaller_listen_string` 共用。驗證：`cargo test -p emfor-daemon --lib quorum` 通過。[after: 5.1]
- [x] 5.3 writer 收到 `PeerState` 時記住最新值，之後的 `fresh_start` 寫入這個值而不是啟動時的初始值。驗證：新測試 `peer_state_before_start_survives_it` 通過，且暫時改回只放進 pending 時失敗。
- [x] 5.4 peers.rs 收到訊息時只在送出者原本不是存活成員時才更新 `Quorum` 的存活集合（每秒的 tick 仍更新）；`received` 的說明註解移到函式上方。supervisor.rs 的 `peer_targets_follow_reloads` 在重載移除後端後確認 `quorum.disagreements()` 為 0。驗證：`cargo test -p emfor-daemon --lib supervisor` 與 `cargo test -p emfor-daemon --test peers` 通過。
- [x] 5.5 完整檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過。驗證：三個指令結束碼為 0 並記錄測試套件數。[after: 5.2, 5.3, 5.4]
