## Why

`emfor status` 已經顯示 metrics 位址（emfor-status-metrics），但 DNS 位址仍要翻設定檔，而設定檔中的 `[dns] listen` 可能改了還沒重啟。把 DNS 位址也放進 daemon 行，排查 resolver 轉發時不必再對照設定檔。

## What Changes

- daemon 啟動時，在寫入 `meta/started_at` 的同一筆交易中寫入 `meta/dns_listen`：JSON 字串陣列，依 `[dns] listen` 的順序；設定沒有 `[dns]` 時刪除此鍵。`[dns] listen` 不能 reload，所以只在啟動時寫，與 `meta/metrics_listen` 相同。
- `emfor status` 的 daemon 行在 `running` 時，於 `site` 之後、`metrics` 之前加上 `, dns <位址>`，多個位址以空白分隔。
- `emfor status --json` 新增 `dns_listen`（陣列，或 `null`）。
- writer 啟動時要寫的這兩個 listen 鍵改由同一個結構帶入，不再各加一個參數。

## Non-Goals

- 不顯示 `[dns] zone`：zone 可以 reload，要顯示就得在 reload 時也更新 store，另案處理。
- client 與 DNS 伺服器不讀此鍵；`meta/schema_version` 維持 `1`。
- 不改 `emfor status` 的結束碼。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `state-store`: 新增 `meta/dns_listen` 鍵的寫入與刪除規則。
- `status-command`: daemon 行與 `--json` 顯示 DNS 位址。

## Impact

- Affected specs: state-store, status-command
- Affected code: crates/emfor-format/src/lib.rs, crates/emfor-daemon/src/writer.rs, crates/emfor-daemon/src/cli.rs, crates/emfor-daemon/src/status.rs, crates/emfor-daemon/tests/daemon.rs, docs/deploy/daemon.md, README.md, README.zh-TW.md
