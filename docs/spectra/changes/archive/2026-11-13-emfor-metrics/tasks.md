## 1. 設定

- [x] 1.1 設定與 reload 比照 DNS：`crates/emfor-daemon/src/config.rs` 新增 `MetricsConfig` 與 `Config.metrics`，`[metrics]` 只接受 `listen`（預設 `["127.0.0.1:5301"]`），listen 驗證與 `[dns] listen` 共用、錯誤路徑為 `metrics.listen[...]`，並新增只解析 listen 的 `parse_metrics_listen`。完成時 spec service-config「Config validation rules」新增的四列（空陣列、主機名稱、重複位址、未知鍵 `path`）各回報對應錯誤；以 config 單元測試驗證，`examples/emfor.toml` 的既有測試（`example_config_is_valid`）仍通過。

## 2. 計數

- [x] 2.1 BackendUpdate 帶上檢查結果種類：`crates/emfor-daemon/src/schedule.rs` 新增 `CheckOutcome { Up, Down, Error }` 與 `BackendUpdate.outcome`，檢查迴圈依 `CheckResult` 填入。以 schedule 單元測試驗證 Up、Down、Error 三種結果各對應正確的 outcome。
- [x] 2.2 [after: 2.1] 計數器放在 LiveState：`LiveView` 新增每台後端的 checks、transitions 計數、`commit_failures` 與 `started_at_ms`；新增 `LiveState::update_backend`（同一次寫入鎖內更新記錄並計數）與 `LiveState::count_commit_failure`；writer 的 `update` 改用 `update_backend`，`set_layout` 建立新後端的全 0 計數、刪除移除後端的計數（`reset` 不清），`flush` commit 失敗時加 1。以 writer 單元測試驗證 spec metrics-endpoint「Check and transition counters」：「Counting results and transitions」（up 2、down 3、healthy 1、unhealthy 1、unknown 0）、「Counters reset when a backend is removed and added back」、舊 generation 的結果不計、`reset` 保留計數；以及「Daemon metrics」的「Store commit failure counted」。

## 3. 輸出與 HTTP

- [x] 3.1 [after: 2.2] 輸出由純函式產生：新增 `crates/emfor-daemon/src/metrics.rs` 的 `render(&LiveView, version)`，依 spec「Backend and service metrics」與「Daemon metrics」輸出 HELP、TYPE 與各序列。以單元測試驗證「One healthy and one failing backend」的五行內容與缺少延遲序列、`fastest` 服務的 `emfor_backend_in_pool`、非 fastest 服務沒有 in_pool、label 中 `\`、`"`、換行的跳脫、`emfor_build_info` 與 `emfor_start_time_seconds`。
- [x] 3.2 [after: 3.1] 自寫最小的 HTTP 伺服器，實作 spec「Metrics HTTP behavior」：`metrics::serve` 綁定所有位址，每連線讀取 head（上限 8192 位元組、5 秒逾時）、回應後關閉，最多 16 條同時連線。以單元測試驗證「request outcomes」表格六列的狀態碼、`Allow: GET`、`Content-Type` 與 `Content-Length`。

## 4. 啟動、整合測試與文件

- [x] 4.1 [after: 1.1, 3.2] 實作 spec「Metrics endpoint enabled by the metrics table」與 service-config「Reload on SIGHUP applies only a fully valid config」的 metrics 部分：`crates/emfor-daemon/src/cli.rs` 在 DNS 之後依 `[metrics]` 綁定（預設埠 5301 見 design「預設連接埠 5301」）、失敗以結束碼 1 結束且訊息含位址、記 `metrics on <addresses>`；reload 時 listen 改動加入錯誤 `metrics listen settings require restart`。新增整合測試 `crates/emfor-daemon/tests/metrics.rs`：沒有表不開埠、預設位址回 200 且含 `emfor_build_info`、位址被占用結束碼 1、listen 改動 reload 被拒且舊埠照常回應、閒置連線 5 秒後關閉且其他請求照常（「Slow client」）、後端拒絕連線後 `state="unhealthy"` 為 1、移除服務後序列消失（「Removed service disappears」）。測試不得使用固定的 5301，改用各自占住的空埠。
- [x] 4.2 [after: 4.1] 實測一個 10 台後端的設定，計算 `/metrics` 輸出中非註解的行數，確認 design 中「每台後端 9–12 個序列」的估計，結果寫進 `docs/deploy/daemon.md`。
- [x] 4.3 [after: 4.2] 文件：`docs/deploy/daemon.md` 新增 metrics 一節（啟用、指標表、Prometheus 抓取設定範例、只綁 127.0.0.1 與防火牆說明）；`examples/emfor.toml` 加入註解掉的 `[metrics]`；`docs/spectra/config.yaml` 記錄 metrics 端點。以人工檢閱確認與 spec 一致，`example_config_is_valid` 仍通過。
- [x] 4.4 [after: 4.3] 本機 gate 全數通過：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`，以及 Debian 映像 `emfor-clients-test` 中的同一組指令。

## 5. 驗證後修正

- [x] 5.1 [after: 4.4] 實作 spec「Metrics HTTP behavior」的「Not ready yet」：`crates/emfor-daemon/src/metrics.rs` 的連線處理在 `LiveView.ready` 為 false 時對 `GET /metrics` 回 503（其他路徑與方法仍回 404、405、400），`ready` 後回 200。以 metrics 單元測試驗證未就緒的 `LiveState` 回 503 且內容沒有 `emfor_`、`replace` 後同一請求回 200；`docs/deploy/daemon.md` 的 Metrics 一節加上 503 的說明；macOS 與 Debian gate 全數通過。
