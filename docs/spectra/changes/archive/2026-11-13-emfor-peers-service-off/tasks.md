## 1. 設定

- [x] 1.1 crates/emfor-daemon/src/config.rs 的 `Settings` 加 `peers: bool`（預設 `true`），`SETTING_KEYS` 加 `peers`，`[defaults]` 與服務表皆可設、服務表優先；值不是 TOML 布林時錯誤 `peers must be true or false`（路徑 `defaults.peers` 或 `services."<name>".peers`）；沒有 `[peers]` 時照樣接受。依 service-config「TOML config with defaults and per-service overrides」與「Config validation rules」。驗證：config 單元測試涵蓋 service-config「effective settings resolution」新增的兩列與預設 `true`、validation 範例的三列 `peers`；既有 config 測試不改仍通過。

## 2. daemon 行為

- [x] 2.1 crates/emfor-daemon/src/schedule.rs 後端迴圈每圈依 `s.peers` 取用聯防：`false` 時等同沒有 `PeerHooks`（每次自己檢查、結果與理由原樣套用、不送 `result` / `down`、不呼叫 `quorum.skip()`），已在進行的快速重檢跑完剩下次數。依 peer-group「Services left out of the peer group」。驗證：新單元測試在有 hooks、兩位負責主機票為 `down` 的情況下，`peers = false` 的後端套用自己的 `up`、沒有 `PeerOut`、`quorum.skipped()` 不變；把 `s.peers` 的判斷暫時拿掉時測試失敗。[after: 1.1]
- [x] 2.2 crates/emfor-daemon/src/quorum.rs 加 `Quorum::set_off(names)`（排序、去重），`summary` 在 `disagreements` 後輸出 `services_off`；crates/emfor-daemon/src/peers.rs 的 `start` / `start_on` 收 `services_off` 並在算 `initial_state` 前設定，crates/emfor-daemon/src/cli.rs 依設定傳入。依 state-store「Peer group state in the store」。驗證：quorum 單元測試的 `summary` 期望字串加上 `"services_off":[]` 與排序後的清單；crates/emfor-daemon/src/writer.rs 等測試常數同步更新後通過；state-store 情境「Peer state written at start」的啟動值含 `"services_off":[]`。[after: 1.1]
- [x] 2.3 crates/emfor-daemon/src/supervisor.rs 的 `publish_targets` 只把 `peers = true` 的服務放進 `Targets` 與 `quorum.retain` 的 keep，並以 `quorum.set_off` 交出關閉清單；reload 改 `peers` 時立即生效、後端 task 不重啟。依 service-config「Reload on SIGHUP applies only a fully valid config」與 peer-group「Services left out of the peer group」（reload 丟掉保存的狀態）。驗證：supervisor 單元測試：關閉的服務不在 `Targets`、其意見不一被丟掉、`summary` 列出它；reload 改回 `true` 後重新出現在 `Targets`，後端 task 的 generation 不變。[after: 2.2]
- [x] 2.4 整合測試 crates/emfor-daemon/tests/peers.rs：三個 daemon，其中一個對服務設 `peers = false`；其他兩個的檢查看到 `down` 而它看到 `up`。驗證：它的後端保持 `healthy`、`meta/peers` 沒有意見不一且 `services_off` 列出該服務、其他兩個收到的 `result` 都不是來自它，其他兩個把後端標為 `unhealthy`（peer-group 情境「A service left out on one member」）。[after: 2.1, 2.3]

## 3. status

- [x] 3.1 crates/emfor-daemon/src/status.rs 的 `PeerState` 加 `services_off`（缺少時為空），清單內服務的後端不標 `voted`、不計入 `checks n/m backends here`，`--json` 的 `assigned_checkers` 中其 `checkers` 為 `[]`。依 status-command「Status text output」與「Status filtering and JSON output」。驗證：新單元測試重現 status-command 情境「Service left out of the peer group is not voted」與「JSON assigned checkers of a service left out」；既有 `voted_backends_table`、`json_assigned_checkers` 不改仍通過（缺欄位時照舊）。[after: 2.2]

## 4. 文件與完整檢查

- [x] 4.1 docs/manual/03-configuration.md（`[defaults]` 表與服務表加 `peers`、reload 表加一列、`[peers]` 一節說明以服務關閉與「各成員設成一樣」）、docs/manual/01-concepts.md（聯防群組一節一句話）、docs/manual/08-operations.md（`services_off` 與 status 的影響）、examples/emfor.toml、CHANGELOG.md。驗證：人工審閱與規格一致，crates/emfor-daemon/tests/daemon.rs 中驗證 examples/emfor.toml 的既有測試通過。[after: 2.4, 3.1]
- [x] 4.2 完整檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過，Cargo.lock 沒有變更。驗證：三個指令結束碼為 0 並記錄測試套件數。[after: 4.1]

## 5. 驗證與審查後的修正

- [x] 5.1 specs/service-config/spec.md 刪掉「Config validation rules」範例表中誤複製、沒有結尾的 `<!-- @trace` 片段，讓三列 `peers` 範例接在 `[peers] members` 那一列之後，「Reload on SIGHUP applies only a fully valid config」不再被註解蓋住。驗證：檔案中 `<!--` 與 `-->` 數量相同（皆為 0），`spectra analyze emfor-peers-service-off` 沒有 Critical 或 Warning，`spectra validate emfor-peers-service-off` 通過。
- [x] 5.2 crates/emfor-daemon/src/peers.rs 單元測試：對 `peers = false`（不在 `Targets`）的服務送 `down`，`Targets::deliver` 回錯誤且該後端的 `Recheck` 沒有收到報告，也不記 digest 不符（peer-group 情境「Down message for a service left out」）。驗證：`cargo test -p emfor-daemon --lib peers` 通過。
- [x] 5.3 crates/emfor-daemon/tests/peers.rs 整合測試：兩個服務 `peers = false` 啟動，`meta/peers` 的 `services_off` 依序列出兩者；把其中一個改成 `peers = true` 並送 SIGHUP，2 秒內只剩另一個（state-store 情境「Services left out are listed」）。supervisor.rs 新測試前補上空行。驗證：`cargo test -p emfor-daemon --test peers` 通過。
- [x] 5.4 完整檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過，Cargo.lock 沒有變更。驗證：三個指令結束碼為 0 並記錄測試套件數。[after: 5.1, 5.2, 5.3]
