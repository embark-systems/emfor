## Context

- 挑選分兩段。第一段 `rules::healthy_or_all` 決定候選；第二段各 `balance` 方法從候選中挑一台：
  - client：`clients/rust/src/select.rs` 的 `pick` 與 `weighted_index`，用程序內的輪流計數器；
  - daemon 的 DNS：`crates/emfor-daemon/src/select.rs` 的 `select`，A/AAAA 的 `weighted` 用 `smooth_weighted`，其他方法用計數器；SRV 用 `srv_targets`。
- daemon 在 `schedule.rs` 算出狀態轉變（`Transition { from, to }`），隨 `BackendUpdate` 送到 `writer.rs` 的 `update`。`update` 更新記錄、交給 `live` 給 DNS 使用，並寫入歷史。依 state-store spec，狀態轉變會立即 commit 到 store。
- reload 時，沒有被重設（`ServiceLayout.reset` 為 false）的 backend 保留原本的記錄欄位；被重設的 backend 從 `unknown` 重新開始。
- client 與 daemon 跑在同一台主機，讀的是同一個時鐘。
- store 格式版本 1 曾多次直接新增欄位（`weight`、`site`、`latency_us`、`pool`），讀取端會忽略不認得的欄位。
- conformance 的 store 由 `emfor-fixture` 依案例寫出；`heartbeat` 已經支援相對於現在的寫法，backend 欄位則照案例原樣寫入。

## Goals / Non-Goals

**Goals:**

- 從 `unhealthy` 恢復的 backend，在 `slow_start` 期間依經過時間逐步拿回流量。
- client 與 DNS 用同一個有效權重定義，結果一致，並由 conformance 測試資料約束 client 端。
- 沒有 backend 在加量時，所有行為與現在完全相同（包括 SRV 的 weight）。

**Non-Goals:**

- 不對 `failover`、有 key 的 `sticky`、daemon 啟動、reload 新增的 backend 加量。
- 不提供加量曲線、級數或最低比例的設定。
- 不在 `emfor status` 或 `/metrics` 顯示加量進度。
- 不升 store 格式版本。

## Decisions

### daemon 記錄恢復時間，挑選端自己算進度

backend 從 `unhealthy` 轉為 `healthy` 時，daemon 寫入 `recovered_at` = 該次檢查的 `checked_at`（Unix 毫秒），服務記錄帶 `slow_start_ms`。client 與 DNS 用「現在 − `recovered_at`」自己算加量級數。

- 這樣 daemon 每次恢復只寫一次 store，加量期間不用每秒重寫記錄。
- client 和 daemon 在同一台主機，時鐘相同。
- 考慮過的替代：daemon 直接把有效權重寫進 store。這要在加量期間不斷重寫服務記錄，而且 client 讀到的值最多會晚一次 heartbeat，所以不採用。

### recovered_at 的寫入規則

在 `writer.rs` 的 `update` 依 `transition` 設定：

- `unhealthy` → `healthy`：`Some(checked_at)`；
- 其他任何轉變：`None`，包括 `unknown` → `healthy`，以及離開 `healthy`；
- 沒有轉變：保留原值。

reload 沒有重設的 backend 保留 `recovered_at`，被重設的 backend 從 `unknown` 開始，值為 `None`。`slow_start_ms` 隨服務設定寫入，reload 改了它就立即套用，連正在加量中的 backend 也照新值計算。

### 有效權重：10 級線性加量

級數 `step`：

- `recovered_at` 為 null、`slow_start_ms` 為 0、現在早於 `recovered_at`（時鐘被往回調）、或經過時間 ≥ `slow_start_ms`：`step` = 10。
- 其他情況：`step` = max(1, ceil(10 × 經過毫秒 / `slow_start_ms`))，最大 10。

有效權重 = 基礎權重 × `step`。基礎權重在 `weighted` 是 backend 的 `weight`，其他方法是 1。「正在加量」指 `step` < 10。

- 固定 10 級：DNS 的 `smooth_weighted` 在候選的權重清單改變時會重設狀態；分成 10 級，一次加量最多重設 10 次，分配比例仍然準確。考慮過連續的比例，但那會讓 `smooth_weighted` 幾乎每次挑選都重設，所以不採用。
- 最低 10%：一恢復就有少量流量，讓健康檢查以外的真實請求也能暖機。
- 時鐘往回調時直接全量：寧可提早全量，也不要讓 backend 卡在低流量直到時鐘追上。

計算放在 `crates/emfor-format/src/rules.rs`，例如 `fn effective_weight(b: &BackendRecord, base: u32, slow_start_ms: u64, now_ms: u64) -> u32`，daemon 與 client 共用。

### 加量期間改用加權挑選，其他時候不變

在第一段候選（以及 `locality`、`fastest` 縮小後的候選）中，只要有一台 `step` < 10：

