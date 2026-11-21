## Context

見 proposal。三個修正互相獨立，但都屬於「讀密鑰檔」，一起做一次 verify。

## Goals / Non-Goals

**Goals:** 舊路徑晚到的結果不套用；kqueue 開檔不會在 FIFO 上卡住；`secret_file` 的讀取有上限。

**Non-Goals:** 見 proposal。

## Decisions

以下是我（助手）做的設計選擇，使用者可以改。

### 進行中的讀取記下路徑，結果回來時比對

`ReadInFlight` 增加一個 `PathBuf`（開始讀取時的路徑）。`refresh` 在讀取已結束時，若記下的路徑不等於這次傳入的現行路徑，丟棄結果（不呼叫 `apply_secret`，因此也不會有警告或輪替），接著照常開始讀現行路徑。丟棄時不記日誌：換路徑本來就是使用者的動作。讀取仍卡住時行為不變（等它結束，不新開）。

### kqueue 開檔用 `O_NONBLOCK`

目錄與檔案都以 `OpenOptions::custom_flags(libc::O_NONBLOCK)` 開，與 `read_secret` 的註解同一個理由。對一般檔案與目錄沒有影響。這段程式碼只在 FreeBSD 與 macOS 編譯；本機只能 `cargo check`/`clippy`，測試不能在 Debian 容器跑（既有限制）。

### 上限 65536 位元組，用 `take` 讀

`read_secret` 以 `f.take(MAX_SECRET_LEN as u64 + 1).read_to_end(...)` 讀，讀到超過 65536 位元組就回錯誤「is too large (more than 65536 bytes)」。常數 `MAX_SECRET_LEN` 放在 `config.rs`。檢查在去掉結尾換行之前之後都一樣，用讀到的總長度判斷（含換行）。

## Implementation Contract

- **行為**：見上。丟棄舊路徑的結果後，新路徑在同一次 `refresh` 內讀（與沒有進行中的讀取時一樣等最多 `SECRET_READ_WAIT`）。
- **介面**：`SecretReader::refresh` 簽章不變；`ReadInFlight` 是 `task.rs` 內部型別；`read_secret` 簽章不變。
- **失敗模式**：超過上限是一般的設定錯誤，沿用 `peers.secret_file:`／`peers.previous_secret_file:` 前綴。
- **驗收**：`tools/docker-test.sh` 通過；新測試：舊路徑的結果被丟棄、大檔被拒絕且最多讀 65537 位元組；macOS 上 `cargo check`/`clippy` 乾淨（kqueue）。
- **範圍內**：上述三項與手冊、CHANGELOG。**範圍外**：見 proposal 的 Non-Goals。

## Risks / Trade-offs

- 丟棄不記日誌，少了線索；換路徑的 reload 本來就有 `info` 日誌（監看建立），可接受。
- 65536 若不夠（極不可能）使用者要改程式；因為沒有設定項目。這是刻意的，避免再多一個選項。

## Open Questions

- 無。
