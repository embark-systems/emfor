## Context

`emfor-check` 是 emfor 的 plugin 執行檔，以第一個參數選擇 plugin（`redis`…`grpc`），透過 checker-plugins protocol 1 接收檢查請求。各 plugin 的選項驗證與檢查在 `crates/emfor-check/src/<plugin>.rs`，由 `lib.rs` 的 `Kind` 分派；`grpc` 用 `h2` crate 與 `emfor-tls`（rustls 0.23，`aws-lc-rs`）完成 HTTP/2 與 TLS。plugin 的 tokio runtime 由 `emfor-plugin` 建立（multi-thread），每個檢查是一個 task，逾時由 plugin 依 `timeout_ms` 回 `down`、reason `timeout`。latency 由 daemon 從檢查開始計時到收到結果（state-store requirement「Check latency」），plugin 不回報 latency。

現有檢查全部走 TCP，沒有任何方法確認 UDP 上的 QUIC／HTTP/3 是否可用。

## Goals / Non-Goals

**Goals:**

- `emfor-check quic`：每次檢查做一次完整的 QUIC v1 握手，驗證憑證與 ALPN。
- `http3 = true` 時再送一個 HTTP/3 `GET`，以 `expect_status`／`expect_body` 判斷，語意與內建 `http` check 相同。
- reason 分得出逾時、TLS、QUIC 傳輸層與 HTTP/3 層的失敗。
- 只在 `crates/emfor-check` 新增依賴；daemon、client、store 格式不變。

**Non-Goals:**

- DNS over QUIC、0-RTT、QUIC v2、跨檢查重用連線。
- `GET` 以外的方法、request body、`host_header` 以外的自訂 header。
- daemon 內建 QUIC check。
- 自己解析 QUIC 或 HTTP/3 frame。

## Decisions

### QUIC check 放在 emfor-check plugin，不內建在 daemon

quinn 是完整的 UDP 傳輸層與連線狀態機。放在 plugin process 裡，daemon 的依賴、二進位大小與攻擊面不變，plugin 崩潰也只影響 QUIC 檢查。這與 `grpc`、`dns` 等協定檢查的放法一致。

替代方案：在 daemon 的 `checker` 模組內建。daemon 目前只內建 `tcp`、`http`、`exec`，加入 QUIC 會讓所有部署都帶上 quinn，不採用。

### 用 quinn 與 h3／h3-quinn，鎖定確切版本

quinn 直接使用 rustls 0.23，可以沿用 `emfor-tls` 的 `TlsSettings`、`client_config`、`server_name`，再以 `quinn::crypto::rustls::QuicClientConfig::try_from` 轉成 QUIC 設定（只保留 TLS 1.3）。HTTP/3 用 hyperium 的 `h3` 與 `h3-quinn`，與 `grpc` 用的 `h2` 同一家族。`h3` 仍是 0.0.x，所以在 `Cargo.toml` 用 `=` 鎖定確切版本，升級時要重新跑本 change 的測試。

替代方案：自己寫精簡的 HTTP/3（QPACK 只用靜態表）。可行，但多一套自己維護的 frame 解析與 fuzz target，不採用。reqwest 的 `http3` 功能需要 `--cfg reqwest_unstable`，也不採用。

### 每個 address family 共用一個 quinn endpoint

plugin process 第一次需要時，為 IPv4 綁定 `0.0.0.0:0`、為 IPv6 綁定 `[::]:0` 各一個 `quinn::Endpoint`（存在 `tokio::sync::OnceCell`），之後的檢查都用它建立新連線。這樣同時進行的大量檢查不會各開一個 UDP socket。綁定失敗是本機問題：該次檢查回 `error`，並在下次檢查時重試綁定。

### ALPN 不符就是 TLS 失敗

QUIC 規定（RFC 9001 §8.1）server 找不到共同的 ALPN 時必須以 `no_application_protocol` alert 中止握手，所以「握手成功但 ALPN 不在清單內」不會發生；這種情況以 TLS alert 呈現，reason 為 `tls: no application protocol (ALPN)`。為防範不合規的 server，握手後仍檢查協商出的 ALPN 是否在清單內。

### HTTP/3 選項沿用 http check 的語意

`path`、`host_header`、`expect_status`、`expect_body` 的意義、預設值與 reason 文字與內建 `http` check 相同（health-checking requirement「HTTP checker」），使用者不必學兩套。這些選項只在 `http3 = true` 時有效，否則是設定錯誤，避免以為有檢查 HTTP 其實只做了握手。

## Implementation Contract

**行為**

- 設定：

  ```toml
  [plugins."quic"]
  command = ["/usr/local/libexec/emfor-check", "quic"]

  [services."web.svc.emfor"]
  check = "quic"
  backends = ["10.0.9.11:443", "10.0.9.12:443"]
  check_options = { http3 = true, path = "/healthz", server_name = "web.internal" }
  ```

