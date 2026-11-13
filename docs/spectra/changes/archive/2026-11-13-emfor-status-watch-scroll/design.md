## Context

`emfor status --watch`（`crates/emfor-daemon/src/status.rs` 的 `watch`）目前在 tokio current-thread runtime 裡每秒讀一次 store，把 `text` 產生的行加上 "recent changes:" 一段，以 `\x1b[H` 從左上角整份重畫到 alternate screen，結尾 `\x1b[J` 清掉剩下的部分。它不讀 stdin、不進 raw mode、不知道終端機大小；超過畫面的行直接被推出畫面外。結束只靠 tokio signal 的 SIGINT / SIGTERM / SIGHUP，結束時寫 `\x1b[?25h\x1b[?1049l` 還原。

emfor-status 的設計曾決定「watch 用 ANSI 與 tokio 處理中斷」、不引入 crossterm / ratatui，並把互動式捲動列為 Non-Goal。服務數量增加後清單超過一屏，使用者決定加入捲動並以 crossterm 處理按鍵與終端機（不用 libc termios 自己寫）。

限制：emfor 支援 Linux 與 FreeBSD；macOS 只作開發用。正式部署平台沒有本機 FreeBSD 環境，FreeBSD 只能以 `cargo check --target x86_64-unknown-freebsd` 交叉檢查。

## Goals / Non-Goals

**Goals:**

- 清單超過終端機高度時，以 PageUp / PageDown、上下方向鍵、Home / End 捲動。
- 每秒重畫保持捲動位置；清單變短或終端機變大時夾回有效範圍。
- 在 raw mode 下以 Ctrl-C 或 `q` 結束；訊號照舊結束。
- 任何結束路徑（按鍵、訊號、panic、讀不到 store）都把終端機還原。
- 畫面組成（哪幾行顯示、頁尾內容）與按鍵對應是純函式，可用單元測試涵蓋。

**Non-Goals:**

- 不做水平捲動、搜尋、篩選、展開收合或滑鼠捲動。
- 不引入 ratatui 或其他 TUI 框架；只用 crossterm 的終端機控制與事件。
- 非 `--watch` 的輸出（文字、`--json`、`--history`）與它們的跳脫字元規則不變。
- 不改 "recent changes:" 的內容與筆數（最近 10 筆）。

## Decisions

### 以 crossterm 處理 raw mode、按鍵與終端機大小

**Supersedes**: emfor-status / watch 用 ANSI 與 tokio 處理中斷

加入 `crossterm`（啟用 `event-stream` feature）。`--watch` 啟動時 `terminal::enable_raw_mode`，以 `EnterAlternateScreen`、`cursor::Hide`、`DisableLineWrap` 準備畫面；`terminal::size()` 取得行數，失敗時當作 24 行。事件以 `crossterm::event::EventStream`（實作 `futures::Stream`）在既有的 `tokio::select!` 中與每秒 tick、三個訊號一起等待。畫面輸出仍是一次寫出整個 frame 字串（`MoveTo(0,0)` 後逐行寫入、每行結尾清到行尾、最後清到畫面底），避免閃爍。

- 理由：使用者已選定 crossterm；它處理 termios、`/dev/tty`、SIGWINCH 與按鍵解碼，Linux / FreeBSD / macOS 共用 unix 實作。`EventStream` 直接接進 tokio，不需另開執行緒。
- 替代方案：libc termios 自己寫並自行解碼 escape sequence —— 使用者否決。在 `spawn_blocking` 裡呼叫 `event::read` —— 結束時阻塞的讀取無法取消，捨棄。

### 關閉自動換行使每一行剛好佔一列

在 alternate screen 中送 `DisableLineWrap`（DECAWM 關閉），超過寬度的行由終端機截掉，不會折成兩列。因此「一行文字 = 一列畫面」，捲動計算只需要行數，不需要知道字元寬度或解析行內的色彩跳脫字元。結束時送 `EnableLineWrap`。

- 替代方案：依終端機寬度自行截斷 —— 需略過 ANSI 色碼並處理寬字元，捨棄。

### 標頭固定、服務與變化清單一起捲動

畫面分三段：固定標頭（`store:` 與 `daemon:` 兩行；讀取失敗時只有 `store:` 一行）、可捲動的內容（`text` 去掉標頭後的服務行，接著空行、`recent changes:` 與各筆變化）、頁尾。內容放得下時（標頭行數 + 內容行數 ≤ 終端機行數）不顯示頁尾、位置固定為 0，畫面與現在相同。放不下時最後一列為頁尾，可捲動區高度為「終端機行數 − 標頭行數 − 1」（至少 1 列），頁尾格式為 `-- lines <first>-<last> of <total>  PgUp/PgDn Up/Down Home/End  q quit --`，`first`、`last` 為 1 起算的顯示範圍，並以 reverse video 顯示（`NO_COLOR` 時不加）。

為此 `text` 拆成 `header` 與新的 `services_lines`（服務部分），`text` 仍回傳兩者相接的結果，非 watch 輸出不變。

- 理由：daemon 狀態是最重要的資訊，捲到哪裡都要看得到；"recent changes:" 跟在服務後面以維持現有的版面，End 鍵一次就能到。
- 替代方案：把 "recent changes:" 也固定在底部 —— 服務多時會吃掉 11 列以上，可捲動區太小，捨棄。

### 捲動位置以第一個顯示的內容行表示並每次夾住

`Scroll { offset }` 記錄可捲動區第一個顯示的行。動作：Up/Down ±1、PageUp/PageDown ± 頁高（頁高 = 可捲動區高度）、Home 為 0、End 為最大值。最大值 = 內容行數 − 頁高（不足時為 0）。每次重畫前與每次動作後都以目前的內容行數與頁高夾住，所以清單變短、終端機變大時不會留在空白處；位置在 tick 之間保留，與內容是否變化無關。

