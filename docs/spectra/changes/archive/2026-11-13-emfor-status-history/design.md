## Context

store 目前每個服務只有一個 `svc/<name>` 記錄，內容是後端「現在」的狀態；writer（`crates/emfor-daemon/src/writer.rs` 的 `Writer`）在 `update` 收到帶 `transition` 的 `BackendUpdate` 時立即 `flush`，把所有 dirty 的服務記錄寫在同一個交易。啟動時 `set_layout(fresh_start = true, ...)` 會 `Op::DeletePrefix("svc/")` 再寫新記錄；reload 時對移除的服務 `Op::Delete(service_key)`；`check` 或 `check_options` 改變的服務帶 `reset = true`，後端回到 `unknown`，但不經過 `update`，所以沒有 transition。

`emfor status`（`crates/emfor-daemon/src/status.rs`）以 `Reader::read` 在一個唯讀交易讀 `meta/*` 與 `svc/*` 組成 `Snapshot`；`Watch` 自己比對前後畫面，把看到的變化以字串存在 `changes`（最多 10 筆）。所以 `--watch` 開始前的轉換看不到，daemon 重啟後也全部消失。

限制：store 格式版本維持 1；client（Rust、libemfor、各語言包裝）與 DNS 只讀 `svc/*` 與 `meta/*`，讀取端本來就忽略不認得的 key，所以新增 key 不影響它們。

## Goals / Non-Goals

**Goals:**

- 每台後端保留最近 20 筆由檢查造成的健康狀態轉換，跨 daemon 重啟保留。
- `emfor status` 依歷史標出抖動的後端，提供 `--history` 文字輸出，JSON 加上 `history` 與 `flapping`。
- `--watch` 開始時就列得出之前的轉換。

**Non-Goals:**

- 抖動不影響結束碼、daemon 挑選後端、`all_down` 或 DNS 回應（使用者選定「只顯示」）。
- 不提供可設定的筆數上限、時間窗或門檻；20 筆、10 分鐘、5 次為固定值。
- 不記錄 reset（啟動或 `check`/`check_options` 改變）造成的 `unknown`；不記錄 daemon 狀態變化到 store。
- 不改 store 格式版本、不新增 crate、不改 client 端讀取程式。

## Decisions

### 歷史存在獨立的 hist key

每個服務一個 `hist/<name>`，值為 `{"backends":{"<id>":[entry...]}}`，不放進 `svc/<name>` 記錄。

- 理由：client 與 DNS 每次查詢都解析 `svc/` 記錄；放進去會讓最多 20 × 後端數筆資料隨每次查詢被解析。獨立 key 只有 `emfor status` 讀。
- 替代方案：放進 `BackendRecord` 的新欄位 —— 讀取端雖會忽略，但仍須解析整段 JSON，捨棄。每筆轉換一個 key（`hist/<name>/<id>/<at>`）—— 修剪要逐 key 刪除、交易內 op 數變多，捨棄。

### 歷史與服務記錄同一交易寫入

writer 在記憶體保有 `histories: BTreeMap<Arc<str>, ServiceHistory>` 與 `hist_dirty: BTreeSet<Arc<str>>`。`update` 遇到 `transition` 時先把 entry 加到該後端（超過 20 筆丟最舊），標記 hist dirty，然後照舊 `flush`。`flush` 在輸出 dirty 服務記錄的 `Op::Put` 之後，對每個 hist dirty 服務輸出 `Op::Put(hist_key, json)`，歷史為空時輸出 `Op::Delete(hist_key)`。commit 失敗時，`pending` 的過濾條件比照 `svc/` 一併丟掉 `hist/` 的 put 與 delete，hist dirty 集合保留，下次 heartbeat 從記憶體重新產生。

- 理由：沿用既有的「transition 立即寫」路徑，記錄與歷史不會不一致；重試時以最新記憶體為準，不會寫回舊值。

### 啟動時由 cli 讀出既有歷史交給 writer

