## Why

主機時鐘被調動（手動 `date -s`、NTP step、VM 暫停後恢復）時，daemon 寫進 store 的心跳是牆鐘時間，client 拿自己的牆鐘去比：差超過 `max_staleness`（預設 3 秒）就把 store 當成過期。活的 daemon 在下一次心跳（約 1 秒內）會自己恢復，但這段時間 client 看到過期，`on_all_down` 生效（`open` 照樣從全部後端挑，`closed` 回 `ALL_DOWN`）。維運人員事後只看到「store 過期」或幾筆 `ALL_DOWN`，daemon 完全沒有記錄時鐘跳過，手冊第 12 章「After a clock jump」也只能叫人自己去查 chrony。

TODO 73 第 2 項的討論（2026-11-25）決定不改心跳內容、不做單調時鐘或 boot id 心跳，只留下一件可選的小事：讓 daemon 自己偵測並記下時鐘跳動，給維運一個證據與一個可以告警的計數。

## What Changes

- 寫心跳的每秒 tick 比較兩件事：兩次 tick 之間牆鐘走了多少毫秒，與單調時鐘走了多少毫秒。兩者相差超過門檻（2 秒）就視為時鐘跳動。daemon 被卡住或系統忙時兩個時鐘一起走，不會誤報；時鐘被調動時只有牆鐘變，會報。
- 偵測到時記一行 `warn`，欄位 `event = clock_jumped`、`direction`（`forward` 或 `backward`）、`jump_ms`，訊息用一句話說明主機時鐘往哪個方向調了多少，並建議用 slew（chrony 預設）而不是 step。
- 記 log 的頻率有上限（同一個 daemon 每 10 秒最多一行），避免時鐘一直被調動時洪水；每次跳動仍會累計到計數器。
- 新增 `/metrics` 計數器 `emfor_clock_jumps_total`，每偵測到一次跳動加一（不受 log 頻率上限影響）。
- 啟動後第一次 tick 只記下基準，不比較。
- 手冊第 12 章「After a clock jump」與第 16 章補這行 log 長什麼樣、計數器怎麼用（en 與 zh_TW）；CHANGELOG Unreleased 加一條。
- 數字都是估計，不是量測值：門檻 2 秒與 10 秒頻率上限。確認方式見 design.md（量測正常運轉時兩個時鐘差的最大值，確認遠小於門檻；用 `date -s` 在 VM 往前往後調，確認會報）。

## Non-Goals

- 不改心跳的內容或 store 格式，不加 boot id 或單調時鐘心跳（2026-11-25 討論決定不做，要改 store 格式與跨語言 client 規格，只為縮短約 6 秒的窗口）。
- 不改 client 的 `is_stale`，不改聯防的 5 秒規則與時鐘偏差量測（`emfor_peer_clock_offset_seconds` 量的是成員之間的差，這裡量的是本機時鐘自己跳）。
- 不嘗試修正或補償時鐘跳動，也不因為跳動停止檢查或重送心跳；只記錄與計數。
- 新的 log 行欄位不列入穩定欄位清單（維持「其他行的欄位不穩定」的既有說法）；計數器名稱屬於 `/metrics`，依該端點既有的相容性規則。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `state-store`: 新增一條需求：寫心跳時偵測牆鐘與單調時鐘的差，超過門檻記 `clock_jumped` 警告，並有頻率上限與基準規則。
- `metrics-endpoint`: 新增一條需求：`emfor_clock_jumps_total` 計數器，每偵測到一次時鐘跳動加一。

## Impact

- 程式：`crates/emfor-daemon/src/writer.rs`（心跳 tick 的偵測與頻率上限，新增比較用的純函式）、`crates/emfor-daemon/src/live.rs`（計數器欄位與寫入方法）、`crates/emfor-daemon/src/metrics.rs`（輸出計數器）。
- 文件：`docs/manual/12-operating.md`、`docs/manual/16-troubleshooting.md` 與 `docs/manual.zh_TW/` 對應檔；`CHANGELOG.md` Unreleased。
- 規格：`docs/spectra/specs/state-store/spec.md`、`docs/spectra/specs/metrics-endpoint/spec.md` 各一條新需求。
- 不影響 client、store 格式、DNS、聯防協定。
