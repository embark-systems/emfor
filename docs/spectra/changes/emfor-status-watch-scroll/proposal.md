## Why

`emfor status --watch` 每秒把整份清單從畫面左上角重畫，不知道終端機有多高；服務一多，清單超過一屏，下面的服務與 "recent changes:" 就看不到，維運人員只能改用 `--service` 一個一個看。

## What Changes

- `--watch` 進入 raw mode 讀取按鍵，知道終端機大小，並在終端機改變大小時重畫。
- `store:` 與 `daemon:` 兩行固定在最上面；其下的服務與 "recent changes:" 為可捲動區域。內容超過畫面時最下面一行顯示目前位置與按鍵說明。
- PageUp / PageDown 一次捲一頁，上下方向鍵一次一行，Home / End 到頭尾；每秒重畫時保持捲動位置，清單變短時夾回有效範圍。
- raw mode 下 Ctrl-C 不再產生 SIGINT，所以 Ctrl-C 與 `q` 改由按鍵結束；SIGINT、SIGTERM、SIGHUP 照舊結束。
- 正常結束、收到訊號、panic 時都還原終端機（離開 raw mode、顯示游標、恢復自動換行、回到主畫面）。
- 新增相依套件 crossterm（啟用 `event-stream`），取代 `--watch` 手寫的 escape sequence。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `status-command`: "Status watch mode" 增加捲動、按鍵結束與固定標頭的規則。

## Impact

- 程式：crates/emfor-daemon/src/status.rs（`watch` 與畫面組成）
- 相依：crates/emfor-daemon/Cargo.toml、Cargo.lock 新增 crossterm 及其相依套件
- 文件：docs/deploy/daemon.md、docs/manual/08-operations.md、docs/manual.zh_TW/08-operations.md、CHANGELOG.md、docs/spectra/config.yaml（專案脈絡中「不用 TUI 套件」一句）
