## Context

daemon 的現況都在記憶體裡：`LiveState`（`crates/emfor-daemon/src/live.rs`）包著 `RwLock<LiveView>`，`LiveView.services` 以服務名稱存每個服務的 `ServiceRecord`，writer 每收到一次檢查結果就以 `LiveState::set_record` 更新，DNS 回答也從這裡讀。檢查任務（`schedule.rs` 的檢查迴圈）每次檢查後送出 `BackendUpdate`，內容有狀態、`reason`、`transition`、`latency_us`，但沒有這次檢查是 up、down 還是 error。writer 在 `update` 開頭丟掉舊 generation 的結果，`flush` 在 commit 失敗時記 error。

daemon 目前不開 HTTP 連接埠；依賴裡的 `hyper` 只是 `reqwest` 帶進來的用戶端功能。DNS 伺服器以可選的 `[dns]` 表啟用，`listen` 改動在 reload 時以 `parse_dns_listen` 比對、拒絕。

## Goals / Non-Goals

**Goals:**

- 可選的 `[metrics]` 表啟用 `/metrics`，輸出 Prometheus 文字格式的後端現況與自啟動以來的累計次數。
- 不新增 crate；HTTP 部分只做到 Prometheus 抓取所需。

**Non-Goals:**

- 驗證、TLS、存取控制；DNS 查詢指標；OpenMetrics、push gateway、StatsD。
- 改變 store、client、`emfor status`、檢查或選後端的任何行為。

## Decisions

### 自寫最小的 HTTP 伺服器

`crates/emfor-daemon/src/metrics.rs` 以 tokio `TcpListener` 接受連線，每條連線一個 task：在 5 秒內讀到 `\r\n\r\n` 為止（上限 8192 位元組），解析請求行，寫出回應後關閉。同時服務的連線以 `tokio::sync::Semaphore`（16 個）限制，拿不到就直接關閉連線。

- 理由：Prometheus 抓取只需要 `GET /metrics` 與單一回應；自寫約一兩百行，不增加依賴，也不必為 `hyper` 開伺服器功能、再引入 `hyper-util`。
- 替代方案：開啟 `hyper` 的 `server`、`http1` 功能 —— 要多加 `hyper-util` 與 service 介面，捨棄。`axum` —— 依賴樹大幅增加，捨棄。

### 預設連接埠 5301

`[metrics] listen` 預設 `["127.0.0.1:5301"]`，與 DNS 的預設 5300 相鄰。

- 理由：Prometheus wiki「Default port allocations」（2026-11-13 查閱）上 9100–9999 已排滿，9312 為避開 Sphinx 搜尋 API 而刻意不分配；5300、5301 都不在清單上。預設只綁 loopback，衝突時可自行改。
- 替代方案：9312 —— 與 Sphinx searchd 預設埠相同，捨棄；不給預設、強制寫 `listen` —— 多一個必填項，捨棄。

### 計數器放在 LiveState

`LiveView` 新增 `counters: HashMap<(String, String), BackendCounters>`（以服務名稱與後端 id 為 key；`BackendCounters` 有 `checks: [u64; 3]` 與 `transitions: [u64; 3]`）與 `commit_failures: u64`，另以 `started_at_ms` 記啟動時間。writer 在 `update` 通過 generation 檢查後，改呼叫新的 `LiveState::update_backend(record, backend, outcome, transition)`：在同一次寫入鎖內更新記錄並累加計數；`set_layout` 依新設定建立新後端的全 0 計數器、刪除不再設定的後端計數器（`reset` 不清計數器）；`flush` commit 失敗時以 `LiveState::count_commit_failure` 加 1。

- 理由：writer 已經在每次檢查時持有 `LiveState` 的寫入鎖更新記錄，計數在同一處、同一順序，與 generation 過濾一致；`/metrics` 只需讀一個 handle。
- 替代方案：檢查任務自己累加 —— 舊 generation 的結果會被算進去，也要另傳 handle，捨棄。

### BackendUpdate 帶上檢查結果種類

`schedule.rs` 新增 `pub enum CheckOutcome { Up, Down, Error }`（由 `CheckResult` 轉換），`BackendUpdate` 新增 `outcome: CheckOutcome` 欄位。

### 輸出由純函式產生

`metrics::render(view: &LiveView, version: &str) -> String` 依 spec「Backend and service metrics」「Check and transition counters」「Daemon metrics」產生內容：服務依名稱排序、後端依記錄中的順序；延遲為 `latency_us as f64 / 1e6` 以 Rust `{}` 格式輸出（1200 µs → `0.0012`）；時間戳為 `{秒}.{毫秒三位}`（1762905600123 → `1762905600.123`）；label 值跳脫 `\`、`"`、換行。HTTP 層只負責呼叫它。

### 設定與 reload 比照 DNS

