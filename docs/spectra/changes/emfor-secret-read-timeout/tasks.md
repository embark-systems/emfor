## 1. SecretReader

- [x] 1.1 實作 spec peer-group 的 "Peer secret reload"（設計決定「迴圈不等讀取，狀態放在迴圈裡的 SecretReader」）：在 `crates/emfor-daemon/src/peers/task.rs` 新增 `SecretReader`（欄位：進行中的讀取 `Option<(JoinHandle<Result<Vec<u8>, String>>, Instant)>`、讀取函式 `fn(&Path) -> Result<Vec<u8>, String>`）與常數 `SECRET_READ_WAIT`（200 毫秒）、`SECRET_READ_SLOW`（5 秒）。完成時：有一個方法接受目前的路徑、`Group`、`warned`、`now`，依 design 的規則行事（無進行中讀取：啟動並最多等 200 毫秒；已有進行中讀取：已結束就套用並啟動新的，未結束就不等，超過 5 秒且距上次警告已滿 `WARN_EVERY` 就記 `warn`），結果一律交給既有的 `apply_secret`；`JoinHandle` 回傳錯誤（讀取函式 panic 或執行緒被取消）時走 `apply_secret` 的錯誤路徑（設計決定「讀取函式 panic 或執行緒被取消」）。每次啟動讀取都用迴圈目前持有的路徑（設計決定「換路徑：舊讀取結束後才讀新路徑」）。
- [x] 1.2 [after: 1.1] `run` 迴圈的 `tick` 分支與 `settings.changed()` 分支改用 `SecretReader`（迴圈持有一個實例並每次傳入目前的 `secret_file`），移除 `refresh_secret`。`start_on` 維持既有簽章，委派給多一個讀取函式參數的內部入口（正式程式碼傳 `read_secret`）。完成時：`cargo build` 無警告，既有 `apply_secret` 測試與 `heartbeats_liveness_and_reload` 不改就通過。

## 2. 測試

- [x] 2.1 [after: 1.1] `task.rs` 的測試模組新增 `SecretReader` 單元測試，用 `#[tokio::test(start_paused = true)]` 與 `tracing_test::traced_test`（鄰近的 `full_queue_drops_and_counts` 的做法），注入一個阻塞在 channel 上的讀取函式：(a) 阻塞期間許多個 tick 後，函式只被呼叫一次；(b) 阻塞 5 秒後出現一行含 `has not finished` 與 `keeping the last valid secret` 的警告，一分鐘內不重複，之後滿一分鐘再出現一行；(c) 函式回傳新的有效 secret 後，`group.keys.current()` 變成它；(d) 立即回傳的函式在同一次呼叫內套用（Spec 情境 "Normal read applied at once"）；(e) 注入的函式 panic 時，結果走錯誤路徑並保留最後有效的 secret，之後的 tick 可以再啟動新讀取；(f) 讀取進行中換路徑：不啟動新讀取，舊讀取結束後的下一次啟動讀新路徑；(g) 3 秒後才回傳的函式，在回傳前不警告，回傳後套用（情境 "Slow read of the secret file"）。
- [x] 2.2 [after: 1.2] 迴圈層級測試（既有 `pair` 是真實時間，沿用它）：讓 `pair` 的 A 用注入的、永遠阻塞的讀取函式啟動（`pair` 增加一個讀取函式參數，既有呼叫傳 `read_secret`），在 2 秒內斷言 B 在 A 看起來存活（`ha.stats.up() == vec![(b, true)]`，A 仍在收 B 的 heartbeat 並處理）、B 看 A 存活，且函式只被呼叫一次（情境 "Read of the secret file hangs" 的前半；5 秒後的警告由 2.1 在暫停時間下檢查，不在真實時間等 5 秒）。完成後釋放阻塞的函式，避免測試結束後留下卡住的執行緒。

## 3. 文件

- [x] 3.1 `docs/manual/03-configuration.md` 與 `docs/manual.zh_TW/03-configuration.md` 中 "The secret file is read again every second." 那句後加一句：讀取慢或卡住不會讓 heartbeat 停止，卡住期間沿用最後有效的 secret，超過 5 秒會有一行警告（zh_TW 對應譯文）。完成時：`python3 tools/check-manual-links.py` 報 0 個問題。
- [x] 3.2 `CHANGELOG.md` 的 Unreleased 新增 Fixed 條目：peer 任務讀 `secret_file` 卡住不再讓群組停擺；內含 Checked（Debian 容器內 fmt、clippy、全部測試結果與新測試名稱）與 Not verified（沒有在真實卡住的 NFS 上試過；200 毫秒與 5 秒未用真實環境量測）。條目寫的測試數字以實際執行結果為準。

## 4. 檢查

- [x] 4.1 [after: 1.2, 2.1, 2.2, 3.1, 3.2] 在 Debian 容器（`tools/docker-test.sh`）跑 `cargo fmt --check`、`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`、`cargo test --locked --workspace --all-features`，全部通過，並把結果寫進 3.2 的條目。

## 5. 檔案監看（需求變更：使用者決定能監看就監看，不能才每 tick 讀檔；不考慮 NFS）

- [x] 5.1 `crates/emfor-daemon/Cargo.toml` 的 `nix` 加 features `inotify`、`event`，新增 `crates/emfor-daemon/src/peers/watch.rs` 的 `SecretWatch`（設計決定「用 nix 的 inotify 與 kqueue 監看，不加新套件」與「監看放在專用執行緒，以 Notify 通知迴圈」）。完成時：Linux 上對一個路徑建立監看，路徑所在目錄內原地改寫與 rename 換檔都使 `Notify` 被觸發；建立失敗回傳錯誤；丟棄後執行緒在 1 秒內結束；FreeBSD/macOS 以 kqueue 實作並能編譯（`cargo check --target` 不可得時，在 Debian 容器內只編譯 Linux，kqueue 程式碼以 `#[cfg]` 隔開，編譯結果在實機確認）。
- [x] 5.2 [after: 5.1] `run` 迴圈接上 `SecretWatch`（設計決定「監看什麼」與「讀檔的時機」）：啟動時先建立監看再讀檔；新增 select 分支等 `Notify` 後呼叫 `SecretReader::refresh`；能監看時 tick 分支只在距上次讀取滿 `SECRET_SAFETY_READ`（30 秒）時讀；不能監看時每個 tick 讀並在第一次退回時記一行 `info`；`secret_file` 換路徑時重建監看，失敗就退回輪詢。完成時：既有測試不改就通過。
- [x] 5.3 [after: 5.2] 測試（真實時間，Debian 容器）：(a) rename 換檔後，新 secret 在 1 秒內被套用，不等 30 秒；(b) 原地改寫同理；(c) 注入讓 `SecretWatch` 建立失敗的路徑（例如對不存在的目錄），退回每 tick 讀並有一行 `info`；(d) 換路徑後監看新路徑。kqueue 不在容器測，標為未驗證。
- [x] 5.4 [after: 5.2] 更新手冊 `docs/manual/03-configuration.md` 與 `docs/manual.zh_TW/03-configuration.md`（"The secret file is read again every second" 那句改為：能監看時變更立即被看到並每 30 秒保險讀一次，否則每秒讀一次；既有的慢讀取一句保留）與 CHANGELOG 條目（寫出監看、退回與 kqueue 未驗證；數字以實際結果為準）。
- [x] 5.5 [after: 5.3, 5.4] 在 Debian 容器跑 fmt、clippy `-D warnings`、全部測試，結果寫進 CHANGELOG；macOS 與 FreeBSD 的 kqueue 手動驗證列為發佈前待辦，不在這個 change 內執行。
