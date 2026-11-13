## Context

一次 gRPC 呼叫是一個 HTTP/2 `POST`：

- path 為 `/<package>.<Service>/<Method>`；
- header 帶 `content-type: application/grpc` 與 `te: trailers`；
- 內容是 5 位元組的前綴（壓縮旗標 1 位元組、長度 4 位元組 big-endian）加上 protobuf 訊息。

伺服器回 HTTP 200，內容是同樣格式的訊息，最後以 trailers 帶 `grpc-status`（0 為成功）與可選的 `grpc-message`（percent-encoded）。失敗時伺服器可以只送一組 header，裡面就有 `grpc-status`（Trailers-Only）。

健康檢查協定 `grpc.health.v1`：

- `HealthCheckRequest { string service = 1; }`；
- `HealthCheckResponse { ServingStatus status = 1; }`，其中 `UNKNOWN = 0`、`SERVING = 1`、`NOT_SERVING = 2`、`SERVICE_UNKNOWN = 3`。

服務沒有註冊健康服務時回 `grpc-status` 12（UNIMPLEMENTED）；`service` 不認得時回 5（NOT_FOUND）。

`emfor-check` 現有的 plugin 都不需要 HTTP/2。Cargo.lock 裡沒有 HTTP/2 的實作，有 `http`、`bytes` 等周邊套件。

## Goals / Non-Goals

**Goals:**

- 以標準健康協定判斷 gRPC 服務，包含指定子服務；
- 明文（h2c prior knowledge）與 TLS（ALPN `h2`）都支援；
- 原因能分辨「服務回報不健康」、「沒有健康服務」與「根本不是 gRPC」。

**Non-Goals:**

- `Watch` 串流、gRPC reflection、呼叫任意方法；
- 用戶端憑證（mTLS）、自訂 metadata（例如驗證用的 token）、自訂 `:authority`；
- 壓縮的訊息；
- 經由 HTTP/1.1 升級（h2c upgrade）的連線。

## Decisions

### 以 h2 套件處理 HTTP/2

HTTP/2 需要 HPACK（含 Huffman 解碼與動態表）、流量控制與 frame 狀態機。自己寫的份量和出錯的風險遠大於 SMTP 或 AMQP。`h2` 是 hyper 與 tonic 底層使用的實作，相依大多已在 Cargo.lock 中，只需新增 `h2` 與 `fnv`。只連結進 `emfor-check`，daemon 不受影響。

替代方案：
- `tonic`：否決，需要 prost 與 tower 等大量相依，而健康檢查只要一個固定格式的請求；
- 自己實作 HTTP/2：否決，理由如上。

### protobuf 手寫編解碼

請求只有一個欄位：tag `0x0a`、長度、`service` 的位元組。回覆只需讀欄位 1 的 varint，其他欄位依 wire type 跳過；沒有欄位 1 時視為 0（UNKNOWN），這是 protobuf 的預設值。不引入 prost。

### TLS 用 ALPN h2，明文用 prior knowledge

`tls = true` 時，從 emfor-tls 的 `client_config(tls)` 複製一份設定並加上 ALPN `h2`，以 `handshake_with_config` 握手。伺服器沒有選 `h2` 時為 `tls: server did not select h2 (ALPN)`。

明文時直接送 HTTP/2 connection preface。gRPC 的 h2c 都是這種方式。

`:authority` 為 `server_name`（有設時）或後端位址 `ip:port`（IPv6 加中括號）。

### 回覆依層級判斷，先協定再狀態

依序判斷：

1. HTTP/2 連線或 stream 錯誤：`query: HTTP/2: <錯誤>`；
2. HTTP 狀態不是 200：`query: HTTP status <n>`；
3. `grpc-status` 不是 0：`grpc: <代碼> <名稱>[: <grpc-message 解碼後>]`；
4. 沒有 `grpc-status`：`query: no grpc-status`；
5. 訊息格式：
   - 壓縮旗標為 1：`query: compressed message`；
   - 長度超過 65536：`query: reply too large`；
   - 沒有訊息：`query: no message`；
   - protobuf 解不開：`query: malformed message`；
6. 健康狀態不是 SERVING：`status: <名稱>`，不認得的數值寫成 `status: <n>`。

`content-type` 不以 `application/grpc` 開頭時（例如一般 HTTP/2 網站），在第 3 步之前為 `query: not a gRPC response (content-type <值>)`。

## Implementation Contract

**行為**

- 每次檢查開一條新的 TCP 連線（`tls = true` 時先 TLS 握手並協商 `h2`）；
- 建立 HTTP/2 連線 → 送一個 `Check` 請求 → 讀完回覆與 trailers → 關閉連線；
- 健康狀態為 SERVING 才是 `up`。

**選項**

- `service`：0 到 256 位元組的字串，預設空字串，否則為 `must be at most 256 bytes`；
- `tls`、`tls_verify`、`server_name`、`ca_file`：規則與 redis、mysql、pgsql 相同（`requires tls = true`）；
- 其他 key 為 `unknown key`。

**請求**

- `POST /grpc.health.v1.Health/Check`；
- header：`content-type: application/grpc`、`te: trailers`、`user-agent: emfor-check/<版本>`；
- 內容：`00`、長度、`HealthCheckRequest`。

**原因**

- `status: NOT_SERVING`、`status: UNKNOWN`、`status: SERVICE_UNKNOWN`、`status: <n>`
- `grpc: 12 UNIMPLEMENTED`、`grpc: 5 NOT_FOUND: unknown service`（形式為 `grpc: <代碼> <名稱>[: <訊息>]`）
- `query: HTTP/2: ...`、`query: HTTP status <n>`、`query: not a gRPC response (content-type <值>)`
- `query: no grpc-status`、`query: compressed message`、`query: reply too large`、`query: no message`、`query: malformed message`
- `tls: server did not select h2 (ALPN)`、`tls: ...`、`connect: ...`、`timeout`

**驗收**

- grpc.rs 單元測試以 `h2` server 做的本機假伺服器涵蓋 spec 範例表每一列（含 TLS 與 ALPN）；拿掉健康狀態判斷時，`status: NOT_SERVING` 的測試會失敗；
- tests/plugins/run.sh 對 python3-grpcio 寫的伺服器驗證：
  - 明文 `service = ""` 為 `up`；
  - `service = "down"` 為 `status: NOT_SERVING`；
  - 沒有健康服務的 port 為 `grpc: 12 UNIMPLEMENTED`；
  - TLS port 以 `tls = true` 加測試 CA 為 `up`；
- tests/cli.rs 的 handshake 測試、macOS 與 Debian 映像的 fmt、clippy、test 全部通過。

**範圍**

- 範圍內：grpc plugin、選項、文件、範例、CHANGELOG、實機測試；
- 範圍外：Non-Goals 所列各項。

## Risks / Trade-offs

- [新增相依 `h2` 與 `fnv` 需要下載] → 實作時先徵求同意再抓套件；版本記在 Cargo.lock。
- [需要 token 或 mTLS 的 gRPC 服務無法檢查] → 文件寫明不支援；之後有需求再加 metadata 或用戶端憑證。
- [python3-grpcio 不是所有實作的代表] → 文件寫明只以 grpcio（Python）驗證，其他實作未測。
