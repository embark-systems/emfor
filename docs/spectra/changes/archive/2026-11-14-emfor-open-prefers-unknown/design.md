## Context

候選規則由 `crates/emfor-format/src/rules.rs` 的 `healthy_or_all` 決定，daemon 的 A、AAAA、SRV 回答與所有 client 都用它。目前規則依序是：

1. 通過的 backend（`healthy` 且 `reason` 為 `null`）；
2. 健康的 backend（包含正在失敗的）；
3. `on_all_down`：`open` 用全部 backend，`closed` 為 all down。

`trust_health` 為 false（store 過期）時，前兩層是空的。

backend 的 `unknown` 狀態來自 `crates/emfor-daemon/src/state.rs` 的 `BackendHealth`：
- 啟動、新增或 reload 重設時從 `unknown` 開始，連續 `rise` 次 `up` 轉為 `healthy`，連續 `fall` 次 `down` 轉為 `unhealthy`；
- 只有 `error` 持續超過 `fall × interval` 時，會退回 `unknown`。

`reason` 在最近一次結果為 `down` 或 `error` 時有值，`up` 後為 `null`。`unknown` 的 backend 也有 `reason`。

## Goals / Non-Goals

**Goals:**

- `open` 且沒有健康 backend 時，先用還沒確定壞掉的 `unknown` backend，再退到全部。
- `unknown` 之間，最近一次檢查通過的優先。
- client 與 DNS 對同一份狀態給出相同的候選。

**Non-Goals:**

- 不改 `on_all_down = "closed"`：沒有健康 backend 時仍回 `ALL_DOWN` 或 SERVFAIL，即使有 `unknown` 的 backend。
- 不改 store 過期時的行為：全部 backend 都是候選。
- 不改 store 格式、client API、`all_down` 欄位、`/metrics`、`emfor status`。
- 不改 slow start、各 `balance` 方法的挑選方式、SRV 的 priority 編法。
- 不新增設定選項來關閉這個行為。

## Decisions

### unknown 優先的兩層放在 healthy_or_all 的 open 分支

`open` 分支在回傳全部 backend 之前，先找 `unknown` 且 `reason` 為 `null` 的，再找全部 `unknown` 的。`trust_health` 為 false 時跳過這兩層，直接回全部 backend。函式簽名不變。

考慮過的替代：讓 `closed` 也用 `unknown` backend。這會改變 `closed`「沒有健康就失敗」的語意，使用者沒有要求，所以不採用。

### unknown 之間依 reason 是否為 null 分層

沿用 emfor-avoid-failing 對「通過」的判斷：最近一次結果是 `up` 的 `unknown` backend 優先。

考慮過的替代：所有 `unknown` 一視同仁。規則較簡單，但最近一次檢查失敗的 `unknown`（例如剛加入就連不上）會和正在通過的一起被輪到。使用者選擇了分層。

### store 過期時不分層

store 過期時，client 把每個 backend 都視為 `unknown`，`state` 與 `reason` 都可能是舊的。所以 `trust_health` 為 false 時維持「全部 backend」，與現在相同。

## Implementation Contract

**行為**

- store 新鮮、`on_all_down = "open"`、沒有健康 backend 時，client 查詢與 DNS 的 A、AAAA、SRV 回答依序在下列 backend 中挑選：
  1. `unknown` 且 `reason` 為 `null` 的；
  2. 沒有時，全部 `unknown` 的；
  3. 再沒有時，全部 backend。
- A、AAAA 依位址家族分開套用這條規則；SRV 對兩種家族一起套用。
- daemon 剛啟動時全部都是 `unknown`：第 1 層為空時退到第 2 層，結果等於全部 backend，與現在相同。
- 有健康 backend、`closed`、store 過期這三種情況：與現在相同。

**介面與資料**

- `rules::healthy_or_all(on_all_down, list, trust_health, backend) -> Option<Vec<T>>` 簽名不變，doc comment 改寫成五層。
- store 格式、C API、各語言 wrapper 的 API 都不變。

**失敗模式**

- 沒有新的錯誤。`open` 在 `list` 非空時一定有候選；`list` 為空時仍回 `None`（all down）。

**驗收**

- `cargo test -p emfor-format`：`healthy_or_all_table` 涵蓋 client-lookup 範例表「candidates by state, reason and heartbeat」的每一列。
- `cargo test -p emfor-daemon --lib select`：`candidate_sets_example` 涵蓋 backend-selection 範例表的每一列；新增 SRV 測試對應 scenario「All down open with an unknown backend」。
- `cargo test -p emfor-client --all-features`：conformance 新增與修改的案例在 Rust API 與 C API 都通過。
- Debian 容器的 `make -C clients test`：C、Go、Python、Perl、PHP 都是 0 個失敗。
- 暫時拿掉新的兩層時，上述新測試與修改過的測試要失敗。
- 依 RELEASING.md 的檢查在 macOS 與 Debian 通過。

**範圍**

- 範圍內：
  - `healthy_or_all` 與它的測試；
  - daemon 與 client 的候選測試；
  - conformance 資料；
  - 手冊第 1、5、7 章（英文與中文）、`docs/deploy/dns-resolver.md`、CHANGELOG。
- 範圍外：`closed` 的語意、store 格式、`emfor status`、`/metrics`、新設定選項。

## Risks / Trade-offs

- [升級期間舊版 client 仍從全部 backend 中挑] → 新舊行為都是合法的 `open`，CHANGELOG 說明這點。
- [一台 `unknown` backend 可能其實也壞了，例如剛加入就連不上] → 第 2 層只在沒有通過的 `unknown` 時才用到它，而且最多 `fall` 次檢查後它就會轉為 `unhealthy`，回到第 3 層。
- [只剩一台 `unknown` 時，所有流量都集中到它] → 其他 backend 都已確定壞掉，集中到最有機會成功的一台，正是 `open` 的用意。
