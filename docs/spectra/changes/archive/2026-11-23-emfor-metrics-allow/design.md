## Context

`/metrics` 由 `crates/emfor-daemon/src/metrics.rs` 的 `serve` 提供：對 `[metrics] listen` 的每個位址綁一個 `TcpListener`，接受連線後交給 `connection`，只回 `GET /metrics`。`MetricsConfig`（`config.rs`）只有 `listen`，驗證規則在 service-config spec 的「Config validation rules」。daemon 對 DNS 綁非 loopback 位址會在啟動與 reload 記警告（`cli.rs` 的 `listen_warnings`），對 `/metrics` 沒有。重啟檢查在 `cli.rs` 的 reload 驗證：`metrics.listen` 改變時回 `metrics listen settings require restart`。專案沒有 CIDR 相關相依套件。

## Goals / Non-Goals

**Goals:**

- 讓維運能限制哪些來源可讀 `/metrics`，不加相依套件、不管憑證。
- 非 loopback 又沒限制時警告，與 DNS 一致。

**Non-Goals:**

- bearer token、TLS、mTLS（需要密鑰或憑證檔、輪替與熱載入，成本高於要解的問題）。
- DNS 的存取控制。
- 路徑與方法的改動；新的 metric。

## Decisions

### 來源 allow-list，用 CIDR 清單

`[metrics] allow = ["10.0.0.0/8", "fd00::/8", "192.0.2.7/32"]`。比對的是連線的對端位址（`peer_addr`），IPv4-mapped IPv6（`::ffff:a.b.c.d`）先換成 IPv4 再比。沒設 `allow` 時完全不檢查（現狀）。替代方案：bearer token（Prometheus 原生支援，但要管一個密鑰檔及其模式規則）、TLS（最完整但最大）；放棄理由見 Non-Goals。

### CIDR 自己實作，不加相依

比對只需要「位址前 N 位元相同」，約 40 行，不值得為它在封閉環境多帶一個 crate 進供應鏈。解析接受 `位址/前綴長度`（IPv4 前綴 0–32、IPv6 前綴 0–128）；沒有 `/前綴` 的位址不接受（避免與「單一主機」的寫法分歧，要單一主機寫 `/32` 或 `/128`）。主機位元不為 0（例如 `10.0.0.5/8`）接受，比對時只看前綴。

### 不在清單內的來源收 403，不是關閉連線

回 `403 Forbidden`、空 body、`Connection: close`，不讀請求。這樣抓取設定寫錯時，Prometheus 的 target 頁會顯示 403 而不是難以判斷的連線重置。不記 log（避免被掃描時灌爆 log）。

### loopback 不自動放行

設了 `allow` 就只照清單：`allow = ["10.0.0.0/8"]` 時，本機 `curl 127.0.0.1:5301/metrics` 也是 403。規則單純、可預測；要從本機讀就把 `127.0.0.0/8` 列進去。手冊要寫明。

### allow 變更需要重啟

與 `listen` 相同：reload 時 `allow` 與目前不同則拒絕整個 reload，訊息 `metrics allow settings require restart`（錯誤的欄位為 `metrics.allow`）。不做熱載入，因為 accept 迴圈要共享可變狀態，換來的好處很小。

### 非 loopback 且沒設 allow 時警告

啟動與 reload（通過驗證後）都記警告：`metrics listens on <addr>, which is not a loopback address: every host that can reach it can read every service name, backend address and check failure reason; limit who can reach it with allow or a firewall`，每個這樣的位址一則，與 DNS 的警告同型。設了 `allow` 就不警告。

## Implementation Contract

**行為**

- 沒設 `allow`：與現在完全相同（若有非 loopback 位址則多一則警告）。
- 設了 `allow`：來源在任一 CIDR 內得到正常回應（含 503、404、405 等既有狀態）；不在任何 CIDR 內得到 `403 Forbidden`，無指標內容。
- `allow` 驗證失敗時整份設定被拒絕，錯誤指出 `metrics.allow` 與原因。

**介面**

- `MetricsConfig` 加 `allow: Option<Vec<Cidr>>`（`None` = 沒設）；`Cidr` 為 `config.rs` 內的小型別，提供 `parse(&str)` 與 `contains(IpAddr)`。
- `metrics::serve` 接受 allow 清單（或包含它的設定）；`connection` 之前先以 `peer_addr` 判斷。
- 設定值檢查：`allow` 非空字串陣列（空陣列：`allow must not be empty`）；每筆需為 CIDR（`allow entry must be an address with a prefix length, such as "10.0.0.0/8"`）；重複（含完全相同的字串）：`duplicate allow entry`。

**失敗模式**

- 無法取得對端位址（極少見）：視為不在清單內，回 403。
- 驗證失敗：`check-config` 失敗，daemon 啟動失敗或 reload 被拒絕。

**驗收標準**

- `config.rs` 單元測試涵蓋 CIDR 解析與比對（IPv4、IPv6、IPv4-mapped、前綴 0 與滿長、主機位元非 0）與驗證錯誤表。
- `metrics.rs` 整合測試：綁 `127.0.0.1` 並設 `allow = ["127.0.0.0/8"]` 得 200、設 `allow = ["10.0.0.0/8"]` 得 403 且 body 沒有指標。
- `cli.rs` 單元測試：非 loopback 且沒設 `allow` 有警告，設了或全是 loopback 沒有；`allow` 變更的 reload 被拒絕且訊息正確。
- 全部測試經 `tools/docker-test.sh` 在 Debian Docker 內通過。

**範圍**

- 範圍內：`allow` 的設定、驗證、比對、403、警告、重啟規則；metrics-endpoint 與 service-config spec；手冊第 5、12、13 章（中英）與 CHANGELOG。
- 範圍外：Non-Goals 列的各項。

## Risks / Trade-offs

- **來源位址可被中間設備改寫**：經過 NAT、反向 proxy 時，daemon 看到的是 proxy 的位址。`allow` 保護的只是直接連線的來源；手冊要說明，並且不支援 `X-Forwarded-For`。
- **本機也被擋**：設了 `allow` 就連 loopback 也要列入，見上。手冊寫明。
- **allow 變更要重啟**：重啟 daemon 會讓狀態回到 `unknown`（既有行為），所以調整 `allow` 的代價比 reload 高；與 `listen` 一致，且 `allow` 很少改。