### 按鍵對應

| 按鍵 | 動作 |
| --- | --- |
| PageUp | 上一頁 |
| PageDown、空白鍵 | 下一頁 |
| ↑ | 上一行 |
| ↓ | 下一行 |
| Home | 頂端 |
| End | 底端 |
| `q`、`Q`、Ctrl-C | 結束 |

只處理 `KeyEventKind::Press`（與 Repeat）；其他按鍵忽略。Resize 事件立刻以新大小重畫。按鍵與 Resize 只用上一次讀到的內容重畫，不重新讀 store、不呼叫 `Watch::update`，所以反白仍只在讀取後的那一個 frame 出現一次（按鍵觸發的重畫保留同一份反白）。

### 以 guard 與 panic hook 還原終端機

`TerminalGuard` 在建立時進入 raw mode 與 alternate screen，`Drop` 時依序 `EnableLineWrap`、`cursor::Show`、`LeaveAlternateScreen`、`disable_raw_mode`，錯誤忽略。訊號、按鍵結束都經由 guard 的 `Drop`。另外安裝 panic hook：先執行同樣的還原再呼叫原本的 hook，讓 panic 訊息出現在主畫面上且終端機不停在 raw mode（release profile 為預設的 unwind，guard 也會 drop；還原動作重複執行無害）。結束後恢復原本的 panic hook。

### 非終端機與事件錯誤

標準輸出不是終端機時照舊印 `--watch needs a terminal` 並以 2 結束，不進入 raw mode。`enable_raw_mode` 失敗（例如標準輸入與 `/dev/tty` 都不可用）時印 `cannot use the terminal: <錯誤>` 並以 2 結束。無法安裝訊號處理時印 `cannot install signal handlers`，原本以 0 結束，改為以 2 結束（與其他無法開始的情況一致）。`EventStream` 回傳錯誤或結束時停止讀取按鍵、繼續每秒重畫，仍可用訊號結束。

## Implementation Contract

**Behavior**：在終端機中執行 `emfor status --watch`：

- 內容放得下時畫面與現在相同（無頁尾）。
- 放不下時 `store:`、`daemon:` 兩行固定在頂端，最後一列顯示 `-- lines 1-<n> of <total>  PgUp/PgDn Up/Down Home/End  q quit --`；PageDown 後第一個顯示的內容行往下移一頁，End 後最後一筆變化出現在頁尾上方。
- 每秒刷新後捲動位置不變；服務被移除使內容變短時，位置夾到新的最大值。
- 按 `q` 或 Ctrl-C，或收到 SIGINT / SIGTERM / SIGHUP，回到主畫面、游標可見、終端機回到 cooked mode（輸入有回顯、Enter 換行正常），以 0 結束。
- 標準輸出不是終端機：`--watch needs a terminal`，結束碼 2。

**Interface**（`status.rs` 內部，供單元測試）：

- `enum Action { LineUp, LineDown, PageUp, PageDown, Top, Bottom, Quit }`
- `fn key_action(key: &crossterm::event::KeyEvent) -> Option<Action>`
- `struct Scroll { offset: usize }`，`fn apply(&mut self, action, content_len, page)` 與 `fn clamp(&mut self, content_len, page)`
- `fn compose(header: &[String], content: &[String], rows: usize, scroll: &mut Scroll, style: Style) -> Vec<String>`：回傳整個畫面要顯示的行（含頁尾），行數不超過 `rows`（`rows` 小於標頭加 2 時，可捲動區仍至少 1 列，標頭照樣輸出）。
- `fn services_lines(...)`：`text` 的服務部分；`text = header + services_lines`。

**Failure modes**：見「非終端機與事件錯誤」；寫入終端機失敗照舊忽略。

**Acceptance**：

- 單元測試：`key_action` 涵蓋上表每個按鍵、Release 事件與未對應按鍵回 `None`；`Scroll` 涵蓋 PageDown 一頁、Up 在 0 不變、End 到最大值、內容縮短後夾住、內容放得下時位置為 0；`compose` 涵蓋放得下不加頁尾、放不下時標頭固定、頁尾數字、極小終端機。
- `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features` 通過。
- `cargo check --target x86_64-unknown-freebsd` 對只依賴 crossterm（`event-stream`）的暫存 crate 通過，證明 crossterm 在 FreeBSD 可編譯。
- 手動：在終端機以超過一屏的設定跑 `emfor status --watch`，確認可捲動、`q` 與 Ctrl-C 可結束、`kill` 結束後終端機已還原。

**Scope**：只改 `--watch` 的終端機處理與畫面組成；`Watch::update`、`Watch::changes`、`text` 的內容格式、`--json` / `--history`、結束碼規則不變。

## Risks / Trade-offs

- [新增相依套件增加供應鏈與編譯時間] → 只啟用 `event-stream`，不加 ratatui；版本記錄在 Cargo.lock。
- [FreeBSD 沒有本機環境，只能交叉 `cargo check`] → 文件與 tasks 記下只做過編譯檢查；選用的 CI freebsd job 可補執行測試。
- [關閉自動換行後太長的行被截掉] → 與現在被折行推出畫面相比資訊損失較小；需要完整原因時可用 `--json` 或 `--history`。
- [panic hook 是全域狀態] → 只在 `--watch` 期間替換，結束時換回原本的 hook。

## Migration Plan

無資料或設定遷移。退回只需還原此 change 的程式與 crossterm 相依。

## Open Questions

（無）
