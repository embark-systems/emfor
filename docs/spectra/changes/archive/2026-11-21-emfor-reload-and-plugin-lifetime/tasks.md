## 1. reload 的讀檔

- [x] 1.1 實作設計決定「reload 的讀檔放進 spawn_blocking」：`crates/emfor-daemon/src/cli.rs` 的 `load` 把 `config::read_layers_with` 與 `warnings` 放進 `spawn_blocking` 再 await；blocking 任務 panic 時回一個 `ConfigError`（路徑 `""`，訊息 `config read failed: …`）。完成時：`load` 的簽章與其他行為不變；既有 reload 與 `check-config` 測試不改斷言地通過；`cargo clippy -D warnings` 乾淨。
- [x] 1.2 [after: 1.1] 實作設計決定「同一時間只有一個 reload，額外的 SIGHUP 合併」與 spec service-config "Reload on SIGHUP applies only a fully valid config" 的兩個新情境：`crates/emfor-daemon/tests/daemon.rs` 新增兩個測試。先讀該檔既有的 `SIGHUP` 測試（`d.signal`）與設定檔寫入輔助函式再決定寫法，不發明新框架。(a) 設定檔路徑換成沒有寫入端的 FIFO（`nix::unistd::mkfifo`），送 `SIGHUP` 再送 `SIGTERM`，行程在 5 秒內結束，日誌含 `stop requested while reloading`；若這個框架造不出 FIFO 的情況，改為把讀取包成可注入的函式並以通道阻塞，並在回報裡說明。(b) reload 進行中連送三個 `SIGHUP`，之後恰好多一次 reload（以日誌行計數；reload 進行中以 plugin 的 `validate` 延遲或同類既有旋鈕撐住時間，先查測試 plugin 有哪些旋鈕）。完成時：兩個測試在 Debian 容器通過；說明新測試未對舊程式跑過（舊程式下 (a) 會超時）若未實際驗證。

## 2. plugin 任務的壽命

- [x] 2.1 實作設計決定「plugin 任務的壽命由 Process 擁有」與「abort 前先給 stderr 與 router EXIT_DRAIN」：`crates/emfor-daemon/src/plugin/host.rs` 新增持有 stderr 記錄、stdin 寫入、router 三個 `JoinHandle` 的結構，`Drop` 時全部 `abort()`，取代 `Process.router`；`supervise` 的 `Stalled`、`StdoutClosed`、`Exited` 三個分支在行程回收後以同一個 `EXIT_DRAIN` 上限等 stderr 與 router 任務自行結束，`Stop` 分支在 `finish` 之後同樣等一次；`launch` 在握手失敗的提早返回由該結構的 `Drop` 回收。完成時：`PluginHost` 的對外 API 不變；既有 `tests/plugin_host.rs` 與 `tests/plugins.rs` 不改斷言地通過；clippy 乾淨。
- [x] 2.2 [after: 2.1] 實作 spec checker-plugins "Plugins on reload" 的兩個新情境：`crates/emfor-daemon/tests/support/test_plugin.rs` 新增一個環境變數旋鈕，讓測試 plugin 在握手前啟動一個 `setsid` 的背景行程（繼承 stderr，例如 `setsid sleep 60`）；`crates/emfor-daemon/tests/plugin_host.rs` 新增測試：(a) 該 plugin 被殺掉並重啟數次之後，`tokio::runtime::Handle::current().metrics().num_alive_tasks()`（tokio 1.54 已穩定提供）在 1 秒內回到重啟前的基線附近、且不隨重啟次數增長；(b) plugin 寫 `fatal: out of memory` 到 stderr 後立刻退出，日誌含 `plugin p: fatal: out of memory`（`tracing_test` 在整合測試裡只收測試 crate 的事件，先確認；不行就把這個情境放在 `host.rs` 的單元測試用 `tokio::io::duplex` 與 `log_stderr` 加一個假的行程結束順序驗證，並說明）。測試結束時殺掉背景行程，不留孤兒。完成時：測試在 Debian 容器通過。

## 3. 文件與驗證

- [x] 3.1 [after: 1.2, 2.2] `CHANGELOG.md` Unreleased 加條目：reload 讀設定檔不再擋住停止要求；plugin 重啟時舊行程的任務在 100 ms 內回收；Checked（Debian 容器 fmt、clippy `-D warnings`、測試數字）與 Not verified（真實掛住的 NFS；FreeBSD 與 macOS；新測試未對舊程式跑過，若屬實）。`docs/manual/08-operations.md` 已說明「reload 載入中收到停止要求就放棄」，先讀確認不需改動；`python3 tools/check-manual-links.py` 通過。完成時：`tools/docker-test.sh` 全過，`spectra analyze` 與 `spectra validate` 乾淨。
