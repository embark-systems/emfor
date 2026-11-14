## Why

backend 從不健康恢復後，會立刻分到和其他 backend 一樣多的流量。剛重啟的服務快取是空的、連線池和 JIT 還沒暖好，一下子湧入全量流量，常常又被打掛，接著再恢復、再被打掛，反覆來回。slow start 讓剛恢復的 backend 在一段時間內先分到較少流量，再逐步增加到正常比例。

## What Changes

- 新增服務設定 `slow_start`：一段時間，可以寫在服務的表格或 `[defaults]`，預設 `"0s"`（關閉），允許 `0s` 或 1s 到 1h。
- backend 從 `unhealthy` 轉為 `healthy` 時，daemon 在 store 中該 backend 的記錄寫入 `recovered_at`，值是造成這次轉變的那次檢查的 `checked_at`。backend 離開 `healthy`、或從 `unknown` 轉為 `healthy` 時，`recovered_at` 為 `null`。服務記錄寫入 `slow_start_ms`，關閉時為 0。
- 有效權重：
  - 基礎權重：`weighted` 用 backend 的 `weight`，其他方法用 1。
  - 加量級數：恢復後經過的時間占 `slow_start` 的比例，以 10% 為一級，從 1 級（10%）到 10 級（全量）。
  - 有效權重 = 基礎權重 × 加量級數。不在加量中的 backend 一律是 10 級。
- 候選中有 backend 正在加量時：
  - `round-robin`、`weighted`、`locality`、`fastest`，以及沒有 affinity key 的 `sticky`，都改依有效權重挑選。client 用輪流計數器落在累積權重區間；daemon 的 DNS 用 smooth weighted round-robin。
  - `locality` 與 `fastest` 先照原本規則縮小候選，再在縮小後的候選中依有效權重挑選。
  - `failover` 和有 affinity key 的 `sticky` 不變。
  - SRV 回答的 weight 欄位填有效權重，priority 與順序規則不變。
- 沒有 backend 正在加量時，所有方法的行為與現在完全相同。
- store 過期時不加量。
- 從 `unknown` 轉為 `healthy` 不加量：包括 daemon 剛啟動、reload 新增的 backend。
- 修改 conformance 測試：`emfor-fixture` 支援以「距離現在多久」寫入 `recovered_at`，`tests/conformance/v1/cases.json` 新增 slow start 的案例。
- 更新手冊、`examples/emfor.toml` 與 CHANGELOG。

## Non-Goals (optional)

- 不加量的情況：`failover`、有 affinity key 的 `sticky`、daemon 啟動與 reload 新增的 backend。
- 不提供加量曲線的設定（固定為線性、10 級），也不提供最低比例的設定（固定 10%）。
- `emfor status`、`/metrics` 不顯示加量進度；之後需要時另開 change。
- 不升 store 格式版本：新欄位直接加進格式版本 1。舊版 client 會忽略它們，對舊版 client 而言恢復的 backend 仍立即全量。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `service-config`：新增 `slow_start` 設定與驗證規則；`[defaults]` 可設定它。
- `state-store`：格式版本 1 的服務記錄新增 `slow_start_ms`，backend 記錄新增 `recovered_at`，並規定何時寫入。
- `backend-selection`：新增有效權重的定義，以及 daemon 在加量期間如何依有效權重挑選。
- `client-lookup`：新增 client 在加量期間如何依有效權重挑選。
- `dns-server`：SRV 回答在加量期間的 weight。

## Impact

- 受影響的 spec：service-config、state-store、backend-selection、client-lookup、dns-server
- 受影響的程式碼：
  - crates/emfor-format/src/lib.rs：`ServiceRecord.slow_start_ms`、`BackendRecord.recovered_at`
  - crates/emfor-format/src/rules.rs：共用的有效權重計算
  - crates/emfor-daemon/src/config.rs：`slow_start` 設定與驗證
  - crates/emfor-daemon/src/writer.rs：轉變時寫入 `recovered_at`
  - crates/emfor-daemon/src/select.rs：A/AAAA 與 SRV 依有效權重挑選
  - clients/rust/src/select.rs、clients/rust/src/client.rs：client 依有效權重挑選
  - tools/emfor-fixture/src/lib.rs：相對時間的 `recovered_at`
  - tests/conformance/v1/cases.json、tests/conformance/v1/README.md
  - docs/manual/03-configuration.md、docs/manual/05-backend-selection.md、docs/manual/07-dns.md（SRV weight）與對應的 docs/manual.zh_TW 章節、examples/emfor.toml、CHANGELOG.md
