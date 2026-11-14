## 1. 自己檢查的頻率

- [x] 1.1 依決策「非負責主機每 3 次排定檢查至少自己檢查一次」與 requirement「Split checks」，把 `crates/emfor-daemon/src/schedule.rs` 的 `OWN_CHECK_EVERY` 改成 3，讓 `split()` 在連續 2 次投票決定後必須自己檢查。驗證：
  - `split` 的表格測試改成 spec Example「what this daemon does at one check time」的每一列（連續 1 次時照投票結果走，連續 2 次時自己檢查）；
  - `sparse_own_checks_while_the_region_is_up` 依 Scenario「Sparse own checks while the assigned checkers see the backend up」改成只在第 1、4、7、10 次自己檢查、`skipped()` 為 6；
  - `region_down_without_an_own_check` 依 Scenario「Region down without an own check」改成前 2 次由投票決定、第 3 次自己檢查，結果仍是 `peers 2/3: refused`，而且因為是自己的檢查讓它變成 `unhealthy`，會送出一則 `down` 通知；這個測試原本也涵蓋 Scenario「Moves decided by votes are not announced」，所以那部分改用 `fall = 2`（第 2 次投票決定時就變成 `unhealthy`）確認不送出通知；
  - 暫時把常數改回 5 時這三個測試都失敗。

## 2. 本機失敗時的快速重查

- [x] 2.1 依決策「由 quorum 的 Combined 告訴排程這是區域為 up 時的本機失敗」，在 `crates/emfor-daemon/src/quorum.rs` 的 `Combined` 加一個布林欄位，由 `Quorum::combine` 在「這台不是負責主機、自己的結果是 `down`、其他負責主機的新鮮票 `up` ≥ 2 且 `down` < 2」時設為 true，其他情況（`error`、`up`、負責主機、`u` < 2、`d` ≥ 2、沒有新鮮票）為 false。驗證：`quorum.rs` 的單元測試涵蓋 spec Example「whether a failed own check starts a fast recheck run」中與票和自己結果有關的每一列（負責主機那列為 false）；暫時拿掉 `up` ≥ 2 的條件、或拿掉「不是負責主機」的條件時測試失敗。（`down` < 2 不另外檢查：非負責主機最多 3 張票，`up` ≥ 2 已隱含 `down` ≤ 1，拿掉它不會改變任何結果。）
- [x] 2.2 [after: 1.1, 2.1] 依決策「本機失敗、區域為 up 時啟動快速重查」，讓 `schedule.rs` 的 `combine()` 把新欄位傳回 backend 檢查迴圈；迴圈在套用結果、更新 `fast_left` 之後，若欄位為 true 且 `starts_fast_run(health.state, fast_left > 0, last_run_end, now, &s)` 成立，就把 `fast_left` 設為 `fall`，並以 `info` 記錄 `backend <backend> of <service> fails here while its assigned checkers see it up (<reason>); checking now`。驗證：
  - `local_failure_while_the_region_is_up` 依 Scenario「Local failure while the region is up」改成第 4 次自己檢查失敗後，200 ms 與 400 ms 後各自己檢查一次，之後 backend 為 `unhealthy`、reason `local: connection refused`，且送出一則 `down` 通知；
  - 新增測試依 Scenario「Assigned checker does not start a run on its own failure」確認負責主機失敗後下一次檢查在 1 秒後；
  - 新增測試確認快速重查結束後不到 `fall × interval` 內，再次本機失敗時不啟動（Example 中 `1s ago` 那列）；這個測試用 `fall = 4`、`interval = 1s`，因為新頻率下一輪結束後下一次自己檢查最快在 3 個 `interval` 後，`fall = 3` 時一定已超過冷卻時間；
  - 新增測試 `local_failure_during_a_run_keeps_the_run` 確認收到 `down` 通知而進行中的快速重查，第一次檢查是本機失敗時不另起一輪，一輪仍在 `fall` 次後結束（Example 中「Fast run in progress」那列）；暫時忽略「進行中」條件時出現第 4 次快速檢查，測試失敗；
  - 暫時拿掉啟動快速重查的程式時，第一個測試失敗。

## 3. 文件

- [x] 3.1 [after: 2.2] 說明新的頻率與快速重查（英文與中文內容一致）：
  - `docs/manual/01-concepts.md` 與 `docs/manual.zh_TW/01-concepts.md` 的分工段落（每第三個 `interval`、本機失敗時每 200 ms 重查、公式 3 + (N − 3) / 3）與「Failure behavior at a glance」表格中一個 backend 那列；
  - `docs/manual/03-configuration.md` 與 `docs/manual.zh_TW/03-configuration.md` 的 `[peers]` 說明；
  - `docs/manual/08-operations.md` 與 `docs/manual.zh_TW/08-operations.md` 中 `emfor_peer_checks_skipped_total` 與延遲更新頻率的說法；
  - `docs/deploy/daemon.md` 的檢查量公式；
  - `docs/spectra/LANGUAGE.md` 的「投票決定」（連續 2 次後第 3 次自己檢查）。

  驗證：`grep` 這些檔案不再有 fifth、每五個、`(N − 3) / 5`、`÷ 5`、「第 5 次」等舊說法；人工比對英文與中文。
- [x] 3.2 [after: 3.1] 在 `CHANGELOG.md` 未發布區段的 Daemon 小節加入條目：非負責主機每 3 次自己檢查、本機失敗時快速重查、推算的最慢時間從約 14 秒變成約 6.4 秒（預設值）、檢查量公式的變化。驗證：人工檢查條目與 proposal 的 What Changes 相符。

## 4. 完整檢查

- [x] 4.1 [after: 1.1, 2.2, 3.2] 依 RELEASING.md，在 macOS 與 Debian 容器跑 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`RUSTFLAGS="--cfg fuzzing" cargo clippy -p emfor-check -p emfor-daemon --target-dir target/fuzzcheck -- -D warnings` 與 `cargo test --workspace --all-features`，並以 `-max_total_time=60` 跑 `peers` fuzz target。`crates/emfor-daemon/tests/peers.rs` 的 `split_checks` 若因頻率變高而斷言不成立，依新頻率調整斷言並記下實測次數。實測（8 秒內自己檢查的次數）：macOS 三次 `[8, 8, 3, 8]`、`[3, 8, 7, 8]`、`[8, 8, 3, 8]`，Debian 三次 `[8, 8, 3, 8]`、`[8, 8, 3, 7]`、`[8, 3, 7, 8]`，調整後 macOS 再三次 `[3, 7, 8, 7]`、`[8, 3, 7, 7]`、`[3, 7, 8, 8]`；非負責主機每次都是 3，所以斷言改成 `2 × 非負責主機 ≤ 負責主機`（3 對 6 仍成立），並把「只有它看得到的故障」的等待上限從 15 秒收緊到 6 秒（推算最慢約 3.4 秒）。驗證：所有指令 0 個錯誤、0 個失敗，`fuzz/artifacts/` 沒有新檔。
