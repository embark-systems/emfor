## Why

`[metrics]` 開啟後，排查時要知道 Prometheus 該抓哪個位址，目前只能翻設定檔；而設定檔改了 `listen` 但還沒重啟時，檔案裡的位址和 daemon 實際在聽的不同。`emfor status` 已經從 store 顯示 daemon 的執行狀態與 `site`，把 metrics 位址放在同一行最直接。

## What Changes

- daemon 啟動時，在寫入 `meta/started_at` 的同一筆交易中寫入 `meta/metrics_listen`：值為 JSON 字串陣列，依 `[metrics] listen` 的順序列出位址（例如 `["127.0.0.1:5301"]`）；設定沒有 `[metrics]` 時刪除此鍵，避免留下上一次執行的值。`[metrics] listen` 不能 reload，所以只在啟動時寫。
- `emfor status` 的 daemon 行在 `running` 時，於 `, site <site>` 之後加上 `, metrics <位址>`，多個位址以空白分隔；`stale`、`stopped` 時不顯示。
- `emfor status --json` 新增 `metrics_listen`：位址字串陣列，鍵不存在或無法解析時為 `null`。
- client、DNS 不讀此鍵；`meta/schema_version` 維持 `1`（讀取端本來就忽略不認識的鍵）。

## Non-Goals

- 不顯示 `[dns] listen`：使用者這次只要 metrics 位址，DNS 位址可另開 change。
- status 不讀設定檔的 `[metrics]`：設定檔可能尚未生效，以 store 中 daemon 實際寫入的值為準。
- 不改 `emfor status` 的結束碼。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `state-store`: 新增 `meta/metrics_listen` 鍵的寫入與刪除規則。
- `status-command`: daemon 行與 `--json` 顯示 metrics 位址。

## Impact

- Affected specs: state-store, status-command
- Affected code: crates/emfor-format/src/lib.rs, crates/emfor-daemon/src/writer.rs, crates/emfor-daemon/src/cli.rs, crates/emfor-daemon/src/status.rs, crates/emfor-daemon/tests/daemon.rs, docs/deploy/daemon.md
