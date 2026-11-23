## 1. 偵測

- [x] 1.1 在 `Quorum`（crates/emfor-daemon/src/quorum.rs）加入沉默 checker 的偵測：新函式 `Quorum::watch` 在每次本機檢查後（由 `schedule.rs` 的 `combine` 包裝在 `Quorum::combine` 之後呼叫）記下 assigned checker 中本機以外的成員、digest、服務 `interval` 與時間；新函式 `update_silent(now)` 依「peer-group / Silent assigned checkers」的定義與窗口 `max(10 秒, 6 × interval)` 維護沉默清單，並依規格 `warn`／`info` 各記一次；提供 `silent_count()` 與排序後的沉默清單讀取；回 `error`（不送結果）的成員與其他沉默成員同樣被回報，不另外處理（design「回 `error` 的檢查不特別處理」），並有一個測試固定這一點。驗證：quorum.rs 單元測試（用傳入的 `now` 控制時間）涵蓋該規格的七個 scenario，每個測試標註 `// spectra-scenario: peer-group / <scenario>`；透過 `tools/docker-test.sh 'cargo test -p emfor-daemon quorum'` 通過。
- [x] 1.2 [after: 1.1] 把沉默清單寫進 `Summary`（`silent_checkers`，空時不寫出欄位，見 state-store「Silent checkers in the peer state」），讓 `status --json` 與 `meta/peers` 帶出；`status.rs` 讀取端加 `#[serde(default)]` 欄位，並在 digest 不符的行之後輸出 `peers: no results from <member> for <service> (<n> backends)`（status-command「Silent checker line in status text」）。驗證：`summary_json` 風格的測試確認有沉默時欄位存在、沒有時完全不出現；status.rs 文字行測試涵蓋該規格三個 scenario（含 `--service` 篩選與缺欄位）；`tools/docker-test.sh 'cargo test -p emfor-daemon'` 通過。
- [x] 1.3 [after: 1.1] 在 peer tick（crates/emfor-daemon/src/peers/task.rs）先呼叫 `update_silent`，再 `summary`，並把 `silent_count()` 存進 `PeerStats`（crates/emfor-daemon/src/peers/stats.rs）；`metrics.rs` 在有 `[peers]` 時輸出 `emfor_peer_silent_checkers`（gauge，沒有沉默時為 0）。驗證：metrics.rs 輸出測試涵蓋 metrics-endpoint「Silent checker metric」兩個 scenario；`tools/docker-test.sh 'cargo test -p emfor-daemon metrics'` 通過。

## 2. 三容器驗證

- [x] 2.1 [after: 1.2, 1.3] `tools/peer-mismatch-test.sh` 加斷言：情境 2 在切斷後 15 秒內出現 C 的沉默訊號（`status --json` 的 `silent_checkers`、status 文字行、metric 為 1），情境 1 持續 30 秒內不出現，情境 3 不出現（只有 digest 不符），情境 4 在 A、B 設 `peers = false` 後訊號消失；標頭註解說明新斷言。驗證：執行 `tools/peer-mismatch-test.sh`，輸出 `all checks passed`；若訊號時間超出 15 秒或情境 1 有誤報，回頭調整 design 的門檻常數並重跑。

## 3. 文件

- [x] 3.1 [after: 1.2, 1.3] 手冊第 11 章「When members differ」與第 16 章那一列（docs/manual 與 docs/manual.zh_TW）：不再寫「沒有警告」，改寫新的訊號（status 行、`silent_checkers`、metric、log）、窗口（約 10 秒或 6 個 interval）、與 `error` 無法區分、以及建議的處理。驗證：內容審閱對照 design 的輸出格式；`python3 tools/check-manual-links.py` 通過。
- [x] 3.2 [after: 1.2, 1.3] 手冊第 12 章（英文與 zh_TW）：status 文字的 peers 行、`--json` 的 `peers.silent_checkers`、metrics 表加 `emfor_peer_silent_checkers`；第 11 章 Metrics 表同樣加一列。驗證：內容審閱；連結檢查通過。
- [x] 3.3 [after: 1.1] CHANGELOG.md 的 Unreleased 加一筆；docs/spectra/LANGUAGE.md 加「沉默的負責主機」（silent checker）一列，定義與避免使用的說法。驗證：內容審閱，詞與 spec 一致。

## 4. 收尾

- [x] 4.1 [after: 2.1, 3.1, 3.2, 3.3] 完整檢查：`tools/docker-test.sh`（不帶參數，含 fmt、clippy、全部測試）通過，且 `cargo test -p emfor-daemon` 中既有的 status、metrics、peer 測試沒有因新欄位或新行而被改動預期。驗證：docker-test.sh 回報全部通過。