`config.rs` 新增 `MetricsConfig { listen: Vec<SocketAddr> }` 與 `Config.metrics: Option<MetricsConfig>`；把 `dns_listen` 改為接受路徑前綴的共用驗證（錯誤訊息相同，路徑為 `metrics.listen[...]`），新增 `parse_metrics_listen`，與 `parse_dns_listen` 同樣只解析 listen。`cli.rs` 在 DNS 之後綁定 metrics（失敗以結束碼 1 結束，訊息含位址），記 `metrics on <addresses>`；reload 比對不同時加入錯誤 `metrics listen settings require restart`（路徑 `metrics.listen`）。

## Implementation Contract

**行為**

- 沒有 `[metrics]` 表：不開 socket，沒有 `metrics on` log。有表：在每個 `listen` 位址開 TCP，`emfor started` 之前記 `metrics on 127.0.0.1:5301`。
- `GET /metrics`（可帶 query）→ 200、`Content-Type: text/plain; version=0.0.4; charset=utf-8`；其他路徑 404；其他方法 405 加 `Allow: GET`；格式錯誤或 head 超過 8192 位元組 400；5 秒沒送完 head 直接關閉；每個回應都有 `Content-Length`、`Connection: close` 後關閉。
- 指標內容見 spec「Backend and service metrics」「Check and transition counters」「Daemon metrics」，數值反映請求當下的 `LiveState`。

**介面 / 資料形狀**

- 設定：`[metrics]`，唯一鍵 `listen`（陣列，預設 `["127.0.0.1:5301"]`）。
- `metrics::serve(listen: &[SocketAddr], live: LiveState) -> Result<MetricsServer, String>`；`MetricsServer` 被 drop 或 daemon 結束時停止接受連線。
- `metrics::render(&LiveView, version) -> String`。
- `BackendUpdate.outcome: CheckOutcome`；`LiveState::update_backend`、`LiveState::count_commit_failure`。

**失敗模式**

- 綁定失敗：啟動時結束碼 1，錯誤含位址；不會在 reload 時發生（listen 改動會被拒絕）。
- 單一連線出錯（讀寫失敗、逾時）：只關閉該連線，不記 error（最多 debug），不影響 daemon。
- 超過 16 條同時連線：新連線直接關閉。

**驗收**

- `metrics.rs` 單元測試：`render` 對 spec「One healthy and one failing backend」的輸出、label 跳脫、`fastest` 的 `in_pool`、無延遲時不出現序列；請求解析對 spec「request outcomes」表格六列的狀態碼。
- writer 單元測試：spec「Counting results and transitions」（up 2、down 3、healthy 1、unhealthy 1）、舊 generation 不計、reload 移除再加回從 0 開始、`reset` 不清計數、commit 失敗加 1。
- config 單元測試：spec service-config「validation outcomes」新增的四列。
- 新整合測試 `crates/emfor-daemon/tests/metrics.rs`：沒有表不開埠；預設位址回 200 且內容含 `emfor_build_info`；位址被占用結束碼 1；listen 改動 reload 被拒、舊埠照常回應；慢速連線 5 秒後被關閉且其他請求照常；後端轉 unhealthy 後 `emfor_backend_state{...,state="unhealthy"} 1`。
- 本機 gate：macOS 與 `emfor-clients-test` Debian 映像的 fmt、clippy、test。

**範圍**

- 包含：`crates/emfor-daemon` 的 metrics、config、cli、schedule、writer、live；測試；`docs/deploy/daemon.md`、`examples/emfor.toml`、`docs/spectra/config.yaml`。
- 不包含：其他 crate、store 格式、client、DNS 指標、驗證與 TLS。

## Risks / Trade-offs

- [端點沒有驗證，任何能連到的人都看得到後端位址與狀態] → 預設與範例只綁 127.0.0.1；文件說明要對外時以防火牆限制來源。
- [自寫 HTTP 解析的邊界情況] → 只接受完整 head、固定上限與逾時，回應後一律關閉，不支援 keep-alive 或 chunked；以單元與整合測試涵蓋 spec 表格。
- [時間序列數] → 每台後端 9–12 個序列（估計：state 3、checks 3、transitions 3，加上延遲、最後檢查時間、pool 各 0–1 個），1000 台後端約 1.2 萬個；以 10 台後端的設定手動實測 `/metrics` 輸出行數確認（task 4.2：104 條，每台 10 條加 4 條，結果記在 `docs/deploy/daemon.md`）。
- [metrics 伺服器比 writer 先啟動，第一次套用服務配置前抓取會拿到啟動時間 0 與空的服務列表] → `LiveView.ready` 為 false 時回 503，比照 DNS handler 在未就緒時不回答；Prometheus 只記一次抓取失敗，不會記到錯的數值。
- [每次檢查多一次計數] → 以 `LiveState::update_backend` 把 `set_record` 與計數合併在同一次寫入鎖內，DNS 讀取不會多等一次鎖。
