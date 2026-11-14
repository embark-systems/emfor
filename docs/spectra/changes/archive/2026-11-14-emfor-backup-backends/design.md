## Context

候選規則在 `crates/emfor-format/src/rules.rs` 的 `rules::healthy_or_all`，daemon 的 A、AAAA、SRV 回答（`crates/emfor-daemon/src/select.rs`）與 Rust client（`clients/rust/src/select.rs` 的 `candidates`）共用它。C library 與 Go、Python、Perl、PHP wrapper 都經由 Rust client。目前的層次是：通過的 → 健康的 → `on_all_down`（`open` 時 `unknown` 優先，見 emfor-open-prefers-unknown）。

store 格式版本是 `emfor_format::SCHEMA_VERSION`（目前為 1），寫在 `meta/schema_version`。有兩個地方要求它等於 `SCHEMA_VERSION`，否則報錯：
- client：`clients/rust/src/client.rs`，回 `INCOMPATIBLE`；
- `emfor status`：`crates/emfor-daemon/src/status.rs`，exit 2。

backend 設定是 `crates/emfor-daemon/src/config.rs` 的 `Backend`（`id`、`addr`、`weight`、`site`）。writer 的 `fresh_backend` 把 `weight`、`site` 寫進 store 記錄。

## Goals / Non-Goals

**Goals:**

- 標成 `backup = true` 的 backend 只在沒有通過的主要 backend 時接手。
- 不認得 `backup` 的 client 不會把流量送到備援：靠格式版本 2 讓它們回 `INCOMPATIBLE`。
- 新版 client 與 `emfor status` 能讀舊 daemon 的版本 1 store，讓 client 可以先升級。

**Non-Goals:**

- `emfor status` 的文字輸出不標示備援 backend（JSON 輸出若直接帶出記錄，會自然包含 `backup`）。
- `/metrics` 不新增指標，`all_down` 的意義不變（仍是「沒有任何健康的 backend」，備援也算）。
- 不做「只有用到 `backup` 時才寫版本 2」：使用者選擇一律寫版本 2。
- 沒有健康 backend、`open` 時，不區分主要與備援。
- SRV 不用 priority 表示備援。
- 不改 `failover`、`locality`、`fastest`、`weighted`、`sticky`、slow start 的挑選方式；它們都在選出的那一層裡運作。唯一的調整是 `fastest` 的 pool 改為主要與備援分開計算（見決策「fastest 的 pool 依層分開計算」），client 端不變。

## Decisions

### 備援層插在通過與正在失敗之間

候選順序：
1. 通過的主要 backend；
2. 通過的備援 backend；
3. 健康的主要 backend（含正在失敗的）；
4. 健康的備援 backend（含正在失敗的）；
5. `on_all_down`。

主要 backend 一開始失敗（`reason` 有值），就先改用通過的備援，不必等它轉為 `unhealthy`。

考慮過的替代：主要 backend 全部 `unhealthy` 才用備援。這樣要多等約 `fall × interval`，期間仍會挑到正在失敗的主要 backend。使用者選擇了「主要的都沒通過時」。

### 格式版本升到 2，讀取端接受 1 與 2

`SCHEMA_VERSION` 改為 2，daemon 一律寫 2。讀取端（client 與 `emfor status`）接受 `1` 與 `2`，`1` 的 `backup` 讀作 `false`。版本判斷集中在 `emfor_format` 的一個函式（例如 `readable_schema(version: &str) -> bool`），兩個讀取端共用；錯誤訊息改成 `this client reads versions 1 and 2` 與 `this emfor reads versions 1 and 2`。

考慮過的替代：
- 版本維持 1、只新增欄位：舊 client 會把備援當一般 backend 用，悄悄把流量送到備援。
- 只在設定了 `backup` 時寫 2：不會影響沒用備援的部署，但規則較複雜。

使用者選擇一律升到 2，代價是升級順序必須是 client 先、daemon 後。

### backup 是設定值，不影響健康狀態

`backup` 和 `weight`、`site` 一樣只是設定。reload 改了它就套用，backend 保留健康狀態。peer group 只交換檢查結果，不受影響。

### open 的全部 down 不分主要與備援

沒有健康 backend、`open` 時，照現行的 `unknown` 優先規則，在全部 backend 中挑。使用者選擇了較簡單的規則。

### SRV 與 A/AAAA 用同一條規則

SRV 只列目前那一層的候選。各 `balance` 的 priority 編法不變，例如 `failover` 在備援層裡從 0 開始編。

### fastest 的 pool 依層分開計算

review 時發現：daemon 原本用全部健康 backend 的最低延遲計算 pool，備援也算在內。很快的備援（例如同機架的冷備）會成為唯一的最低值，主要 backend 全部落在 pool 外；候選只有主要 backend 時，`subset_or_all` 退回全部，結果變成單純輪流，延遲完全不起作用。反過來，備援接手時，正在失敗的主要 backend 也可能把備援擠出 pool。