- 每次檢查：用共用 endpoint 對 backend `ip:port` 建立新 QUIC 連線，server name 依 `emfor_tls::server_name`（`server_name` 或 backend IP），ALPN 為 `alpn` 清單，不使用 0-RTT。
- 只握手（`http3 = false`）：握手完成且協商出的 ALPN 在清單內 → `up`；之後以 application close code `0` 關閉連線。
- `http3 = true`：握手後以 h3 送出 `GET path`，`:authority` 為 `host_header`，否則 `server_name`，否則 backend `ip:port`（IPv6 加中括號），並帶 `user-agent: emfor-check/<package version>`。status 符合 `expect_status` 時，若設了 `expect_body`，就讀 body（最多 65536 bytes，找到即停）。之後以 HTTP/3 的 `H3_NO_ERROR`（0x100）關閉連線。

**選項（`check_options`）**

| Key | 型別 | 預設 | 錯誤 |
| --- | --- | --- | --- |
| `alpn` | 字串陣列 | `["h3"]` | `must be a non-empty array`；元素不是 1 到 255 bytes 的字串：`must be strings of 1 to 255 bytes` |
| `tls_verify`、`server_name`、`ca_file` | 同 `grpc` 的規則 | 同 `grpc` | 同 `grpc`；沒有 `tls` 鍵（QUIC 永遠加密），`tls` 是 `unknown key` |
| `http3` | boolean | `false` | `alpn` 不含 `h3`：`requires "h3" in alpn` |
| `path` | 以 `/` 開頭的字串 | `/` | `must start with /`；沒有 `http3 = true`：`requires http3 = true` |
| `host_header` | 1 到 256 bytes、不含控制字元的字串 | 無 | `must be 1 to 256 bytes without control characters`；`requires http3 = true` |
| `expect_status` | 狀態碼（整數或字串）或 `"lo-hi"` 範圍的非空陣列，都在 100 到 599 | `["200-399"]` | `must be a non-empty array`；`must be a status code from 100 to 599 or a range lo-hi`；`requires http3 = true` |
| `expect_body` | 1 到 16384 bytes 的字串 | 無 | `must be 1 to 16384 bytes`；`requires http3 = true` |

其他鍵是 `unknown key`；型別錯誤為 `must be a string`、`must be a boolean`。

**結果與 reason（依序，第一條符合者適用）**

1. 整個檢查超過 `timeout_ms`：`down`，`timeout`。
2. endpoint 無法綁定、`ca_file` 讀不到或沒有系統 CA：`error`（本機問題，network-checks requirement「Network check outcome classification」）。
3. 版本協商失敗：`down`，`quic: version negotiation failed`。
4. TLS 失敗：`down`，以 `tls: ` 開頭；ALPN 沒有交集為 `tls: no application protocol (ALPN)`；憑證驗證失敗包含 rustls 的錯誤說明。
5. 對方關閉連線：`down`，`quic: closed by peer: <code>`，有 reason 文字時加上 `: <文字>`；stateless reset：`quic: stateless reset`；其他傳輸錯誤：`quic: <quinn 的錯誤說明>`。
6. 協商出的 ALPN 不在清單內：`down`，`tls: server selected ALPN <值> not in alpn`。
7. `http3 = false`：`up`。
8. HTTP/3 連線或 stream 錯誤：`down`，`query: HTTP/3: <錯誤>`。
9. status 不符 `expect_status`：`down`，`unexpected status <status>`。
10. 讀 body 失敗：`down`，以 `body: ` 開頭；`expect_body` 沒找到：`down`，`expect_body: got "<body 前 64 bytes，escaped>"`。
11. 否則 `up`。

**驗收**

- `crates/emfor-check/src/quic.rs` 的單元測試以 in-process 的 quinn（加 h3）server 與 `emfor_tls::testing` 憑證，涵蓋：握手 up、ALPN 不符、憑證不受信任（`tls_verify = true` down、`false` up）、`server_name` 不符、沒人回應的 UDP port 逾時、http3 的 status 與 body 符合與不符、server 立即關閉連線。每個新測試都要 RED 檢查。
- 選項驗證的表格測試涵蓋上表每個錯誤。
- `crates/emfor-check/tests/cli.rs` 的 `quic` handshake。
- `tests/plugins` 的 live 測試以 Debian 套件的 HTTP/3 server 跑一次 http3 檢查。
- macOS 與 Debian 的 RELEASING.md 檢查與既有 fuzz target 通過。

**範圍**

- 在範圍內：`crates/emfor-check`、它的 Cargo 依賴、live 測試映像、手冊第 4 章、`docs/deploy/plugins.md`、`examples/emfor.toml`、`CHANGELOG.md`。
- 不在範圍內：daemon、client、store 格式、`dns` plugin、checker-plugins 協定。

## Implementation Notes

