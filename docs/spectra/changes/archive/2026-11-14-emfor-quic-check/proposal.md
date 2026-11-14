## Why

越來越多服務改用 QUIC（UDP）對外，例如 HTTP/3 的網站與 API。emfor 目前的檢查全部走 TCP：`tcp`、`http`、`grpc` 等都無法得知 UDP port 上的 QUIC 是否可用。QUIC 掛掉、防火牆擋掉 UDP 或憑證過期時，TCP 上的 HTTPS 可能仍然正常，所以 emfor 會繼續把流量導到 HTTP/3 已經不通的 backend。

## What Changes

- `emfor-check` 新增 `quic` plugin（`command = ["/usr/local/libexec/emfor-check", "quic"]`），用 `check = "quic"` 使用。它不內建在 daemon，與 `grpc` 一樣在 plugin process 裡執行。
- 每次檢查對 backend 的 UDP port 做一次完整的 QUIC v1 握手（不用 0-RTT），帶上 `alpn` 選項列出的 ALPN（預設 `["h3"]`），並依 `tls_verify`、`server_name`、`ca_file` 驗證憑證（QUIC 只用 TLS 1.3）。握手完成且協商出的 ALPN 在清單內就是 `up`，之後以 application error code 0 關閉連線。
- 選項 `http3 = true`（預設 `false`，`alpn` 必須包含 `h3`）會在握手後送出一個 HTTP/3 `GET`：`path`（預設 `/`）、`host_header`，並依 `expect_status`（預設 `["200-399"]`）與 `expect_body` 判斷，語意與內建 `http` check 相同。沒有 `http3 = true` 時設定這些 HTTP 選項是設定錯誤。
- 失敗原因（reason）分得出：逾時、版本協商失敗、TLS alert 或憑證錯誤、ALPN 不符、對方關閉連線或 stateless reset、HTTP/3 錯誤、status 或 body 不符。手冊說明 UDP 沒有「connection refused」：port 沒開、防火牆擋掉與 server 掛掉通常都是逾時。
- 新增依賴：`quinn`（QUIC，使用 repo 現有的 rustls 0.23）、`h3` 與 `h3-quinn`（HTTP/3，與 `grpc` 已在用的 `h2` 同一家族），都鎖定確切版本，只加在 `crates/emfor-check`。
- 手冊第 4 章（英文與中文）、`docs/deploy/plugins.md`、`examples/emfor.toml` 與 `CHANGELOG.md` 說明 `quic` plugin。

## Non-Goals

- DNS over QUIC：之後可讓 `dns` plugin 選擇 QUIC 傳輸，另開 change。
- 0-RTT、QUIC v2、連線在多次檢查間重用：健康檢查要的是每次都完整地建立連線。
- `GET` 以外的 HTTP/3 方法、request body、自訂 header（`host_header` 除外）。
- daemon 內建 QUIC check：quinn 是完整的 UDP 傳輸層，放在 plugin 才不會擴大 daemon 的依賴與攻擊面。
- 自己實作 QUIC 或 HTTP/3 frame 的解析：一律交給 quinn 與 h3。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `network-checks`: 新增 `quic` plugin 的選項與檢查行為兩個 requirement。
- `database-checks`: requirement「Plugin selection by argument」的 plugin 名單加入 `quic`，並新增它的 handshake 情境。

## Impact

- 受影響的 spec：`network-checks`、`database-checks`
- 受影響的程式碼：
  - crates/emfor-check/src/quic.rs（新檔）
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/options.rs
  - crates/emfor-check/src/main.rs
  - crates/emfor-check/Cargo.toml
  - Cargo.lock
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
  - tests/plugins/Dockerfile
  - tests/plugins/inside.sh
  - tests/plugins/run.sh
  - docs/manual/04-health-checks.md
  - docs/manual.zh_TW/04-health-checks.md
  - docs/deploy/plugins.md
  - examples/emfor.toml
  - CHANGELOG.md
- 依賴：`quinn`、`h3`、`h3-quinn`（只在 emfor-check）。daemon、client 與 store 格式不變。
