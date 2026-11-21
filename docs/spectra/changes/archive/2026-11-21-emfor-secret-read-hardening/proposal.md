## Why

2026-11-21 的 Fable 審查列了三個低嚴重度項目，都在這個 session 寫的或碰過的程式碼上：

1. `secret_file` 在 reload 換路徑時，舊路徑上還沒回來的讀取，晚到的結果會被當成現行密鑰套用（`SecretReader::refresh`，`crates/emfor-daemon/src/peers/task.rs`）。spec 現在寫「晚到的結果和其他結果一樣套用」，沒有排除換路徑的情況；那是舊路徑檔案的內容。
2. kqueue 後端（`crates/emfor-daemon/src/peers/watch.rs`）以 `File::open` 開目錄與檔案，沒有 `O_NONBLOCK`。路徑被換成 FIFO 時，`open` 會在沒有寫入端時卡住監看執行緒（`read_secret` 開檔已經帶 `O_NONBLOCK`，原因相同）。
3. `read_secret`（`crates/emfor-daemon/src/config.rs`）用 `read_to_end` 讀整個檔案，沒有上限；`secret_file` 若指向一個巨大的一般檔案（例如被換成日誌），會讀進記憶體。

## What Changes

- `SecretReader` 記下進行中的讀取讀的是哪個路徑；結果回來時路徑已不是現行路徑就丟棄，並開始讀新路徑。spec 的「Peer secret reload」增加這個例外與一個情境。
- kqueue 後端開目錄與檔案時加 `O_NONBLOCK`。
- `read_secret` 最多讀 65537 位元組，檔案超過 65536 位元組（含結尾換行）是 `peers.secret_file`（與 `previous_secret_file`）的錯誤。spec 的「Peer group configuration」增加上限與一個情境。65536 是設計選擇：密鑰至少 32 位元組，沒有理由更大。

## Non-Goals (optional)

- 不處理審查的其他低嚴重度項目（reload 時同步讀設定檔、`.writer` 的 `flock`、亂序結果覆蓋較新的票、pf 範例腳本、plugin 重啟時的背景任務、CR 與 bidi 字元、redis AUTH 回顯、mongodb PBKDF2 上限、CI 與範例腳本）；它們在別的程式碼，各自另開。
- 不改 `emfor-check` 或 exec 的環境變數處理。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `peer-group`: "Peer group configuration"（`secret_file` 大小上限）與 "Peer secret reload"（換路徑時丟棄舊路徑晚到的結果）。

## Impact

- Affected specs: peer-group
- Affected code:
  - New: (none)
  - Modified: `crates/emfor-daemon/src/peers/task.rs`、`crates/emfor-daemon/src/peers/watch.rs`、`crates/emfor-daemon/src/config.rs`、`docs/manual/03-configuration.md`（與 zh_TW）、`CHANGELOG.md`
  - Removed: (none)
- Compatibility: 大於 65536 位元組的 `secret_file` 以前可用（只要符合其他條件），現在是設定錯誤；實際的密鑰沒有這麼大。