- `emfor-check` 的 release 二進位（macOS，aarch64）：加入 quinn 0.11.12、h3 0.0.8、h3-quinn 0.0.10 前 10,381,840 bytes，實作 `quic` 後 11,750,768 bytes，增加 1,368,928 bytes（約 1.31 MiB，13%）。`cargo tree -p emfor-daemon` 不含 quinn 與 h3。

- 對方關閉連線的測試：quinn 的 server 在握手前只能 `refuse`（`CONNECTION_REFUSED`，code 2，沒有 reason 文字），握手完成後才能以任意 application code 加 reason 關閉；而 client 的握手比 server 先完成，所以「握手期間以 0x10 與 `busy` 關閉」無法用 in-process server 重現。Example 改為兩列：`refuse` → `quic: closed by peer: 2`；`http3 = true` 時握手後以 0x10、`busy` 關閉 → `quic: closed by peer: 16: busy`（HTTP/3 因連線被關閉而失敗時，依規則 5 以關閉原因回報，而不是規則 8 的 `query: HTTP/3: `）。
- 規則 6（協商出的 ALPN 不在清單內）只會發生在不合規的 server；符合 RFC 9001 的 server 會先以 alert 中止握手，所以沒有測試能觸發它。

## Risks / Trade-offs

- [h3 仍是 0.0.x，API 可能變動] → 鎖定確切版本；升級時重跑本 change 的測試。
- [UDP 沒有 connection refused] → port 沒開、防火牆、server 掛掉都只會是 `timeout`，檢查最多花滿 `timeout_ms`。手冊說明；ICMP port unreachable 若被 quinn 回報，就照 reason 5 呈現。
- [二進位變大] → quinn、h3 只加在 emfor-check。加入前後各量一次 `emfor-check` release 二進位大小並寫進 CHANGELOG。實測（macOS aarch64）增加 1,368,928 bytes，約 1.31 MiB、13%，比原先估計的數百 KB 大，見 Implementation Notes。
- [共用 endpoint 的 socket 錯誤影響所有檢查] → 綁定失敗回 `error`（不算 backend 失敗），下次重試；endpoint 運作中的錯誤只影響當時的連線。
- [live 測試需要 HTTP/3 server] → 先確認 Debian bookworm 的 `caddy` 套件可用並預設開啟 HTTP/3；不可用時改用 `python3-aioquic` 的範例 server，兩者都不行就回報，不跳過。

- 關閉順序：完整測試在負載下偶爾讓 `http3_closes_with_h3_no_error` 失敗，server 看到的是自己關閉（`LocallyClosed`），而不是 client 的 `H3_NO_ERROR`。原因是 client 先停掉 h3 控制 stream 的 task 才關閉連線，server 先看到控制 stream 結束，就以 H3_CLOSED_CRITICAL_STREAM 自行關閉。在停 task 後加 50 ms 延遲可穩定重現。現在 `request()` 先依規則 5 讀取對方的關閉原因（quinn 的本地 `close()` 會蓋掉它），再以 `H3_NO_ERROR` 關閉，最後才停 task；加同樣的延遲後 16 個測試都通過。另外測試 server 改為先讀完 request 再回應，否則它在 client 的 FIN 到達前丟掉 stream，會以 code 0 停止 stream，client 看到 `query: HTTP/3: Remote reset: 0x0`。

- verify 與 review 之後的修正：
  - `host_header` 與 `path` 會組進 request URI，所以讀選項時改用 `http` crate 驗證。`host_header` 必須能解析為 `http::uri::Authority`，而且不含 userinfo（RFC 9114 4.3.1 禁止），所以 `api.internal/x`、`a b`、`user@api.internal` 都會被拒絕，錯誤是 `must be a host with an optional port, 1 to 256 bytes`。`path` 必須能原樣解析為 `PathAndQuery`，所以 `/a b`、`/a#b` 會被拒絕，錯誤是 `must be a URL path without spaces, control characters or #`。這樣錯誤會在 `check-config` 出現，而不是每次檢查都回 `internal: HTTP/3 request: …`。
  - `server_name` 是 IPv6 位址時，`:authority` 會加上方括號。`grpc.rs` 也有同樣的問題，但不在本 change 範圍內。
  - 規則 5 先於規則 8 與 10：`query: ` 或 `body: ` 的失敗，只要連線的關閉原因不是本地關閉（`LocallyClosed`，例如 h3 自己因協定錯誤而關閉），就改用連線錯誤作為 reason。對方關閉、stateless reset 與 idle timeout 都算。測試 `peer_close_while_reading_the_body` 涵蓋對方在讀 body 時關閉的情況；idle timeout（quinn 預設 30 s）比測試的檢查逾時長，所以沒有測試。
  - 規則 10 由 `body_read_failure` 涵蓋（server 送出 header 後 reset stream）。
  - 規則 2（本地 UDP socket 開不起來）沒有測試：`Endpoint::client` 綁定的是 `0.0.0.0:0` 或 `[::]:0`，在測試中無法讓它穩定失敗；要測就得為了測試讓綁定位址可以注入，不值得。程式碼路徑只有一個 `map_err`。

