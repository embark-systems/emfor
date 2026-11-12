## Why

要知道後端健康狀況，目前只能在主機上執行 `emfor status`。已經有 Prometheus 之類監控系統的環境，沒辦法把每台主機的後端狀態、檢查延遲、失敗次數收進來做告警與趨勢圖；`emfor status` 只看得到當下，store 的歷史每台後端也只留 20 筆，算不出長期的失敗率或轉換頻率。

## What Changes

- 設定檔新增可選的 `[metrics]` 表：`listen`（預設 `["127.0.0.1:5301"]`）。有這個表時 daemon 才開 HTTP 連接埠；沒有就完全不開。
- daemon 以自寫的最小 HTTP/1.1 實作回應 `GET /metrics`，輸出 Prometheus 文字格式（`text/plain; version=0.0.4`）；其他路徑回 404、其他方法回 405、格式錯誤或標頭過大回 400，每個回應後關閉連線。不新增 crate，也不開啟 `hyper` 的伺服器功能。
- 指標（標籤 `service`、`backend`）：
  - 現況：`emfor_backend_state`（`state` 為 `unknown`/`healthy`/`unhealthy`，目前那個為 1、其餘為 0）、`emfor_backend_check_latency_seconds`、`emfor_backend_last_check_timestamp_seconds`、`emfor_backend_in_pool`（只有 `fastest` 服務）、`emfor_service_all_down`。
  - 累計（自 daemon 啟動）：`emfor_backend_checks_total{result="up|down|error"}`、`emfor_backend_transitions_total{to="..."}`、`emfor_store_commit_failures_total`。
  - 其他：`emfor_build_info{version}`、`emfor_start_time_seconds`。
- 移除的服務或後端，其指標從下一次輸出消失。
- 啟動時無法綁定 `[metrics] listen` 的位址，daemon 以結束碼 1 結束並指出該位址；reload 時改動 `[metrics] listen`（含新增或移除整個表）會被拒絕，訊息為 `metrics listen settings require restart`。

## Non-Goals

- 不提供驗證、TLS 或存取控制；範例設定只綁 127.0.0.1，對外開放由防火牆控管。
- 不提供 DNS 查詢次數等 DNS 伺服器的指標，也不提供 OpenMetrics 格式、push gateway 或 StatsD。
- 不改 store、client、`emfor status` 的行為。

## Capabilities

### New Capabilities

- `metrics-endpoint`: daemon 的 `/metrics` HTTP 端點：啟用方式、HTTP 行為、指標名稱與內容、計數器的生命週期。

### Modified Capabilities

- `service-config`: 驗證規則加入 `[metrics] listen`；SIGHUP 重新載入時，改動 `[metrics] listen` 需要重啟。

## Impact

- 程式：新增 crates/emfor-daemon/src/metrics.rs（計數器、輸出格式、HTTP 端點）；crates/emfor-daemon/src/config.rs（`[metrics]` 表與只讀 listen 的解析）；crates/emfor-daemon/src/cli.rs（啟動時綁定、reload 檢查）；crates/emfor-daemon/src/schedule.rs（`BackendUpdate` 帶上檢查結果種類）；crates/emfor-daemon/src/writer.rs（累加計數器、commit 失敗次數）；crates/emfor-daemon/src/lib.rs。
- 測試：metrics 單元測試、新增 crates/emfor-daemon/tests/metrics.rs 整合測試。
- 文件：docs/deploy/daemon.md（metrics 一節）、examples/emfor.toml（註解掉的 `[metrics]`）、docs/spectra/config.yaml。
- 不影響：client、store 格式、DNS 行為、檢查器；不新增 crate。
- 序列數：每台後端約 12 個時間序列（估計：3 個 state、延遲、最後檢查時間、最多 1 個 pool、3 種檢查結果、最多 3 種轉換），以實作後對一個 10 台後端的設定實際計算 `/metrics` 輸出的行數確認。