改為在 `state::fast_pool` 中，主要與備援各自取最低延遲，各自判斷誰夠快；寫入 store 的 pool 是兩者的聯集。因為候選永遠只有一層，候選與 pool 的交集就是那一層的快 backend。store 格式、client 與 DNS 的程式都不用改，舊 client 讀到的 pool 也一樣正確。

替代方案：client 依候選重新計算 pool。這和 requirement「Readers SHALL use the pool as written」衝突，而且要改六種 client，所以不採用。

## Implementation Contract

**行為**

- 服務有主要 backend 通過時，查詢與 DNS 不會回傳備援 backend。
- 沒有通過的主要 backend、有通過的備援時，只回傳通過的備援。
- 都沒有通過的時，先用健康的主要 backend，再用健康的備援。
- 都不健康時照 `on_all_down`：`open` 在全部 backend 中依 `unknown` 優先挑選，`closed` 回 `ALL_DOWN` 或 SERVFAIL。
- store 過期時與現在相同。
- 新 daemon 寫的 store：舊版 client 回 `INCOMPATIBLE`，舊版 `emfor status` exit 2。
- 新版 client 與 `emfor status` 讀舊 daemon 的版本 1 store：照常運作。

**介面與資料**

- 設定：backend 表格的 `backup`，布林值，預設 `false`。
  - 不是布林值時，錯誤訊息為 `backup must be true or false`，路徑是該 backend 的 `backup`。
  - 全部都是備援時，錯誤訊息為 `at least one backend must not be a backup`，路徑是服務的 `backends`。
- Rust：
  - `config::Backend.backup: bool`；
  - `BackendRecord.backup: bool`，加上 `#[serde(default)]`，序列化時放在 `site` 之後；
  - `SCHEMA_VERSION = 2`；
  - 新增 `emfor_format` 的版本判斷函式。
- store：每個 backend 物件一律帶 `backup`；`meta/schema_version` 為 `2`。

**失敗模式**

- `meta/schema_version` 不是 `1` 也不是 `2`：client 回 `INCOMPATIBLE`，`emfor status` exit 2。
- 版本 1 的 store 若帶有 `backup: true`（不應發生）：照樣當作備援讀取，不報錯。

**驗收**

- `cargo test -p emfor-format`：
  - `healthy_or_all_table` 涵蓋 client-lookup 範例表的新列；
  - 新增 `backup` 欄位的序列化測試，以及版本判斷函式的測試（`1`、`2` 可讀；`0`、`3`、空字串不可讀）。
- `cargo test -p emfor-daemon`：
  - config 的 `backup` 驗證測試，涵蓋 service-config 範例表「backup values」與「Only backup backends」；
  - writer 寫出 `backup` 與版本 2；
  - select 的 A/AAAA 範例表新列與 SRV scenario「Backups listed only while no primary passes」；
  - status 接受版本 1 與 2、拒絕 3。
- `cargo test -p emfor-client --all-features`：conformance 的新案例在 Rust 與 C API 都通過，涵蓋：
  - 版本 2 的備援案例；
  - 版本 1 store 仍可讀；
  - 版本 3 回 `INCOMPATIBLE`。
- Debian 容器 `make -C clients test`：五個介面都是 0 失敗。
- 暫時拿掉備援層時，選擇相關的新測試失敗；暫時只接受版本 2 時，版本 1 的測試失敗。
- 依 RELEASING.md 的檢查在 macOS 與 Debian 通過；`config` fuzz target 跑 60 秒。

**範圍**

- 範圍內：
  - 上列 spec 與程式碼，以及 fixture 和 conformance 資料；
  - 手冊第 3、5、6、7 章（英文與中文），第 6 章寫明升級順序；
  - `examples/emfor.toml`、`CHANGELOG.md`。
- 範圍外：
  - `emfor status` 的文字標示、`/metrics`；
  - 只在用到時才升版本；
  - SRV priority。

## Risks / Trade-offs

- [升級 daemon 後舊 client 全部失敗] → CHANGELOG 標 **BREAKING**，手冊第 6 章與 CHANGELOG 寫明先升級 client。新 client 能讀舊 store，所以可以安全地先升級 client。
- [conformance 資料夾叫 `tests/conformance/v1`，案例卻有版本 2 的 store] → 資料夾名稱指的是 conformance 資料的格式，不是 store 版本。`README.md` 會說明 `store.schema_version` 可以是 `1` 或 `2`。
- [主要 backend 只是短暫失敗一次，就把流量全移到備援] → 下一次通過就移回來，和 emfor-avoid-failing 對正在失敗的處理一致。若不想這麼敏感，可以用 `failover` 或不設 `backup`。
