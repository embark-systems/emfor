## 1. 相依套件

- [x] 1.1 以 crossterm 處理 raw mode、按鍵與終端機大小：取得使用者同意下載後，在 `crates/emfor-daemon/Cargo.toml` 加入 `crossterm`（`event-stream` feature），`Cargo.lock` 隨之更新。以 `cargo tree -p emfor-daemon -i crossterm` 確認只由 emfor-daemon 引入，並以一個只依賴 crossterm（同版本、同 feature）的暫存 crate 執行 `cargo check --target x86_64-unknown-freebsd` 通過，證明在 FreeBSD 可編譯。

## 2. 畫面組成與捲動（純函式）

- [x] 2.1 標頭固定、服務與變化清單一起捲動：把 `text` 拆成 `header` 與 `services_lines`，`text` 回傳兩者相接，非 watch 輸出逐字不變。以現有 `status.rs` 單元測試（`one_healthy_and_one_failing_backend`、`colors_only_when_asked` 等）不改斷言仍通過驗證。
- [x] 2.2 [after: 2.1] 捲動位置以第一個顯示的內容行表示並每次夾住：新增 `Scroll`（`apply`、`clamp`）與 `compose(header, content, rows, scroll, style)`，內容放得下時不加頁尾、位置為 0；放不下時頁尾為 `-- lines <first>-<last> of <total>  PgUp/PgDn Up/Down Home/End  q quit --`，可捲動區為 `rows − 標頭 − 1`（至少 1）。以單元測試驗證 spec「Status watch mode」的「Output fits the terminal」、「Scrolling a long list」的 keys 表每一列、「Position kept across refreshes and limited when the list shrinks」（`22-42` → `10-30`），以及 `rows` 小於標頭加 2 時至少顯示一行內容。
- [x] 2.3 [after: 1.1] 按鍵對應：新增 `Action` 與 `key_action(&KeyEvent)`，依 design 的按鍵表對應 PageUp、PageDown、空白鍵、↑、↓、Home、End、`q`、`Q`、Ctrl-C；Release 事件與其他按鍵（例如 `x`、單獨的 `c`）回 `None`。以單元測試逐一驗證。

## 3. 終端機 I/O

- [x] 3.1 [after: 2.2, 2.3] 以 guard 與 panic hook 還原終端機，並關閉自動換行使每一行剛好佔一列：`TerminalGuard` 進入 raw mode、alternate screen、隱藏游標、`DisableLineWrap`，`Drop` 時反向還原；`--watch` 期間安裝會先還原終端機的 panic hook，結束後換回原 hook。`enable_raw_mode` 失敗時印 `cannot use the terminal: <錯誤>` 並以 2 結束；非終端機與事件錯誤照 design 處理（標準輸出不是終端機仍印 `--watch needs a terminal` 結束碼 2，由 `crates/emfor-daemon/tests/daemon.rs` 既有測試驗證）。
- [x] 3.2 [after: 3.1] 改寫 `watch` 迴圈實作 spec「Status watch mode」：`tokio::select!` 同時等待 tick、`EventStream`、SIGINT/SIGTERM/SIGHUP；tick 讀 store 並重算內容，按鍵與 Resize 以上一次的內容與新大小重畫、不呼叫 `Watch::update`；`Action::Quit` 與訊號結束迴圈後經 guard 還原並以 0 結束。以 `cargo build` 後手動在終端機執行驗證（見 4.2）。

## 4. 文件與驗證

- [x] 4.1 [after: 3.2] 更新 `docs/deploy/daemon.md`、`docs/manual/08-operations.md`、`docs/manual.zh_TW/08-operations.md`（`--watch` 的捲動按鍵、`q`/Ctrl-C 結束、固定標頭）、`CHANGELOG.md` Unreleased，以及 `docs/spectra/config.yaml` 中狀態指令「不用 TUI 套件」一句改為以 crossterm 處理按鍵與終端機、可捲動。以人工檢閱確認與 spec 一致。
- [x] 4.2 [after: 3.2] 手動驗證：以一份超過一屏的設定（例如 30 個服務）啟動 daemon，在終端機執行 `emfor status --watch`，確認 PageDown/PageUp/↑/↓/Home/End 捲動且標頭不動、`q` 與 Ctrl-C 結束、另開終端機 `kill` 結束後終端機回到 cooked mode（輸入有回顯）；把結果與無法驗證的部分記在 PR 說明。
- [x] 4.3 [after: 4.1, 4.2] 本機 gate 全數通過：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。
