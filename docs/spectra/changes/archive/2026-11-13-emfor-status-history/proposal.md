## Why

`emfor status` 只看得到現況：store 只存每台後端目前的狀態，`--watch` 也只列得出自己開著期間看到的變化。後端「一直在 healthy 與 unhealthy 之間跳」或「凌晨掛過一次又恢復」都看不到，只能翻 daemon 的 log；daemon 重啟後連這段經過都對不起來。

## What Changes

- daemon 在 LMDB 新增 `hist/<service>` key：每台後端最近 20 筆健康狀態轉換（時間、舊狀態、新狀態、當時的失敗原因），與帶有該轉換的服務記錄在同一個交易寫入。
- 歷史跨 daemon 重啟保留；啟動與 reload 時刪掉已不在設定中的服務與後端的歷史。client 與 DNS 不讀這個 key（讀取端本來就忽略不認得的 key），store 格式版本維持 1。
- `emfor status` 依歷史判斷抖動：10 分鐘內 healthy 與 unhealthy 之間轉換 5 次以上的後端，在該列標出 `FLAPPING (<n> changes in 10m)`。只顯示，不影響結束碼，也不影響 daemon 挑選後端。
- `emfor status --history`：列出每台後端的轉換紀錄（新的在上），可搭配 `--service`。
- `emfor status --json` 多出 `history`（store 中的歷史原樣）與 `flapping`（被判定抖動的後端與轉換次數）。
- `emfor status --watch` 的「最近變化」改從 store 的歷史取得，開始時就看得到之前的轉換；daemon 狀態變化仍只記錄執行期間看到的。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `state-store`: 新增後端狀態歷史 `hist/<service>` 的內容、筆數上限、寫入時機與保留規則。
- `status-command`: 新增抖動標示與 `--history`；JSON 多出 `history`、`flapping`；`--watch` 的變化清單改用 store 的歷史。

## Impact

- 程式：crates/emfor-format/src/lib.rs（歷史記錄型別與 key 前綴）、crates/emfor-daemon/src/writer.rs（記錄、上限、修剪、啟動時載入）、crates/emfor-daemon/src/cli.rs（啟動時把既有歷史交給 writer）、crates/emfor-daemon/src/status.rs（讀歷史、抖動、`--history`、JSON、`--watch`）。
- 測試：writer 與 status 單元測試、crates/emfor-daemon/tests/daemon.rs（重啟後歷史仍在、`--history` 輸出）。
- 文件：docs/deploy/daemon.md、docs/spectra/config.yaml。
- 不影響：client（Rust、libemfor 與各語言包裝）、DNS、檢查器、store 格式版本；不新增 crate。
- store 大小：每個服務多一個 key，每台後端最多 20 筆，每筆約 100 位元組（估計，以實作後實測一個 10 台後端服務的 key 大小確認），預設 64 MiB 的 `map_size` 足夠。
