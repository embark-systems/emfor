## Problem

兩個低嚴重度項目（2026-11-21 的 Fable 審查，程式碼位置已讀過確認）：

1. **reload 讀設定檔會擋住主迴圈。** `SIGHUP` 時 `crates/emfor-daemon/src/cli.rs` 的 `load` 在第一個 await 之前，同步呼叫 `config::read_layers_with`（std::fs：`read_trusted`、`list_fragments`）。設定檔或 include 目錄放在掛住的檔案系統上時，讀取不返回，跑 `run` 主迴圈的執行緒被卡住：supervisor 的 watchdog、writer 的訊息不再處理，`SIGTERM`/`SIGINT` 也要等讀取返回才處理。peer 任務跑在別的 worker 上，不受影響（runtime 是多執行緒的 `Runtime::new()`）。
2. **plugin 重啟時，舊行程的任務沒有結束。** `crates/emfor-daemon/src/plugin/host.rs` 的 `supervise` 在重啟時以新的 `Process` 覆蓋舊的：router 任務的 `JoinHandle` 被丟掉（沒有 abort），stderr 任務（`log_stderr`）和 stdin 寫入任務在 `launch` 裡 detach、沒有留 handle。它們靠 EOF 結束；`kill_group` 殺掉行程群組後 EOF 就會到。但 plugin 的子孫行程若用 `setsid` 脫離群組並握著 pipe，EOF 永遠不來，每次重啟都留下一個任務、一個 fd 和一個 `Arc`。

## Root Cause

1. `load` 把阻塞的檔案讀取直接放在 async 函式裡：呼叫端的 `tokio::select!` 只能在 await 點取消，擋不了阻塞中的呼叫。`config::parse_layers` 已經用 `spawn_blocking`，讀檔是漏掉的那一段。
2. 任務的壽命只由 pipe 的 EOF 決定，沒有由 supervisor 擁有與回收：`Process` 沒有持有 stderr 與寫入任務的 handle，router 的 handle 在覆蓋時只是被丟掉。

## Proposed Solution

1. 把 `read_layers_with` 移進 `spawn_blocking`（路徑複製進去，結果與警告帶回來），`load` 在這一步 await 它。這樣 reload 與 `SIGTERM`/`SIGINT` 的 `select!` 才真的能競爭；停止時 reload 被放棄的行為（日誌一行、結束）維持不變，卡住的 blocking 執行緒由既有的 `runtime.shutdown_timeout(1s)` 放棄。同一時間只會有一個 reload 在讀，因為 reload 只在主迴圈的 `hup` 分支裡進行；reload 進行中又來的 `SIGHUP` 由 signal 串流合併成一個，在這次 reload 結束後再做一次。
2. `Process` 持有 router、stderr、stdin 寫入三個任務的 handle，放在一個被丟掉時就 abort 全部的守衛裡。supervisor 在殺掉行程群組、回收 child 之後，先給 stderr 任務與 router 任務 `EXIT_DRAIN`（100 ms，router 現在已有的上限）讀完管線裡剩下的行，再讓守衛 abort。不提早 abort stderr，所以 plugin 崩潰前最後幾行仍會記進日誌。`launch` 在握手失敗的提早返回路徑上也由同一個守衛回收。這是根因修法：任務壽命由 supervisor 擁有，不再只靠對方關 pipe。

## Non-Goals (optional)

- 不處理對等節點的結果亂序覆蓋（另開一個 change，在 `quorum.rs`）。
- 不改 plugin 與 DNS 的其他任務、啟動時讀 secret 的路徑、RADIUS 密碼檔、exec 與 plugin 的 `env_clear`。
- 不讓 reload 可以被第二個 `SIGHUP` 取消，也不改 reload 的驗證順序。

## Success Criteria

- 設定檔路徑換成沒有寫入端的 FIFO 後送 `SIGHUP`，daemon 在送出 `SIGTERM` 後的數秒內結束，且日誌有「stop requested while reloading」一行。
- reload 的結果（成功套用、驗證失敗保留舊設定）與修改前相同；既有 reload 測試不改斷言地通過。
- 重啟一個其 stderr 被 `setsid` 的子孫行程握住的 plugin 後，舊行程的 router、stderr、stdin 寫入三個任務在 `EXIT_DRAIN` 之後結束；plugin 在退出前寫的 stderr 行仍出現在日誌。

## Impact

- Affected specs: service-config, checker-plugins
- Affected code:
  - Modified: `crates/emfor-daemon/src/cli.rs`、`crates/emfor-daemon/src/plugin/host.rs`、`crates/emfor-daemon/tests/daemon.rs`、`crates/emfor-daemon/tests/plugin_host.rs`、`crates/emfor-daemon/tests/support/test_plugin.rs`、`CHANGELOG.md`
  - New: (none)
  - Removed: (none)
