## Context

emfor 要讓 backend 暫時不接新流量，現在只能把它從設定檔拿掉。已有的先例是 `backup`：backend 表格的鍵，寫進 backend 記錄，reload 時由 writer 套用（`crates/emfor-daemon/src/writer.rs` 在 reload 時把 `weight`、`site`、`backup` 從設定抄到記錄上，健康狀態沿用），client、DNS 與 `emfor lookup --explain` 共用 `crates/emfor-format/src/rules.rs` 的 `explain_tiers` 決定候選。`/spectra-discuss`（2026-11-16，advisor 參與，相容性與程式位置由子代理初查後我逐項核對）確認了這些事實，都是讀原始碼的結果，沒有執行測試：

- 所有語言的 client（C、Go、Python、Perl、PHP）都包裝同一份 `libemfor`（Rust 的 `emfor-client`），自己不解析 `state`。`HealthState` 有 `#[serde(other)] Unrecognized`，不認得的 `state` 當 `unhealthy`、不讓 lookup 失敗。
- `SCHEMA_VERSION` 是 2，可讀 1 與 2。state-store 規格寫明：新增一個舊讀者不能當 `unhealthy` 處理的 `state` 值，必須提高 `meta/schema_version`；schema 3 的 store 舊 client 回 `INCOMPATIBLE`。選用欄位則可以直接加（記錄的讀者要忽略不認得的欄位）。
- `ServiceRecord` 在 daemon 記憶體裡就是寫進 store 的那一份：writer 在記憶體裡更新它，`start_commit` 用 `serde_json::to_string(rec)` 寫出，DNS 與 metrics 讀的 live 記錄是它的複本。
- 檢查任務的存續由 `ServiceConfig::check_changed`（`checker_id` 與 `check_options`）決定，backend 表格其他鍵（`weight`、`site`、`backup`）的改變不會重啟檢查任務，狀態沿用。`drain` 比照辦理。
- `refresh_all_down`（`emfor-format`）只算 `Healthy`；`fast_pool`（`state.rs`）的範圍是 `Healthy` 且有延遲；`BackendCounters::STATES`（`live.rs`）是固定 3 個值的陣列，metrics 的 `emfor_backend_state` 與 `emfor_backend_transitions_total` 都依它列出。
- 服務定義 digest 由 `check_digest(checker_id, check_options, timeout)` 算，與 backend 表格無關。

## Goals / Non-Goals

**Goals:**

- 設定檔把某台 backend 標成 `drain = true`，reload 後所有新的查詢（client、DNS）都不會挑到它，所有舊 client 也不會失敗。
- 檢查照跑，取消 drain 時立刻回到真實狀態。
- 維運在 `emfor status`、`lookup --explain` 與 metrics 看得出哪些 backend 已 drain，以及它們的真實檢查狀態。

**Non-Goals:** 見 proposal：不做執行期 drain、不同步給 peers、不新增 `state` 值、不升 schema、不改 `weight` 範圍、不做保留 persistence 的模式。

## Decisions

### 加欄位，不加 `state` 值，不升 schema

backend 記錄新增選用欄位 `drain: bool`（`#[serde(default)]`，缺少讀作 `false`）。已 drain 的 backend 以 `state = "unhealthy"`、`reason = "drained"` 寫入，舊 client 看到的就是一台不健康的 backend，不需要升級也不會 `INCOMPATIBLE`；新 client 看到 `drain = true` 就在所有層級排除它。

替代方案一：新的 `state = "drained"` 並把 schema 升到 3。語意最乾淨，但每個舊 client 立刻回 `INCOMPATIBLE`，叢集必須先升完所有主機的 `libemfor` 才能啟用，等於第一次破壞相容性，只為了一個維護功能，不值得。替代方案二：drain 的 backend 直接不寫進 store。完全相容，但 `emfor status` 與 `lookup --explain` 看不到它，維運分不出「被 drain」與「被刪掉」，也沒有地方顯示真實狀態。

已知代價：舊 client 在「全部 backend 都不健康且 `on_all_down = "open"`」的最後一層仍會把已 drain 的 backend 當一般的不健康 backend 挑出來。手冊與 `docs/compatibility.md` 要寫明；叢集裡所有 client 升級之後這個差異就消失。

