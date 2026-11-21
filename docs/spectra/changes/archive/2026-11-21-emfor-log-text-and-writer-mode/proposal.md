## Why

三個低嚴重度項目（2026-11-21 的 Fable 審查，程式碼位置已逐一讀過確認）：

1. plugin 的 stderr 行與它在 Hello 裡自報的名稱和版本，原樣寫進日誌（`crates/emfor-daemon/src/plugin/host.rs` 的 `log_stderr`，以及啟動時的 `plugin {name} started (... {plugin_name} {version})`）。檢查結果的 reason 已經經過 `normalize_reason` 跳脫控制字元與 bidi 字元；這兩條路徑沒有。plugin 可以印出 CR 蓋掉同一行前面的字、或用 bidi 字元重排日誌。
2. redis 檢查把伺服器對 `AUTH` 回的錯誤文字原樣放進失敗原因（`crates/emfor-check/src/redis.rs` 的 `auth: {e}`，以及 `describe` 對簡單回覆與錯誤的回顯）。一個把參數回顯的伺服器（惡意或設定錯誤的；正版 Redis 的 WRONGPASS 不回顯）會讓密碼進到 store、日誌與 `emfor status`。
3. `<store>.writer` 以 `OpenOptions::create` 建立，模式是 umask 預設（常見 0644）。`flock` 不需要寫入權限，任何能唯讀開啟這個檔案的本機使用者都能持有鎖，讓 daemon 啟動失敗（`crates/emfor-daemon/src/store.rs` 的 `Store::open`）。

## What Changes

- 新增 `escape_unprintable`（`checker/mod.rs`，`normalize_reason` 改用它再加長度上限）；plugin 的 stderr 行與 Hello 的名稱、版本寫日誌前先經過它。
- redis 檢查在回報伺服器文字前，把密碼出現的地方換成 `***`。
- `<store>.writer` 以 0600 建立，已存在且模式不同時設成 0600；設不成只記 `warn`，不阻止啟動。spec 的「Single writer per store」增加這點與一個情境。

## Non-Goals (optional)

- 不改 spec 的 checker-plugins（stderr 記錄的需求沒有說到跳脫，既有的情境對一般文字仍成立）。
- 不處理 plugin 重啟時背景任務沒有 abort（受 EOF 限制，需要脫離行程群組的子孫行程才會漏）、reload 時同步讀設定檔、亂序結果覆蓋較新的票、mongodb 的 PBKDF2 上限（程式碼已有 1,000,000 次的上限，審查的描述不成立）。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `state-store`: "Single writer per store" 增加 `.writer` 檔的模式。

## Impact

- Affected specs: state-store
- Affected code:
  - New: (none)
  - Modified: `crates/emfor-daemon/src/checker/mod.rs`、`crates/emfor-daemon/src/plugin/host.rs`、`crates/emfor-check/src/redis.rs`、`crates/emfor-daemon/src/store.rs`、`docs/manual/01-concepts.md`（與 zh_TW）、`CHANGELOG.md`
  - Removed: (none)
- Compatibility: 其他本機使用者原本可以（意外地）開啟 `.writer`；現在不行。應用程式不需要這個檔案（它們用 `-lock` 檔）。
