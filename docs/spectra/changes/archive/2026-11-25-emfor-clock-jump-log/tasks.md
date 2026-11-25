## 1. 偵測與計數

- [x] 1.1 依 design 的「用牆鐘走的量減單調時鐘走的量判斷跳動」與「比較邏輯抽成純函式，時間值由呼叫端傳入」，並實現 spec 需求 Clock jump log line 的判斷、基準與頻率上限規則（design 的「log 頻率上限：每 10 秒最多一行；計數器不受影響」），新增 `ClockWatch`（`crates/emfor-daemon/src/writer.rs` 或同 crate 小模組）：`observe(wall_ms, mono_ms)` 第一次只記基準；之後以 `wall_delta - mono_delta` 判斷，`|jump| > 2000` 為跳動，回傳方向與 `jump_ms`，距上次記 log 不到 10 秒的跳動標為「不記 log」。驗證：單元測試涵蓋無跳動、往前、往後、剛好 2000 不報與 2001 報、tick 遲到但兩個時鐘一起走不報、第一次 tick 不比較、10 秒內第二次跳動計數但不記 log；agent 讀程式與測試、跑 `tools/docker-test.sh` 的 emfor-daemon 單元測試。
- [x] 1.2 [after: 1.1] 實現 spec 需求 Clock jump log line 的 log 輸出，依 design 的「log 行格式，不列入穩定欄位」：`heartbeat()` 每次 tick 以 `unix_ms()` 與 `std::time::Instant` 算出的毫秒呼叫 `ClockWatch::observe`；跳動時 `tracing::warn!` 寫 `event = %"clock_jumped"`、`direction`、`jump_ms` 與一句說明方向、秒數並建議 slew 的訊息，並呼叫 `live.count_clock_jump()`；心跳值、store 格式與 commit 流程不變。驗證：既有 writer 測試全部不變地通過，加一個測試確認跳動時 `LiveView` 的計數增加；agent 內容審閱。
- [x] 1.3 實現 spec 需求 Clock jump counter，依 design 的「計數器放在 LiveView，仿 commit_failures」：`LiveView` 加 `u64` 欄位與 `count_clock_jump()`（仿 `count_commit_failure()`），`metrics.rs` 輸出 `emfor_clock_jumps_total`（`counter`、無標籤，各一行 `# HELP` 與 `# TYPE`）。驗證：metrics 測試確認值為 0 時輸出 `emfor_clock_jumps_total 0`、計數 2 時輸出 `emfor_clock_jumps_total 2`，且既有「每個 family 只有一組 HELP 與 TYPE」的測試通過。

## 2. 量測與手動驗證

- [x] 2.1 [after: 1.2] 量測誤報：在 Debian docker（`tools/docker-test.sh` 的環境）與 FreeBSD VM 各跑 daemon 10 分鐘並製造 CPU 與 I/O 負載，臨時記錄每次 tick 的 `|wall_delta - mono_delta|` 最大值；驗證：最大值小於 500 毫秒，且整段期間 log 沒有 `clock_jumped`，結果寫進 CHANGELOG 或手冊的「量過」說明；若最大值 ≥ 500 毫秒回頭修 design 的門檻。
- [x] 2.2 [after: 1.2] 真實時鐘調整：在有 root 的 VM（FreeBSD 15.1 或 Rocky 9）用 `date -s`（或 `date` 設定）往前調一次、往後調一次；驗證：log 各出現一行對應方向的 `clock_jumped`（`jump_ms` 接近調整量），`/metrics` 的 `emfor_clock_jumps_total` 為 2，10 秒內再調一次時只增加計數；Debian docker 無法調時鐘，所以不在那裡做。

## 3. 文件

- [x] 3.1 [after: 1.2] 第 12 章「After a clock jump」（en 與 zh_TW「時鐘跳動之後」）補：daemon 偵測到跳動會記 `clock_jumped` 的 warn 與 `emfor_clock_jumps_total`，給一行範例與告警做法；第 16 章「The store looks stale while the daemon runs」（zh_TW「daemon 在執行，store 卻看起來過期」）加一句指向這行 log；寫明小於 2 秒的調動不會被記錄、log 欄位不在穩定清單。驗證：agent 內容審閱，en 與 zh_TW 各有對應段落，用語沿用手冊既有的「時鐘跳動」「時鐘偏差」（LANGUAGE.md 的區別）。
- [x] 3.2 [after: 2.1, 2.2] CHANGELOG Unreleased 加一條（格式照既有條目）：新 log 行、新計數器、門檻 2 秒與頻率上限 10 秒，並寫出 2.1 與 2.2 實際量到的結果與沒測的部分。驗證：agent 內容審閱，條目中的數字與 2.1、2.2 的結果一致。
