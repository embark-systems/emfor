## 1. grpc plugin

- [x] 1.1 Plugin selection by argument 接受 `grpc`：`Kind::Grpc` 加入 crates/emfor-check/src/lib.rs，usage 與 Cargo.toml description 列出 grpc；以 tests/cli.rs 的 handshake 測試驗證（本 change 必須在 emfor-check-mongodb 封存之後開始）
- [x] 1.2 gRPC check options：在新的 crates/emfor-check/src/grpc.rs 解析 `service` 與 TLS 選項；以 grpc.rs 單元測試涵蓋 spec 範例表 gRPC option validation 每一列 [after: 1.1]
- [x] 1.3 依「以 h2 套件處理 HTTP/2」在 crates/emfor-check/Cargo.toml 加入 `h2`、`http`、`bytes`（下載 `h2` 與 `fnv` 前先徵求同意），依「TLS 用 ALPN h2，明文用 prior knowledge」建立連線，依「protobuf 手寫編解碼」組請求與解回覆；以 `cargo tree -p emfor-check` 確認只新增 `h2` 與 `fnv`，並以單元測試驗證請求的 path、header 與內容位元組 [after: 1.2]
- [x] 1.4 gRPC check：依「回覆依層級判斷，先協定再狀態」判斷結果；以 grpc.rs 單元測試（`h2` server 端做的本機假伺服器，含 TLS 與 ALPN）涵蓋 spec 範例表 gRPC responses 每一列，並確認拿掉健康狀態判斷時 `status: NOT_SERVING` 的測試失敗 [after: 1.3]

## 2. 實機測試

- [x] 2.1 tests/plugins/Dockerfile 加入 python3-grpcio（重建 emfor-plugins-test 映像前先徵求同意），新增 tests/plugins/grpc_server.py：以 generic handler 回應原始位元組的健康服務，50051 明文（`""` 為 SERVING、`down` 為 NOT_SERVING）、50052 TLS（測試憑證）、50053 沒有健康服務；tests/plugins/inside.sh 啟動它；crates/emfor-check/tests/live.rs 新增 `grpc_live` 涵蓋這四種情況；以 tests/plugins/run.sh 通過驗證 [after: 1.4]

## 3. 文件與發行說明

- [x] 3.1 docs/deploy/plugins.md 新增 grpc 一節（選項、請求內容、判斷順序與所有原因、不支援 token、mTLS 與壓縮、只以 grpcio 驗證），docs/manual/04-health-checks.md 與 docs/manual.zh_TW/04-health-checks.md 的 plugin 表加入 grpc，examples/emfor.toml 加一個 grpc 服務，CHANGELOG.md 未發行段落記錄；以 `emfor check-config` 檢查範例設定、macOS 與 Debian 映像的 fmt、clippy、test 全部通過驗證 [after: 2.1]