### 真實狀態放在記憶體記錄，寫入 store 時才轉成「對外的樣子」

daemon 的記憶體記錄（也是 DNS 與 metrics 讀的 live 記錄）永遠保留檢查得出的真實 `state` 與 `reason`，加上 `drain`。只有在 `start_commit` 寫進 store 時，對 `drain = true` 的 backend 轉成：`state = "unhealthy"`、`reason = "drained"`、`drain = true`，並新增選用欄位 `checked_state`（寫真實的 `state`，`drain` 為假時不寫，`skip_serializing_if`）。取消 drain 時，記憶體裡的真實狀態直接寫回去，不需要重新累計 `rise`，也不是狀態轉換，所以 drain 與取消 drain 本身不觸發 slow start、不寫進歷史；drain 期間才從不健康恢復成健康的 backend，`recovered_at` 照平常記錄，取消 drain 時若還在 `slow_start` 內就照平常慢啟動。

為什麼不把真實狀態只放在 `reason`：`reason` 是自由文字，client 與 `status` 不該解析它。為什麼不讓 `status` 從 daemon 取得：`status` 只讀 store，這是它的規格（`Status reads the store read-only`）。

取捨：drain 期間真實的失敗原因（真實的 `reason`）不寫進 store，`status` 只能顯示「已 drain，檢查結果為 healthy／unhealthy／unknown」；真實原因仍在 daemon 的記憶體，取消 drain 後恢復。可以接受：要看原因，取消 drain 或看日誌。

### 排除放在 `explain_tiers`，一處決定，client、DNS 與 explain 共用

在 `explain_tiers` 開頭把 `drain = true` 的 backend 排除，不論是哪一層（包含 `on_all_down = "open"` 的最後一層 `Tier::All`），並新增 `Exclusion::Drained`，`emfor lookup --explain` 因此顯示「drained」。因為 `healthy_or_all` 與 DNS 的候選都走 `explain_tiers`，不需要另外在每個分配方式裡處理；所有分配方式（含 `sticky` 與 `failover`）自然不會挑到它。`fast_pool` 的範圍同樣排除 `drain`，所以快組不含它；`refresh_all_down` 把 `drain` 的 backend 當作不健康，`all_down` 因此與「還有沒有東西可以挑」一致。

排除是在記憶體記錄與 store 記錄上都生效：daemon 內部（DNS）用記憶體記錄的 `drain` 旗標，client 與 `status` 用 store 記錄的 `drain` 旗標，結果相同。

### 全部 backend 都已 drain

候選為空：`lookup` 回 `ALL_DOWN`，DNS 回 SERVFAIL，不論 `on_all_down`。這不是「照現有規則當作它不存在」：`open` 的意思是「全部不健康時照樣挑」，但這時沒有任何 backend 可以挑，drain 的意思就是不接新流量。設定驗證不禁止全部 drain（和 `backup` 不同，「至少一台不是 backup」是因為備援需要主要 backend 才有意義；全部 drain 是一個明確的維護動作）。

### 設定與 reload

`backends` 的表格形式新增 `drain`（布林，預設 `false`）；非布林是驗證錯誤，與 `backup` 同一種寫法與錯誤訊息。`Backend` 新增 `drain` 欄位；`writer.rs` reload 時比照 `backup` 抄到記錄上，狀態沿用。`drain` 不屬於 `check_changed`，所以檢查任務不重啟、狀態與延遲沿用。

### 只在本機，不進 digest，不給 peers

drain 是這台 daemon 的設定。不傳給 peers，不進 `check_digest`，所以各台主機可以各自 drain，不會被判定為服務定義不同；peers 對這台 backend 的檢查結果與投票照常交換與計算（檢查仍在跑）。

### 看得見

- `emfor status`：文字輸出在 backend 那行標示 `drained`，並顯示 `checked_state`；`--json` 新增 `drain` 與 `checked_state` 欄位（既有欄位不變，`--json` 的規格本來就是只增不減）。
- `emfor lookup --explain`：排除原因 `drained`；`--json` 同樣。
- metrics：`emfor_backend_state` 與 `emfor_backend_transitions_total` 繼續依 `BackendCounters::STATES` 回報真實的檢查狀態（讀記憶體記錄，所以是真實狀態），不新增狀態標籤，不改既有指標的意義。新增一個 gauge `emfor_backend_drain{service,backend}`，值 0 或 1，每台 backend 都輸出（與 `emfor_backend_state` 一致，方便寫告警規則）。

