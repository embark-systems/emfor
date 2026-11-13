## Why

gRPC 服務的標準健康介面是 `grpc.health.v1.Health/Check`。Kubernetes、Envoy 與各語言的 gRPC 函式庫都支援它，服務也能分別回報每個子服務是否 `SERVING`。emfor 現在只能用 `tcp` 確認 port 有開，看不到服務主動回報的 `NOT_SERVING`，例如正在關機或依賴的資料庫斷了。

## What Changes

- `emfor-check` 新增 `grpc` plugin：每次檢查開一條 HTTP/2 連線（明文 h2c 或 TLS 加 ALPN `h2`），呼叫 `grpc.health.v1.Health/Check`，回覆 `SERVING` 為 `up`。
- 選項：`service`（預設空字串，代表整個伺服器）、`tls`、`tls_verify`、`server_name`、`ca_file`。
- 原因：`status: NOT_SERVING` 等健康狀態、`grpc: <代碼> <名稱>[: 訊息]`、`query: `、`connect: `、`tls: `、`timeout`。
- 實機測試加入一個以 python3-grpcio 寫的小型 gRPC 伺服器；文件、範例、CHANGELOG 一併更新。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `database-checks`：Plugin selection by argument 接受 `grpc`。
- `network-checks`：新增 gRPC check options 與 gRPC check。

## Impact

- 程式：crates/emfor-check/src/grpc.rs（新）、crates/emfor-check/src/lib.rs、crates/emfor-check/src/main.rs、crates/emfor-check/Cargo.toml。
- 相依：新增 `h2`（HTTP/2，需下載 `h2` 與 `fnv` 兩個新套件；其餘相依已在 Cargo.lock）與已在 Cargo.lock 的 `http`、`bytes`。只連結進 `emfor-check`。
- 測試：grpc.rs 單元測試（以 `h2` 的 server 端做本機假伺服器）、crates/emfor-check/tests/live.rs、crates/emfor-check/tests/cli.rs、tests/plugins/Dockerfile（加 python3-grpcio，需重建映像）、tests/plugins/inside.sh 與一個測試用的 Python 伺服器。
- 文件：docs/deploy/plugins.md、docs/manual/04-health-checks.md、docs/manual.zh_TW/04-health-checks.md、examples/emfor.toml、CHANGELOG.md。
- 順序：必須在 emfor-check-mongodb 之後實作與封存（同樣修改 Plugin selection by argument）。