`cli.rs` 在 `Store::open` 之後、`writer::spawn_live` 之前，用新增的 `Store::prefix(HIST_KEY_PREFIX)`（回傳 `BTreeMap<String, String>`）讀出所有 `hist/*`，逐一解析成 `ServiceHistory`；解析失敗的以 `tracing::warn!` 記下 key 與錯誤後丟棄。結果以新參數傳給 `spawn_live`（`spawn` 與 `spawn_with_tick` 給空的 map），writer 存成初始 `histories`。`set_layout(fresh_start = true)` 時：`Op::DeletePrefix("hist/")`，把 `histories` 修剪到設定中存在的服務與後端，再把非空的全部標記為 hist dirty，於同一次 `flush` 寫回。reload 時：移除的服務 `histories.remove` 並 `Op::Delete(hist_key)`；留下的服務若有後端被移除，刪掉該後端的 entry 並標記 hist dirty。

- 理由：writer 維持只寫不讀（`Sink` trait 不變，測試用的假 sink 不需要讀能力）；修剪集中在 `set_layout` 一處。
- 替代方案：writer 透過 `Sink` 讀 store —— 要擴充 trait 與所有假實作，捨棄。

### 歷史型別放在 emfor-format

`crates/emfor-format/src/lib.rs` 新增 `HIST_KEY_PREFIX = "hist/"`、`hist_key(name)`、`HISTORY_LIMIT = 20`、`HistoryEntry { at: u64, from: HealthState, to: HealthState, reason: Option<String> }`、`ServiceHistory { backends: BTreeMap<String, Vec<HistoryEntry>> }`，以及 `ServiceHistory::push(id, entry)`（套用上限）與 `ServiceHistory::retain(keep)`（以閉包決定保留哪些後端，空的後端一併移除，回傳是否有變）。daemon 與 status 共用同一份格式定義。

### 抖動由 emfor status 讀取時計算

`status.rs` 新增 `flap_count(entries, now_ms) -> usize`：計算 `from`/`to` 為 healthy 與 unhealthy 互換、且 `now_ms - at <= 600_000` 的 entry 數；`FLAP_THRESHOLD = 5`、`FLAP_WINDOW_MS = 600_000`。daemon 不計算、不存結果。

- 理由：使用者選定「只顯示」，判斷只在顯示端；門檻日後要改不需動 store。

### 狀態讀取一併讀歷史

`Snapshot` 新增 `history: BTreeMap<String, ServiceHistory>`，`Reader::read` 在同一個唯讀交易內 `prefix_iter("hist/")`，只保留有 `svc/` 記錄的服務。無法解析的 `hist/` 值當作不存在（不讓 `emfor status` 因歷史損壞失敗），`svc/` 的錯誤處理不變。`filter` 依 `--service` 同時篩選 `services` 與 `history`。

### watch 的變化清單由歷史與 daemon 變化合併

`Watch` 不再自己從畫面差異產生後端變化字串，改成只記錄 daemon 狀態變化 `(at_ms, from, to)`（最多 10 筆）。每一幀由 `Watch::changes(now_ms)` 把上一個成功讀到的 snapshot 歷史中所有後端 entry 與 daemon 變化依時間合併、新的在上、取 10 筆後格式化；當地日期不是今天的加上 `YYYY-MM-DD ` 前綴。畫面高亮仍由 `Watch::update` 比對前一幀決定，規則不變。

## Implementation Contract

**行為**

