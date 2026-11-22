## Why

`/metrics` 沒有任何存取控制：`[metrics] listen` 可以綁任何位址，綁了非 loopback 位址之後，能連到的主機都能讀到所有服務名稱、後端 IP 與 port、健康狀態與檢查失敗原因。手冊只說「預設綁 loopback」，要靠防火牆自己擋；daemon 對 DNS 綁非 loopback 會警告，對 `/metrics` 卻不會。封閉環境（電信機房）常讓 Prometheus 從另一台主機抓取，又不想讓整個網段都讀得到。

## What Changes

- `[metrics]` 新增 `allow`：CIDR 清單（IPv4 與 IPv6，例如 `["10.0.0.0/8", "fd00::/8"]`）。設了之後，來源位址不在清單內的連線只收到 `403 Forbidden`，不給任何指標；沒設則行為與現在相同（任何能連到的來源都可讀）。
- `[metrics] listen` 有非 loopback 位址且沒設 `allow` 時，啟動與 reload 都記一則警告，措辭與 DNS 的非 loopback 警告一致。
- `allow` 與 `listen` 一樣，變更需要重啟：reload 被拒絕，訊息 `metrics allow settings require restart`。
- 驗證：`allow` 必須是非空字串陣列，每筆是 CIDR（位址加 `/前綴長度`），不得重複；`[metrics]` 的未知鍵仍報 `unknown key`。
- 手冊（第 5、12、13 章，中英）與 CHANGELOG 更新。第 13 章目前寫「No authentication」，改為說明 `allow` 的用法與限制。

## Non-Goals

- bearer token、TLS 或 mTLS：已評估，成本（憑證管理、輪替、相依）高於這個變更要解的問題，不做。
- DNS 的存取控制：另一個問題，不動（DNS 有自己的 `rate_limit` 與非 loopback 警告）。
- 在 `allow` 之外對 `/metrics` 的路徑、方法做任何改變。
- 新增相依套件：CIDR 比對自己實作。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `metrics-endpoint`: 新增 requirement「Metrics source allow-list」（`allow` 的行為、403、警告、重啟規則）。
- `service-config`: requirement「Config validation rules」對 `[metrics]` 多一個鍵 `allow` 與它的驗證規則。

## Impact

- Affected specs: metrics-endpoint, service-config
- Affected code:
  - Modified: crates/emfor-daemon/src/config.rs, crates/emfor-daemon/src/metrics.rs, crates/emfor-daemon/src/cli.rs, docs/manual/05-configuring.md, docs/manual/12-operating.md, docs/manual/13-security.md, docs/manual.zh_TW/05-configuring.md, docs/manual.zh_TW/12-operating.md, docs/manual.zh_TW/13-security.md, CHANGELOG.md
  - New: none
  - Removed: none
