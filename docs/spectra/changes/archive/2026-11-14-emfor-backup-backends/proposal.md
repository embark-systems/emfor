## Why

有些服務有一組平常不想用的 backend，例如另一個資料中心的副本、規格較差的機器，或冷備的主機。它們只應該在主要的 backend 都不能用時才接手。目前 emfor 只能用 `failover`（全部依序，一次只用一台）或 `locality`（依 site 偏好），沒辦法說「這幾台平常輪流用，那幾台只在它們都出問題時才用」。

## What Changes

- backend 表格新增選填的 `backup`（布林值，預設 `false`）。服務的 backend 全部都是 `backup` 時，設定檢查報錯。
- store 新鮮時，候選依下列順序，取第一個不是空的集合：
  1. 最近一次檢查通過的主要 backend；
  2. 最近一次檢查通過的備援 backend；
  3. 健康的主要 backend（包含正在失敗的）；
  4. 健康的備援 backend（包含正在失敗的）；
  5. 交給 `on_all_down`：`open` 照現行的「`unknown` 優先」規則，在全部 backend（主要與備援一起）中挑；`closed` 為 all down。
- store 過期時與現在相同：沒有 backend 算健康，`open` 用全部 backend。
- 規則放在共用的 `rules::healthy_or_all`，所以 client 查詢（Rust、C library 與各語言 wrapper）和 DNS 的 A、AAAA、SRV 回答都跟著改。SRV 只列目前的候選，priority 編法不變。各 `balance` 方法、slow start 都在選出的那一層裡運作。
- **BREAKING**：store 格式版本升到 2，backend 記錄新增 `backup` 欄位。
  - daemon 一律寫版本 2，沒有設定 `backup` 也一樣。
  - 新版 client 與 `emfor status` 同時接受版本 1 與 2，版本 1 的 backend 都當作不是備援。
  - 舊版 client 讀到版本 2 時，所有查詢都回 `INCOMPATIBLE`；舊版 `emfor status` 會以 exit 2 結束。
  - 升級順序必須是先升級所有 client，再升級 daemon。
- 更新手冊、`examples/emfor.toml`、`CHANGELOG.md` 與 conformance 資料。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `service-config`：requirement「Host site and backend weight and site」加入 `backup`，以及「不能全部都是備援」的驗證。
- `state-store`：requirement「Store record format version 1」改為版本 2，並新增 `backup` 欄位。
- `backend-selection`：requirement「Candidate backends per address family」加入備援層。
- `client-lookup`：
  - requirement「Candidates follow health and on_all_down」加入備援層；
  - requirement「Lookup failures use four shared error codes」改為接受版本 1 與 2。
- `dns-server`：requirement「SRV answers」的候選加入備援層。
- `status-command`：requirement「Status reads the store read-only」改為接受版本 1 與 2。

## Impact

- 受影響的 spec：service-config、state-store、backend-selection、client-lookup、dns-server、status-command
- 受影響的程式碼：
  - crates/emfor-format/src/lib.rs：`SCHEMA_VERSION` 改為 2、`BackendRecord.backup`，以及可接受版本的判斷
  - crates/emfor-format/src/rules.rs：`healthy_or_all` 加入備援層
  - crates/emfor-daemon/src/config.rs：backend 表格的 `backup` 與驗證
  - crates/emfor-daemon/src/writer.rs、crates/emfor-daemon/src/supervisor.rs：把 `backup` 寫進 store
  - crates/emfor-daemon/src/select.rs：A/AAAA 與 SRV 的測試
  - crates/emfor-daemon/src/status.rs：接受版本 1 與 2
  - clients/rust/src/client.rs、clients/rust/src/select.rs：接受版本 1 與 2，以及候選測試
  - tests/conformance/v1/cases.json、tools/emfor-fixture：版本 2 的案例
  - docs/manual 與 docs/manual.zh_TW 的第 3、5、6、7 章、examples/emfor.toml、CHANGELOG.md
- 升級：舊版 client 與舊版 `emfor status` 讀不了新 daemon 的 store，必須先升級。