- client：`round-robin`、`locality`、`fastest`、沒有 key 的 `sticky`、`weighted` 都用 `weighted_index` 的累積權重方式，權重換成有效權重。
- daemon A/AAAA：上述方法都用 `smooth_weighted`，權重換成有效權重。這會讓非 `weighted` 的方法在加量期間共用 `FamilyChoice` 裡的 smooth weighted 狀態；它在權重清單改變時本來就會重設。
- SRV：weight 欄位填有效權重（`weighted` 為 `weight` × `step`，其他方法為 `step`），priority 與順序規則不變。

沒有任何一台在加量時，走原本的程式路徑，結果與現在逐位元相同。`failover` 與有 key 的 `sticky` 永遠走原本的路徑。

考慮過讓 `round-robin` 跳過一定比例的挑選。但這需要額外的亂數或計數器，client 與 DNS 也難以一致，所以不採用。

### store 過期時不加量

client 在 store 過期時不信任健康狀態，候選由 `on_all_down` 決定。這時把每台都當作 `step` = 10，以免依據舊的 `recovered_at` 改變比例。

### conformance 測試用相對時間

`emfor-fixture` 接受 backend 物件中的 `recovered_ago_ms`（只存在於測試資料中），寫入時換成 `recovered_at` = 現在 − 該值，並移除這個鍵。案例的 `slow_start_ms` 取 100000、`recovered_ago_ms` 取 25000，這時 `step` = 3；測試執行的延遲要超過 5 秒才會跨到下一級。

## Implementation Contract

**行為**

- 設定 `slow_start = "30s"` 的服務，某台 backend 從 `unhealthy` 轉為 `healthy` 後：
  - 前 3 秒的有效權重是其他台的 1/10；
  - 第 27 到 30 秒是 10/10；
  - 30 秒後完全回到原本的挑選方式。
- client 與 DNS 的結果一致。
- `slow_start` 未設定或為 `"0s"`、daemon 剛啟動、reload 新增的 backend、`failover`、有 key 的 `sticky`、store 過期：行為與現在相同。

**介面與資料**

- 設定：`slow_start`（服務表格與 `[defaults]`），持續時間字串，`"0s"` 或 1s 到 1h，預設 `"0s"`。不合法時，錯誤訊息以該 TOML 路徑開頭，例如 `services."api.svc.emfor".slow_start: must be 0s or from 1s to 1h`。
- store：
  - 服務記錄新增 `slow_start_ms`（非負整數，關閉為 0），缺少時讀作 0；
  - backend 記錄新增 `recovered_at`（Unix 毫秒或 `null`），缺少時讀作 `null`。
- Rust：`ServiceRecord.slow_start_ms: u64`（serde default 0）、`BackendRecord.recovered_at: Option<u64>`（serde default）。
- 共用函式 `rules::effective_weight`，以及判斷「有沒有候選正在加量」的輔助函式。

**失敗模式**

- 時鐘往回調：當作加量結束（全量），不報錯。
- 舊版 client 讀到新欄位：忽略，恢復的 backend 立即全量；手冊的相容性說明會寫出這點。

**驗收**

- 單元測試：`rules::effective_weight` 的級數表（經過 0、1、2.5、29.9、30、40 秒，`slow_start` 30 秒；`recovered_at` null；`slow_start_ms` 0；時鐘往回調）。
- daemon：
  - `config.rs` 的驗證測試：`"0s"`、`"500ms"`、`"1s"`、`"1h"`、`"2h"`；
  - `writer.rs` 的測試：`recovered_at` 在 `unhealthy`→`healthy` 時寫入，在 `unknown`→`healthy` 與 `healthy`→`unhealthy` 時為 `null`；
  - `select.rs` 的測試：加量中的 round-robin 與 weighted 比例，以及 SRV weight。
- client 與 conformance：
  - client：`select.rs` 的加量比例測試；
  - `cases.json` 新增案例，經由 Rust、C API 與所有 wrapper 驗證：round-robin 一台加量中、weighted 加量中、failover 不受影響、store 過期不加量、加量已結束。
- 每個新測試在拿掉加量（`step` 固定為 10）時要失敗。
- 依 RELEASING.md 的檢查在 macOS 與 Debian 通過。

**範圍**

- 範圍內：上列 spec、程式碼、fixture、conformance 資料、手冊第 3、5 與 7 章（英文與中文；第 7 章只補 SRV weight）、`examples/emfor.toml`、CHANGELOG。
- 範圍外：`emfor status`、`/metrics`、加量參數的設定、store 格式版本號。

## Risks / Trade-offs

- [服務只有一台 backend 時，它加量中仍是唯一候選] → 比例只在多台之間有意義，單台照樣拿全部流量。手冊會說明這點。
- [全部 backend 同時恢復（例如共用的資料庫恢復）] → 全部有效權重相同，比例不變，等於沒有加量。手冊會說明 slow start 只在部分 backend 恢復時有效果。
- [舊版 client 不認得新欄位] → 不加量，但仍然正確挑選健康的 backend；手冊的相容性說明會寫出這點。
- [加量期間 round-robin 改走加權路徑，DNS 的 A/AAAA 回答順序與平常不同] → 只在加量期間發生，比例符合有效權重。
