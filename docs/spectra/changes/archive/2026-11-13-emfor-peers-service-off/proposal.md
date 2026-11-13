## Why

目前要讓某個服務不參加聯防，只能拿掉整個 `[peers]`（要重啟，所有服務一起退出）。有些服務不適合聯防：例如檢查結果本來就因主機而異（只看本機能否連到）、或維運人員排查時想讓某個服務暫時只用本機檢查。需要能以服務為單位關掉聯防，而且 reload 就生效。

## What Changes

- 新設定 `peers`（布林，預設 `true`），可寫在 `[defaults]` 或個別服務表；值不是布林時設定錯誤 `peers must be true or false`。沒有 `[peers]` 時照樣接受，沒有作用。
- `peers = false` 的服務在本機完全不參加聯防：
  - 不送 `down` 通知、不送 `result`；
  - 不用其他成員的票（不計區域判斷、不分工、每次都自己檢查）；
  - 收到該服務的 `down`、`result` 一律忽略（不啟動快速重檢、不記成 digest 不符）；
  - 該服務的票、意見不一、digest 不符紀錄全部丟掉。
- reload 改 `peers` 立即生效，後端狀態保留（和改 `interval` 一樣，不從 `unknown` 重來）。
- `meta/peers` 新增欄位 `services_off`：本機 `peers = false` 的服務名稱（排序）。
- `emfor status`：`services_off` 裡的服務不會標 `voted`、不計入 `checks n/m backends here`；`--json` 的 `assigned_checkers` 對這些服務的後端為空陣列。
- 文件說明：其他成員不知道本機關了哪些服務，仍可能把本機排為負責主機；所以同一服務最好在所有成員設成一樣。

## Non-Goals

（見 design.md 的 Goals / Non-Goals。）

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `service-config`: 設定鍵與預設值加 `peers`；驗證規則加 `peers` 必須是布林。
- `peer-group`: 新增需求「Services left out of the peer group」，規定 `peers = false` 服務的行為與 reload。
- `state-store`: `meta/peers` 加 `services_off` 欄位。
- `status-command`: `services_off` 裡的服務不標 `voted`、不計入本機檢查數，JSON 的 `assigned_checkers` 為空。

## Impact

- 程式：crates/emfor-daemon/src/config.rs（`Settings.peers`、`SETTING_KEYS`、驗證）、crates/emfor-daemon/src/schedule.rs（後端 task 依 `peers` 決定是否用聯防）、crates/emfor-daemon/src/supervisor.rs（`publish_targets` 排除關閉的服務、告知 quorum）、crates/emfor-daemon/src/quorum.rs（`meta/peers` 的 `services_off`）、crates/emfor-daemon/src/status.rs（解析 `services_off`）。
- 測試：上述檔案的單元測試、crates/emfor-daemon/tests/peers.rs 整合測試。
- 文件：docs/manual/03-configuration.md、docs/manual/01-concepts.md、docs/manual/08-operations.md、examples/emfor.toml、CHANGELOG.md。
- store 格式 `meta/schema_version` 仍為 1（`meta/peers` 只有 status 讀）；client、DNS 不變；不新增相依套件。