- daemon 每次檢查造成健康狀態轉換時，`hist/<service>` 在同一交易多一筆 `{"at","from","to","reason"}`，每台後端最多 20 筆（舊的先丟），entry 依時間由舊到新。
- daemon 重啟後歷史仍在；不在設定中的服務 key 與後端 entry 在啟動或 reload 後消失；無法解析的值在啟動時記一行 warning 後丟棄。
- `emfor status`：抖動後端的列多 `FLAPPING (<n> changes in 10m)`，位置在 `pool` 之後、`reason` 之前；結束碼不變。
- `emfor status --history [--service <name>]`：`store:`、daemon 行，然後依服務名稱排序、每台後端依 store 順序，entry 新的在上，格式 `<YYYY-MM-DD HH:MM:SS> <from> -> <to>`，有 reason 時加兩個空格與 reason；沒有 entry 的後端印 `no changes recorded`。
- `emfor status --json`：多 `history`（服務名稱 → 存的值）與 `flapping`（`[{"service","backend","changes"}]`），兩者都受 `--service` 篩選；沒有歷史時為 `{}` 與 `[]`。
- `emfor status --watch`：變化清單一開始就含之前的轉換；非今天的加日期前綴。

**介面 / 資料形狀**

- key：`hist/<service name>`；值：`{"backends":{"<id>":[{"at":<u64>,"from":"healthy","to":"unhealthy","reason":"..."|null}]}}`。
- `StatusArgs.history: bool`，以 clap `conflicts_with_all = ["json", "watch"]` 拒絕組合，錯誤時結束碼 2（clap 的使用錯誤）。
- `writer::spawn_live(store, rx, live, history: BTreeMap<String, ServiceHistory>)`。
- `Store::prefix(&str) -> heed::Result<BTreeMap<String, String>>`。

**失敗模式**

- commit 失敗：與現在相同，記 error、下次 heartbeat 重試；重試寫的是當時記憶體中的歷史。
- 啟動時 `hist/` 解析失敗：warning，丟棄該服務歷史，daemon 照常啟動。
- `emfor status` 讀到無法解析的 `hist/`：當作沒有歷史，不報錯。

**驗收**

- writer 單元測試：轉換寫入 `hist/` 且與 `svc/` 同一次 commit；第 21 筆丟最舊；reset 不產生 entry；啟動修剪服務與後端；reload 刪除服務與後端的歷史；commit 失敗後重試寫入最新歷史。
- status 單元測試：`flap_count` 的計數表（spec「Flapping backends」的 Example）；`--history` 文字格式；JSON 的 `history` 與 `flapping`；watch 合併清單（spec「merged change list」Example）。
- `crates/emfor-daemon/tests/daemon.rs` 整合測試：daemon 重啟後 `hist/` 仍在且 `emfor status --history` 列出之前的轉換；`--history --watch` 結束碼 2；`--json` 含 `history`。
- 本機 gate：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。

**範圍**

- 包含：`crates/emfor-format`、`crates/emfor-daemon` 的 writer、store、cli、status，相關測試，`docs/deploy/daemon.md` 說明 `hist/` 與 `--history`。
- 不包含：client、libemfor、各語言包裝、DNS、檢查器、store 格式版本、可設定的門檻。

## Risks / Trade-offs

- [store 變大] → 每筆 entry 約 100 位元組（估計），一個 10 台後端的服務最多約 20 KB；以實作後量測 `hist/` 值長度確認。預設 64 MiB `map_size` 足以容納上千個這種服務；量測結果若遠超估計，再於文件註明。
- [每次轉換多序列化一份歷史] → 只在轉換時寫，轉換頻率遠低於檢查頻率；抖動的後端最多也只是 20 筆 JSON。
- [時鐘倒退使 `at` 不單調] → entry 依接收順序附加，不重排；抖動計數只看 `now - at`，倒退時最多少算或多算一個時間窗，僅影響顯示。
- [reset 不記錄，歷史看不到「設定改了」] → 屬於 Non-Goal；daemon log 仍有 reload 紀錄。

## Migration Plan

不需遷移：舊 store 沒有 `hist/` key，等同空歷史；舊版 daemon 或 client 讀到新 store 會忽略 `hist/`。回復到舊版時，舊 daemon 啟動不會刪 `hist/*`（只刪 `svc/*`），殘留 key 無害，再升級時會被修剪。
