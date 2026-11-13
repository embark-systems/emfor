## 1. store 不 fsync 與損毀重建

- [x] 1.1 實作「LMDB file layout」的 `[store] sync`（依「store 預設以 MDB_NOSYNC 開啟」）：crates/emfor-daemon/src/config.rs 的 `StoreConfig` 新增 `sync: bool`（預設 `false`，TOML 鍵 `sync`，非布林為設定錯誤），`Store::open` 在 `false` 時設 `EnvFlags::NO_SYNC`；`[store]` 變更需重啟的既有規則自然涵蓋 `sync`。驗證：store.rs 單元測試依 spec 範例表（缺省、`false`、`true`）檢查 `env.get_flags()` 是否含 `NO_SYNC`；config.rs 測試 `sync = "yes"` 為設定錯誤；`cargo test -p emfor-daemon --lib`。
- [x] 1.2 實作「commit 在 blocking 執行緒上執行」：crates/emfor-daemon/src/writer.rs 的 `Writer::flush` 經 `tokio::task::spawn_blocking` 呼叫 `Sink::commit`，取回 sink 與 ops；commit 失敗時的 pending 篩選、`count_commit_failure` 與 `clear_stale_readers` 行為不變。驗證：既有 writer 測試（含 `failed_commit_retries_latest_history`、`commit_failures_counted`、`reader_slots_checked_after_failed_commit`）全部通過；新增一個測試以 sink 在 commit 內檢查 `tokio::runtime::Handle::try_current()` 為錯誤（不在 async worker 上）。 [after: 1.1]
- [x] 1.3 實作「Damaged store is replaced at start」（依「損毀判定只看 LMDB 的三種錯誤碼，資料檔改名保留」）：`Store::open` 取得 writer lock 後開 env 並以一個讀交易走訪所有 key；遇到 `MdbError::Corrupted`、`PageNotFound`、`Invalid` 時記 warn（含路徑與錯誤名稱）、把資料檔改名為 `<path>.corrupt`、刪除 `<path>-lock`、重開一次；重開仍失敗或其他錯誤照原本處理。驗證：store.rs 單元測試把資料檔前兩個 page（2 × OS page size，Apple Silicon 為 16 KiB page）寫成 0 後 `Store::open` 成功、`.corrupt` 存在、新 store 可寫；crates/emfor-daemon/tests/daemon.rs 新增整合測試：損毀的 store 啟動後 log 含路徑、`.corrupt` 存在、心跳與服務記錄正常；既有 `second_daemon_on_same_store_exits_one` 確認不產生 `.corrupt`。 [after: 1.1]
- [x] 1.4 文件：examples/emfor.toml 的 `[store]` 加上 `sync` 的註解（預設 false、代價、損毀時自動改名重建、`hist/*` 會遺失）；docs/deploy/daemon.md 加一段說明為何預設不 fsync 與何時設 `sync = true`；CHANGELOG.md 的 Unreleased 記錄行為改變。驗證：`example_config_is_valid` 測試通過；內容審閱。 [after: 1.3]

## 2. reason 統一處理

- [x] 2.1 實作「Reasons are bounded and printable」（依「reason 在檢查結果離開檢查器時統一處理」）：crates/emfor-daemon/src/checker/mod.rs 新增 `normalize_reason` 與 `CheckResult::normalized`，crates/emfor-daemon/src/schedule.rs 在 `run_check` 之後、`health.apply` 之前套用，讓 store、`hist/*`、狀態轉換 log 與 metrics 都只看到處理後的字串。驗證：checker 單元測試涵蓋 spec 範例表六列（含 511 字元加 `…`、剛好 512 不截斷、U+0085 為 `\u{85}`、反斜線不變）；daemon.rs 整合測試以 `exec` 腳本輸出含 ESC 的第一行並 exit 1，store 的 reason 為 `bad\u{1b}[2Jstate`。

## 3. plugin 行長上限

- [x] 3.1 實作「Plugin output line limit」（依「plugin 行讀取改用有上限的讀法」）：crates/emfor-daemon/src/plugin/host.rs 新增 `read_line_limited`，取代 stdout（含握手）與 stderr 的 `BufReader::lines()`；stdout 超過 65536 位元組時記 warn `plugin <name> wrote a line longer than 65536 bytes` 並結束 router（走既有 `Event::StdoutClosed` 的停止與重啟）；stderr 超過時記錄前段、丟棄其餘直到換行。驗證：`read_line_limited` 單元測試（一般行、剛好 65536 位元組、65537 位元組、EOF 無換行）；crates/emfor-daemon/tests/support/test_plugin.rs 新增 `long_stdout` 與 `long_stderr` 兩個 mode，crates/emfor-daemon/tests/plugins.rs 依 spec 兩個情境驗證（warn log、pending 變 `plugin exited`、plugin 重新啟動；stderr 情境 plugin 不重啟且結果為 up）。

## 4. unknown 期間加快檢查

- [x] 4.1 實作「Check scheduling with jitter and no overlap」的新規則（依「unknown 期間的檢查間隔」）：crates/emfor-daemon/src/schedule.rs 新增 `fast_interval`，`first_delay` 改為 `[0, min(interval, 200ms))`，每次檢查後狀態為 `unknown` 且結果為 up 或 down 時等 `fast_interval`，否則 `next_wait`；crates/emfor-daemon/src/state.rs 的 `BackendHealth` 公開目前狀態。驗證：schedule 單元測試依 spec 範例表六列檢查選擇的等待時間（tokio 暫停時鐘或純函式）；daemon.rs 整合測試：`interval = "2s"`、`rise = 2`、`jitter = "0%"` 的健康 tcp backend 在啟動後 1 秒內為 healthy；既有排程與狀態機測試通過。 [after: 2.1]

## 5. 整體檢查

- [x] 5.1 本機檢查：macOS 與 Debian 映像 emfor-clients-test 的 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`（容器內若沒有 clippy / rustfmt 且需要下載，先徵得同意，否則只在 macOS 跑）、`cargo test --workspace --all-features`、`make -C clients test`。驗證：上述命令全部成功，回報中註明哪些檢查沒有在容器內跑。 [after: 1.2, 1.3, 1.4, 2.1, 3.1, 4.1]
