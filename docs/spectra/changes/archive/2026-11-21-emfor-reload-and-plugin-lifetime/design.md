## Context

兩個互不相干的小問題，共同點是「阻塞或任務壽命沒有由擁有者控制」，所以放在一個 change。

**reload。** `cli.rs` 的主迴圈在 `hup` 分支把 `load(...)` 與 `term`/`int` 放進 `tokio::select!`，用意是 reload 等 plugin 啟動、握手、`validate` 要數秒，停止要求可以中斷它。但 `load` 在第一個 await 之前就同步呼叫 `config::read_layers_with`，阻塞呼叫不能被 `select!` 取消。目前 `parse_layers` 已在 `spawn_blocking` 裡；讀檔是漏掉的那段。`main` 用 `Runtime::new()`（多執行緒），並在結束時 `runtime.shutdown_timeout(Duration::from_secs(1))`，所以卡住的 blocking 執行緒不會讓行程無法結束。

**plugin 重啟。** `host.rs` 的 `launch` 啟動行程，並 spawn 三個任務：stderr 記錄（`log_stderr`）、stdin 寫入、握手後的 router（讀 stdout）。`Process` 只持有 router 的 handle。`supervise` 在重啟時以新的 `Process` 覆蓋舊的，被丟掉的 `JoinHandle` 不會 abort 任務。三個任務靠 EOF 結束：`kill_group` 殺掉行程群組，pipe 的寫入端隨之關閉。用 `setsid` 脫離群組並握著 pipe 的子孫行程不會被殺，EOF 不來。

## Goals / Non-Goals

**Goals:**

- reload 時阻塞的設定檔讀取不再擋住 `SIGTERM`/`SIGINT` 與 supervisor 的迴圈。
- plugin 行程被回收後，與它相關的三個任務在有限時間內結束，不論 pipe 是否仍被別的行程握著。
- plugin 退出前最後寫的 stderr 行仍會被記錄。

**Non-Goals:**

- 對等節點的結果亂序覆蓋（另開 change）。
- 啟動時（`current = None`）的讀檔；那時沒有迴圈要保護，仍可阻塞。
- plugin 與 DNS 其他任務、RADIUS 密碼檔、exec 與 plugin 的 `env_clear`。
- 讓第二個 `SIGHUP` 取消進行中的 reload。

## Decisions

### reload 的讀檔放進 spawn_blocking

`load` 把 `read_layers_with` 與它的 `warnings` 放進 `tokio::task::spawn_blocking`（路徑複製進閉包，回傳 `(Result<Layers, Vec<ConfigError>>, Vec<String>)`），await 它；`JoinError`（blocking 任務 panic）轉成與 `parse_layers` 相同形狀的 `ConfigError`，訊息 `config read failed: {e}`。啟動時（`current = None`）也走同一個函式，行為相同，不另開分支：多一次執行緒切換，沒有觀察得到的差別。

替代方案：在 `hup` 分支外面包 `tokio::time::timeout`。不採用：超時無法取消阻塞中的執行緒，而且會把一次慢但最終成功的讀取當成失敗。

### 同一時間只有一個 reload，額外的 SIGHUP 合併

reload 只在主迴圈的 `hup` 分支裡 await，所以同一時間只有一個讀取；不需要像 `SecretReader` 那樣另外記一個「進行中的讀取」。tokio 的 signal 串流把等待期間到達的多個信號合併成一個：reload 進行中 `hup.recv()` 沒有被 poll，結束後下一圈的 `recv()` 立刻返回一次，所以多個 `SIGHUP` 之後恰好再 reload 一次。這是既有行為，spec 補上並用測試固定。

停止要求在 reload 中到達：`select!` 取走 `term`/`int`，reload 的 future 被丟掉，日誌一行（既有文字），主迴圈 `break`。那個 `spawn_blocking` 的執行緒若還卡在讀取，由 `runtime.shutdown_timeout(1s)` 放棄；行程結束。

### plugin 任務的壽命由 Process 擁有

新增一個小結構（暫名 `Tasks`），持有 stderr 記錄、stdin 寫入、router 三個 `JoinHandle`，`Drop` 時對三個都 `abort()`。`Process` 持有它取代單獨的 `router` 欄位；`supervise` 現在 `select!` 裡等的 `&mut process.router` 改為 `&mut process.tasks.router`。`launch` 在 spawn 之後、回傳 `Process` 之前的錯誤返回路徑（握手失敗等）由同一個結構的 `Drop` 回收，不另寫清理。

`abort()` 取消任務時，任務擁有的 `ChildStderr`、`ChildStdout`、`ChildStdin` 隨 future 一起被丟掉，read/write 端的 fd 被關閉；不需另外關。

