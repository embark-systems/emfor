## Context

writer.rs 的寫入執行緒每秒一次 tick（`tokio::time::interval_at`，`MissedTickBehavior::Delay`），tick 呼叫 `heartbeat()`，寫 `unix_ms()`（`SystemTime::now()` 的毫秒數）進 store 的 `meta/heartbeat`。writer 結構裡沒有上一次 tick 的時間，daemon 其他地方也不比較牆鐘，所以時鐘跳動完全沒有記錄。client 與 `emfor status` 把心跳與自己的牆鐘相差超過 `max_staleness` 判為過期（兩個方向），這行為不變。

2026-11-25 的討論（TODO 73 第 2 項）決定：不改心跳內容、不加單調時鐘或 boot id 心跳；只做「記錄時鐘跳動」這件可選的小事。

## Goals / Non-Goals

**Goals:**

- 時鐘被調動後，維運人員在 daemon 的 log 找得到一行說明：往哪個方向、多少毫秒。
- `/metrics` 有一個計數器可以告警。
- 一般運轉（含 daemon 被卡住、系統很忙、chrony 平滑調整）不誤報。

**Non-Goals:**

- 不改心跳、store 格式、client 的 `is_stale`、聯防的 5 秒規則與時鐘偏差量測。
- 不補償或修正時鐘，不停止檢查，不重送心跳。
- 不加 `[log]` 或其他設定項：門檻與頻率上限寫死（見風險）。

## Decisions

### 用「牆鐘走的量減單調時鐘走的量」判斷跳動

每次 tick 取 `unix_ms()` 與單調時鐘（正式程式用 `std::time::Instant`），與上一次 tick 的值相減得 `wall_delta` 與 `mono_delta`，`jump = wall_delta - mono_delta`。`|jump| > 2000` 毫秒就是跳動：`jump > 0` 為 `forward`，`jump < 0` 為 `backward`，`jump_ms` 記 `|jump|`。

理由：daemon 被卡住、被排程延後、tick 延遲（`Delay` 模式）時兩個時鐘一起走，`jump` 仍接近 0；只有牆鐘被調動時兩者才分開。chrony 的 slew 最大約每秒 0.5 毫秒，遠小於門檻，不會誤報。比較 tick 間隔本身（例如 tick 遲到超過 2 秒）會在 daemon 忙時誤報，所以不用。

### 比較邏輯抽成純函式，時間值由呼叫端傳入

新增 `ClockWatch`（在 writer.rs 內或同 crate 小模組），方法 `observe(wall_ms: u64, mono_ms: u64) -> Observation`，不呼叫任何系統時鐘。`heartbeat()` 呼叫 `observe(unix_ms(), 由 Instant 算出的毫秒)`。單元測試直接餵數字，不需要可注入的時鐘 trait，也不必動 `unix_ms()` 的其他呼叫處。

### 第一次 tick 只當基準

`ClockWatch` 在第一次 `observe` 只記下基準、回傳「無跳動」。啟動時 writer 已經寫過一次心跳，第一個 tick 在一個週期之後，所以基準與第二次 tick 之間才有第一次比較。

### log 頻率上限：每 10 秒最多一行；計數器不受影響

`Observation` 帶兩個旗標意義：是否為跳動（計入計數器）、是否要記 log。距離上次記 log 不到 10 秒的跳動只計數、不記 log。時鐘被連續調動時不會每秒一行。

### 計數器放在 `LiveView`，仿 `commit_failures`

`LiveView` 加一個 `u64` 欄位與 `count_clock_jump()`（和 `count_commit_failure()` 同處），`metrics.rs` 在 `emfor_store_commit_failures_total` 附近輸出 `emfor_clock_jumps_total`（`counter`，無標籤）。writer 已經持有 `live`，不需要新的連線。選擇無標籤：方向從 log 看，計數器只用來告警。

### log 行格式，不列入穩定欄位

`tracing::warn!`，欄位 `event = %"clock_jumped"`、`direction = %"forward"` 或 `"backward"`、`jump_ms = <數字>`；訊息說明「host clock jumped <方向> by <n.n> s; prefer slewing (chrony) to stepping」。維持既有說法：穩定欄位只有狀態變更行與 reload 稽核行；新行屬於「其他行的欄位」，不穩定，手冊不改穩定清單。

## Implementation Contract

- **行為**：daemon 運轉時，牆鐘與單調時鐘在兩次心跳 tick 之間相差超過 2000 毫秒，記一行 `warn`（欄位見上）並讓 `emfor_clock_jumps_total` 加一；距離上次 `clock_jumped` log 不到 10 秒時不記 log、計數照加。第一次 tick 不比較。時鐘穩定、daemon 被卡住或 tick 遲到時沒有輸出。
- **介面**：`ClockWatch::observe(wall_ms, mono_ms)` 回傳無跳動，或帶方向、`jump_ms`、`log` 旗標的跳動；`LiveState::count_clock_jump()`；`/metrics` 的 `# HELP`、`# TYPE emfor_clock_jumps_total counter` 與一行值，與其他 family 一樣各一次。
- **失敗模式**：`unix_ms()` 失敗時回 0（既有行為）會讓 `wall_delta` 大幅變動而報一次 `backward`；這是現有 `unwrap_or(0)` 的結果，不另外處理。沒有新的錯誤路徑、不阻擋心跳寫入。
- **驗收**：新的單元測試通過（無跳動、往前、往後、門檻邊界 2000 與 2001、頻率上限、第一次 tick）；metrics 測試確認 family 唯一與輸出值；在 VM 用 `date -s` 往前往後調時鐘各一次，log 出現對應方向的一行、計數器加一，不調時鐘時 10 分鐘沒有 `clock_jumped`（見 tasks）。
- **範圍內**：writer.rs、live.rs、metrics.rs 的上述改動，手冊（en、zh_TW）與 CHANGELOG。**範圍外**：client、store 格式、聯防、設定項、心跳內容。

## Risks / Trade-offs

- **門檻 2 秒與頻率上限 10 秒是估計**，沒有量測。要確認的是：正常運轉時 `|wall_delta - mono_delta|` 的最大值遠小於 2 秒。量測方式：在 Debian docker 與 FreeBSD VM 各跑 daemon 10 分鐘，同時製造負載，記錄每次 tick 的 `jump`，取最大值；若最大值超過 500 毫秒就要重新考慮門檻。確認前不把「不誤報」寫成保證。
- 暫停再恢復的 VM、掛起再喚醒的主機：Linux 的 `CLOCK_MONOTONIC` 在睡眠時不走，喚醒後牆鐘追上時會報一次 `forward`。這是對的（client 也會看到心跳過期），不特別處理。
- 低於 2 秒的調動不會被記錄。client 的過期門檻是 `max_staleness`（預設 3 秒），小於 2 秒的跳動不會造成過期，所以不記是可以接受的。
- 門檻與頻率寫死，之後若要調整是另一個 change。

## Open Questions

- 計數器要不要加方向標籤？目前選無標籤（方向從 log 看）。若維運要分開告警，再加標籤是相容的擴充（新增標籤會改變既有 series，所以要在發版前決定）。
