## 1. 投票判斷

- [x] 1.1 依「程式結構」，quorum.rs 新增 `Quorum::votes`（回傳本機是否為負責主機、是否有負責主機、其他負責主機的新鮮票數、down 票數、up 票數、排名最高的 down 原因）並與 `combine` 共用取票邏輯；新增 `Quorum::skip` / `skipped` 累計略過次數。行為：票的新鮮、摘要、存活條件與 peer-group 規格「Region verdict and combined result」相同。驗證：新單元測試對 peer-group「Split checks」範例中的負責主機與票數（backend `10.0.0.12:8080`、本機 `10.0.0.5:5302`、負責主機 .9 / .6 / .7）逐列確認；`cargo test -p emfor-daemon --lib quorum` 既有測試全部通過。

## 2. 後端排程

- [x] 2.1 schedule.rs 新增純函式 `split`，依「非負責主機沿用原本的檢查方式，間隔 interval × 5」「投票決定的條件：至少 2 票同向」「本機結果不是 up 時改回每次自己檢查」三個決策，回傳自己檢查或投票決定的結果。驗證：單元測試 `split_check_time_table` 照 peer-group 規格「what this daemon does at one check time」範例逐列（含快節奏連查中一定自己檢查）。[after: 1.1]
- [x] 2.2 後端 task 在有 `PeerHooks` 時每次排定時間先呼叫 `split`：投票決定時不呼叫 checker，結果進狀態機、store、history、log 與檢查計數，但不量延遲、不送 `result`、不送 `down`、不改不一致，並呼叫 `Quorum::skip`；依「投票決定的那一次與一般檢查的差別」實作，滿足 peer-group 規格「Split checks」與「Backend failure notification」、health-checking「Check scheduling with jitter and no overlap」。驗證：暫停時間的單元測試重現「Sparse own checks while the assigned checkers see the backend up」（10 次中 checker 只在第 1、6 次被呼叫）、「Local failure while the region is up」、「Region down without an own check」、「Moves decided by votes are not announced」、「Check times decided by votes keep the schedule」；暫時讓 `split` 永遠回傳自己檢查時，第一個測試失敗。[after: 2.1]

## 3. 指標

- [x] 3.1 peers.rs 每秒 tick 把 `quorum.skipped()` 存進 `PeerStats`，metrics.rs 在有 `[peers]` 時輸出 `emfor_peer_checks_skipped_total`（counter，起始 0），滿足 metrics-endpoint「Daemon metrics」。驗證：metrics.rs 的 `peer_metrics` 測試確認此序列與數值，沒有 `[peers]` 時沒有任何 `emfor_peer_` 序列的既有測試仍通過。[after: 1.1]

## 4. 整合測試與文件

- [x] 4.1 crates/emfor-daemon/tests/peers.rs 新增四個 daemon 的分工測試：同一個 exec 檢查每次執行時把一行寫進各 daemon 環境變數指定的計數檔。數秒後非負責的那台檢查次數少於任一負責主機的一半，`/metrics` 的 `emfor_peer_checks_skipped_total` 大於 0；停掉一台負責主機後，原本非負責的那台恢復每次檢查。另一情境：非負責主機的旗標檔讓它自己的檢查失敗時，它以 `local: ` 開頭的原因判 unhealthy，其他三台維持 healthy。驗證：`cargo test -p emfor-daemon --test peers` 通過；暫時讓 `split` 永遠回傳自己檢查時，檢查次數的斷言失敗。[after: 2.2, 3.1]
- [x] 4.2 文件說明第三階段：docs/manual/01-concepts.md 聯防群組一節加入非負責主機的分工、`interval × 5` 本機檢查與「全區 up 但本機連不上」最晚多久發現；docs/manual/08-operations.md 加 `emfor_peer_checks_skipped_total` 與延遲數值的說明；docs/manual/03-configuration.md 註明分工不需新設定；docs/deploy/daemon.md 指標表；CHANGELOG.md；docs/spectra/config.yaml 專案說明改為三階段都已完成；docs/spectra/LANGUAGE.md 加「投票決定」。驗證：人工審閱文件與規格一致，`emfor check-config --config examples/emfor.toml` 仍通過。[after: 4.1]
- [x] 4.3 完整檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過，`git diff Cargo.lock` 沒有變更。驗證：三個指令結束碼為 0 並記錄測試套件數。[after: 4.2]

## 5. 驗證與審查後的修正

- [x] 5.1 規格與設計對齊：peer-group「Split checks」加上「投票會判 up 但後端沒有延遲值時自己檢查」的條件、範例表加「Latency」欄與兩列、新情境「Recovery decided by votes waits for a latency」；「Local failure while the region is up」的原因改為 `connection refused`（與測試一致）；design.md 新增決策「沒有延遲值時不靠投票判 up」並把 `split` 的簽名改成實際的 `last_own_up: Option<bool>` 與 `has_latency`。驗證：`spectra analyze emfor-peers-split-checks` 沒有 Critical 或 Warning，`spectra validate emfor-peers-split-checks` 通過。
- [x] 5.2 schedule.rs 的 `split` 依「沒有延遲值時不靠投票判 up」在沒有延遲值時不回傳投票決定的 `up`；`split_check_time_table` 照新範例表逐列；新測試 `recovery_by_votes_waits_for_a_latency`（投票讓後端轉 unhealthy 後票回到 up，下一次自己檢查且回到 healthy 時有延遲值）與 `no_assigned_checkers_check_every_time`（存活成員剩 2 台時每次排定時間都自己檢查）；範例表「沒有票、本機 down」列的套用結果由 quorum.rs 的 `combined_result_table` 涵蓋，在測試註解註明。驗證：`cargo test -p emfor-daemon --lib schedule` 通過，暫時拿掉延遲條件時 `recovery_by_votes_waits_for_a_latency` 失敗。[after: 5.1]
- [x] 5.3 完整檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過，Cargo.lock 沒有變更。驗證：三個指令結束碼為 0 並記錄測試套件數。[after: 5.2]
