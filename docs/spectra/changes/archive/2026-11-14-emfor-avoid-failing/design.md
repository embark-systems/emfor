## Context

所有挑選都從同一個候選規則開始：`crates/emfor-format/src/rules.rs` 的 `healthy_or_all`。目前的規則是：

- 有 `healthy` 的 backend 時，候選就是這些 backend；
- 沒有時由 `on_all_down` 決定：`open` 用全部 backend，`closed` 回「all down」；
- store 過期時（`trust_health` 為 false），沒有任何 backend 算健康。

這個規則有三個使用者：

- client 的 `candidates`（`clients/rust/src/select.rs`），C library 與各語言 wrapper 都經由它；
- daemon 的 A/AAAA 候選（`crates/emfor-daemon/src/select.rs` 的 `candidate_backends`，每個 IP 版本各算一次）；
- daemon 的 SRV 候選（同一檔案的 `srv_targets`，兩個 IP 版本合併算）。

選出候選之後，`locality`、`fastest` 的偏好（`rules::preferred` 與 `subset_or_all`）以及各 `balance` 的挑法才套用上去。

store 中每個 backend 的 `reason` 記錄最近一次 `down` 或 `error` 結果的訊息，`up` 之後為 `null`（state-store spec）。`reason` 的變動最晚在下一次 heartbeat 寫入 store，也就是最多約 1 秒。daemon 的 DNS 直接讀記憶體中的同一份記錄，所以沒有這段延遲。

## Goals / Non-Goals

**Goals:**

- 健康但最近一次檢查失敗的 backend，在還有其他通過的 backend 時，不被 client 查詢或 DNS 回答選到。
- client 與 DNS 用同一條規則，並由共用的 conformance 測試資料約束 client 端。
- store 格式與 client API 不變；新的 client 讀舊 daemon 的 store 也照新規則運作。

**Non-Goals:**

- 不區分 `down` 與 `error`，也不新增結果種類欄位。
- 不給 SRV 的正在失敗 backend 較低的 priority。
- 不提供關閉這個行為的設定。
- 不做 client 端的被動健康檢查（回報連線失敗）。

## Decisions

### 正在失敗的定義用 reason 是否為 null

正在失敗的 backend：`state` 為 `healthy` 且 `reason` 不是 `null`。只看最近一次結果，不計次數，也不看 `checked_at`：只要下一次 `up` 把 `reason` 清成 `null`，它就回到通過。

考慮過的替代：新增 `last_result` 欄位以區分 `down` 與 `error`。這要改 store 格式，舊 daemon 寫的 store 也用不到，所以不採用。`error`（例如 plugin 逾時）也算正在失敗。若是整個 checker 出問題，所有 backend 都會同時是 `error`，這時退到第二層，等於沒有影響。若只有單一 backend 回 `error`，它會暫時讓位，直到下一次 `up`。

### 三層候選放在共用的 healthy_or_all

規則在 `healthy_or_all` 內實作，順序是：通過的 → 健康的 → `on_all_down`。daemon 與 client 不需要各自修改挑選程式。`trust_health` 為 false 時，第一、二層都是空的，行為不變。

考慮過的替代：只改 client。這會讓 DNS 與 client 對同一份狀態給出不同答案，違反 backend-selection 與 client-lookup 一致的設計，所以不採用。

### SRV 與 A/AAAA 用同一條規則

SRV 的候選同樣排除正在失敗的 backend（有通過的 backend 時），priority 編法不變。考慮過給它們 priority + 1，但這會讓每種 `balance` 的 priority 規則多一個維度，DNS client 也不一定照 priority 嚴格處理，因此不採用。

### balance 方法不變，只換候選

- `weighted`：daemon 的 smooth weighted round-robin 在候選清單改變時本來就會重設；client 的累積權重以新候選計算。
- `sticky`：rendezvous hashing 只在候選中計算。原本對到正在失敗 backend 的 key，暫時改對到別台；它恢復通過後，key 又回到原來的 backend。這正是預期的行為。
- `locality`、`fastest`：在新的候選中套用偏好。`fastest` 的 pool 由 daemon 決定，不因此改變。

## Implementation Contract

**行為**

- store 新鮮時，client 查詢與 DNS 的 A、AAAA、SRV 回答，只在通過的 backend 中挑選；沒有通過的 backend 時，在健康的 backend 中挑選；都沒有時由 `on_all_down` 決定（`open` 用全部 backend，`closed` 回 `ALL_DOWN` 或 SERVFAIL）。
- store 過期（heartbeat 為 0、超過 `EMFOR_MAX_STALENESS`，或不存在）時，`reason` 不影響結果，與現在相同。
- 對呼叫端而言，結果不會顯示是否來自第二層（全部正在失敗）。這與「結果是否來自 all-down 服務」的處理相同。

**介面與資料**

- `rules::healthy_or_all(on_all_down, list, trust_health, backend)` 的簽名不變，回傳值改為三層規則的結果。
- store 格式、`emfor_lookup` 的 C ABI、各 wrapper API 都不變。
- `tests/conformance/v1/cases.json` 新增案例，格式不變（`reason` 已經是 backend 物件的欄位）。

**失敗模式**

- `reason` 寫入 store 最多晚約 1 秒；這段期間 client 仍可能挑到剛開始失敗的 backend。DNS 讀記憶體，沒有這個延遲。這段延遲是刻意接受的。

**驗收**

- `crates/emfor-format/src/rules.rs` 的 `healthy_or_all` 單元測試涵蓋：
  - 通過與正在失敗並存；
  - 全部正在失敗；
  - 全部不健康（open 與 closed 兩種）；
  - store 過期。
- `crates/emfor-daemon/src/select.rs` 的測試：A/AAAA 的 `select` 與 `srv_targets` 在通過與正在失敗並存時不回正在失敗的 backend；全部正在失敗時照常回答。
- `cases.json` 的新案例由 Rust、C API 與 `emfor-conformance` 跑過的所有 wrapper 通過：
  - `failover` 跳過正在失敗的 backend；
  - `round-robin` 只輪流通過的 backend；
  - 全部正在失敗時照常輪流；
  - store 過期時 `reason` 不影響結果。
- 每個新測試都要先確認：暫時拿掉新的一層時它會失敗。

**範圍**

- 範圍內：`rules.rs`、daemon 與 client 的測試、conformance 資料、兩份手冊的 05 章、CHANGELOG。
- 範圍外：store 格式、`emfor status`、`/metrics`、新設定、新 client API。

## Risks / Trade-offs

- [單一 backend 的 checker `error` 讓它暫時讓位] → 下一次 `up` 立刻恢復；這比把流量送到可能故障的 backend 好。
- [只剩一台通過時，流量全部集中到它] → 與現在 `fall` 到達後的情況相同，而且只持續到其他 backend 下一次 `up` 或轉為不健康為止。
- [`sticky` 的 key 在 backend 失敗期間改對到別台] → 屬於預期行為，寫進手冊。
- [client 有約 1 秒的 `reason` 延遲] → 寫進手冊；需要更快反應的服務可以縮短 `interval`。
