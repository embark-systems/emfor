## 1. 設定與 CIDR

- [x] 1.1 實作 requirement「Config validation rules」對 `[metrics] allow` 的規則（design：「CIDR 自己實作，不加相依」）。`crates/emfor-daemon/src/config.rs` 新增 `Cidr`（`parse(&str)`、`contains(IpAddr)`，IPv4-mapped IPv6 先換成 IPv4 再比）與 `MetricsConfig.allow: Option<Vec<Cidr>>`，驗證 `allow` 非空、每筆為「位址/前綴」、無重複、`[metrics]` 的其他鍵仍為 `unknown key`。驗證：`config.rs` 單元測試涵蓋 service-config spec 範例表中 `[metrics] allow` 的每一列，以及 CIDR 比對表（IPv4、IPv6、IPv4-mapped、前綴 0 與滿長、主機位元非 0），帶 `// spectra-scenario: metrics-endpoint / Source matching` 與 `... / Source matching / allow and peer address` 註解；`tools/docker-test.sh 'cargo test -p emfor-daemon --lib config::'` 通過，且新測試先因 `unknown key` 失敗。

## 2. 存取控制

- [x] 2.1 [after: 1.1] 實作 requirement「Metrics source allow-list」的連線過濾（design：「來源 allow-list，用 CIDR 清單」、「不在清單內的來源收 403，不是關閉連線」、「loopback 不自動放行」）。`crates/emfor-daemon/src/metrics.rs` 的 `serve` 接受 allow 清單，`connection` 之前以 `peer_addr` 判斷，不在清單內回 `403 Forbidden`、空 body、`Content-Length: 0`、`Connection: close`，不讀請求、不記 log；沒設 `allow` 行為不變。驗證：`metrics.rs` 或 `crates/emfor-daemon/tests/metrics.rs` 整合測試，`allow = ["127.0.0.0/8"]` 時 `GET /metrics` 得 200 與指標、`allow = ["10.0.0.0/8"]` 時得 403 且 body 無指標、未設 `allow` 得 200（情境「Source inside an allowed network」、「Source outside every allowed network」、「Not set」，帶對應 `spectra-scenario` 註解）；`tools/docker-test.sh` 內通過。
- [x] 2.2 [after: 1.1] 實作警告與重啟規則（design：「非 loopback 且沒設 allow 時警告」、「allow 變更需要重啟」）。`crates/emfor-daemon/src/cli.rs`：非 loopback 的 `[metrics] listen` 位址且未設 `allow` 時，啟動與 reload 通過後各記一則警告（每個位址一則，措辭同 DNS 警告型態）；reload 驗證在 `allow` 與目前不同時加入錯誤 `metrics.allow`／`metrics allow settings require restart`。驗證：`cli.rs` 單元測試——非 loopback 無 allow 有警告、設了 `allow` 或只有 loopback 沒有；`allow` 變更的 reload 回上述錯誤、`listen` 變更仍回原有的 `metrics listen settings require restart`；帶 `// spectra-scenario: metrics-endpoint / Warning for a non-loopback address without allow` 與 `... / Allow change requires restart` 註解；`tools/docker-test.sh` 內通過。

## 3. 文件

- [x] 3.1 [after: 2.1, 2.2] `docs/manual/05-configuring.md`、`12-operating.md`、`13-security.md` 與 `docs/manual.zh_TW/` 對應檔：`[metrics]` 說明加 `allow`（CIDR、重啟才生效、loopback 無例外、經 NAT 或 proxy 時看到的是 proxy 位址、不支援 `X-Forwarded-For`）；第 13 章信任表中「The network to DNS and `/metrics`: No authentication」的一列改為說明 `allow` 與限制；`CHANGELOG.md` 的 Unreleased 加一條。驗證：內容審閱，中英與 spec 用語一致；再跑 `tools/docker-test.sh` 的完整檢查（fmt、clippy、`cargo test --workspace --all-features`）通過。