## Implementation Contract

**Behavior**

- `drain = true` 的 backend：不被任何分配方式、任何候選層、DNS A、AAAA、SRV 與 `fastest` 快組挑出；其他 backend 的行為與現在完全相同；沒有設定任何 `drain` 時，所有輸出與 store 內容和現在相同（沒有新欄位出現）。
- 全部 backend 已 drain：`lookup` 回 `ALL_DOWN`、DNS SERVFAIL，不論 `on_all_down`。
- 檢查照跑；取消 drain（reload）後，backend 立刻回到真實狀態，沒有狀態轉換紀錄；drain 與取消 drain 本身不啟動 slow start（drain 期間才恢復的 backend 照平常從那次恢復起慢啟動）。
- store 記錄：已 drain 的 backend 是 `state = "unhealthy"`、`reason = "drained"`、`drain = true`、`checked_state = <真實狀態>`；沒有 drain 的 backend 沒有 `drain` 與 `checked_state` 欄位（不寫 `false`，欄位缺少即為假），所以未使用 drain 的 store 內容逐位元組不變。`all_down` 不把已 drain 的 backend 算健康。
- 舊 client 讀到已 drain 的 backend 時把它當一般的不健康 backend（最後一層可能挑到它，見上）。

**Interface / data shape**

- 設定：`{ addr = "10.0.0.11:8080", drain = true }`。
- `BackendRecord`：`drain: bool`（`#[serde(default, skip_serializing_if = "std::ops::Not::not")]`）、`checked_state: Option<HealthState>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`）。
- `Exclusion::Drained`，說明文字 `drained`。
- `emfor status --json`：backend 物件新增 `drain`、`checked_state`（僅 drain 時出現）。
- metrics：`emfor_backend_drain{service,backend}`，gauge，0 或 1。

**Failure modes**

- `drain` 不是布林：設定驗證錯誤，路徑是 `services."x".backends[n].drain`，訊息 `drain must be true or false`。
- 沒有新的執行期失敗模式；全部 drain 是上面的 `ALL_DOWN`。

**Acceptance criteria**

- 規則表測試：每一種 balance 方式、每一層、`on_all_down` 兩種值、心跳過期，drain 的 backend 都不出現（`explain_tiers` 與 `healthy_or_all` 的測試涵蓋）。
- 整合測試：設定 `drain = true`、reload 後 store 記錄的內容、DNS 答案不含它、取消 drain 後恢復且狀態沒有重置。
- 現有的設定、writer、選擇、status、lookup、metrics 測試不修改仍通過。
- `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features` 在 Debian docker（`tools/docker-test.sh`）通過；`tools/check-manual-links.py` 通過；macOS 與 FreeBSD 等發版前再測。

**Scope boundaries**

- 範圍內：`emfor-format`、`emfor-client`、`emfor-daemon` 裡上列各處、七個規格、手冊第 3、5、8、10 章（中英）、`docs/deploy/daemon.md`、`docs/compatibility.md`、`examples/emfor.toml`、CHANGELOG。
- 範圍外：執行期 drain、peers 同步、新 `state` 值、schema 升版、`weight = 0`、有效設定傾印。

## Risks / Trade-offs

- **舊 client 的最後一層**：見上，寫進文件。叢集升級完就消失。
- **真實 `reason` 在 drain 期間不進 store**：見上。
- **記憶體記錄與 store 記錄不同**：只有 `start_commit` 轉換，轉換函式有單元測試（drain 與非 drain 的 backend 各一，且確認記憶體記錄不被修改）。
- **新的選用欄位被舊的寫入端覆蓋**：不存在，store 在每次啟動重建，只有 daemon 寫入。
- **`emfor-format` 的 `READABLE_SCHEMA_VERSIONS` 不變**：以測試守住 `SCHEMA_VERSION` 仍是 2。