### abort 前先給 stderr 與 router EXIT_DRAIN

行程被回收之後（`Stalled`、`StdoutClosed`、`Exited` 三個分支結尾，`lock(&process.conn.pending).close()` 之前或之後皆可），`supervise` 先 `timeout(EXIT_DRAIN, ...)` 等 stderr 任務與 router 任務自己結束（正常情況下 `kill_group` 之後 EOF 馬上到，不會等滿），再讓 `Tasks` 被丟掉而 abort。`EXIT_DRAIN`（100 ms）沿用 router 現在已有的上限；`Exited` 分支原本只等 router，改為同時等兩者，總等待仍是這一個上限（用 `tokio::join!` 包在同一個 `timeout` 裡）。這樣 plugin 退出前最後幾行 stderr 仍會記進日誌，而不被 abort 吃掉。

`Event::Stop` 分支：`finish` 等完 plugin 退出後 `return`，`Process` 在返回時被丟掉，三個任務被 abort；先 `drop(process.conn)`（既有）讓寫入任務把佇列寫完。這條路徑的 stderr 最後幾行也要保留：`finish` 之後同樣給 `EXIT_DRAIN`。

替代方案：立刻 abort stderr 任務。不採用：丟掉崩潰前最後的輸出，那往往是診斷最需要的。另一個替代：給三個任務共用一個取消權杖（`CancellationToken`）。不採用：需要新增依賴或自寫，`abort()` 加 `Drop` 已足夠，而這些任務的 await 點都是可取消的 I/O。

## Implementation Contract

- **行為（reload）**：`SIGHUP` 之後設定檔讀取卡住時，`SIGTERM`/`SIGINT` 仍在數秒內讓 daemon 結束（日誌 `stop requested while reloading; the reload is abandoned`）；讀取不卡時，reload 的結果、日誌與錯誤文字與修改前相同。
- **行為（plugin）**：重啟或停止 plugin 時，舊行程的 stderr 記錄、stdin 寫入、router 三個任務在 `EXIT_DRAIN`（100 ms）之內自行結束，否則被 abort；plugin 在退出前寫進管線的 stderr 行仍會被記錄。
- **介面**：`cli.rs::load` 簽章不變；`host.rs` 內部的 `Process` 欄位改變，對外的 `PluginHost` API 不變。
- **失敗模式**：讀檔的 blocking 任務 panic → 該次 reload 以一個 `ConfigError`（路徑 `""`，訊息 `config read failed: …`）被拒絕，舊設定續跑。abort 一個已結束的任務沒有作用。
- **驗收**：(a) `tests/daemon.rs` 的新測試：設定檔路徑換成沒有寫入端的 FIFO，送 `SIGHUP`、再送 `SIGTERM`，行程在 5 秒內結束且日誌含 `stop requested while reloading`；若 FIFO 在這個測試框架裡造不出（先讀測試輔助函式再決定），改為把讀取包成可注入的函式並以通道阻塞，並在回報裡說明。(b) 三個 `SIGHUP` 合併為一次額外 reload 的測試（既有測試框架若無法數 reload 次數就以日誌行計數）。(c) `tests/plugin_host.rs` 的新測試：測試 plugin 新增一個環境變數旋鈕，讓它啟動一個 `setsid` 的背景行程並握住 stderr；重啟後以 `tokio::runtime::Handle::metrics().num_alive_tasks()`（先查 tokio 版本是否已穩定提供，否則改以 `/proc/self/fd` 的 fd 數）驗證舊任務結束、重啟多次後數量不增長。(d) 單元測試：行程退出前寫的 stderr 行仍被記錄。(e) 既有的 reload 與 plugin 測試不改斷言地通過；`tools/docker-test.sh` 全過。
- **範圍**：只動 `cli.rs`、`host.rs`、測試與測試 plugin、`CHANGELOG.md`；範圍外的項目見 Non-Goals。

## Risks / Trade-offs

- 啟動時的讀檔也多一次 `spawn_blocking`：延遲微乎其微，沒有量測；沒有行為差別。
- abort 在任務 await 點之間才生效；這些任務只在 I/O await 點之間做同步工作（日誌、解析一行），所以取消延遲為微秒等級（估計，未量測；測試以「1 秒內」為界）。
- 卡住的 blocking 執行緒在 daemon 還在運行時不會被回收（無法取消）。因為同一時間只有一個 reload 且卡住時主迴圈等的是 `select!`（可被停止要求打斷），累積上限是一個：卡住的 reload 只會因停止要求而被放棄，放棄之後行程就結束，不會再有下一次 reload。
- 未驗證：真實掛住的 NFS；FreeBSD 與 macOS 上的 `setsid` 行為與 FIFO 開啟（測試只在 Debian 容器跑）。
