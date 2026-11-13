## Why

`tcp` 檢查加 `expect = "220"` 只能確認郵件伺服器會打招呼。伺服器拒絕 EHLO、STARTTLS 壞掉或憑證過期時，它仍然是 `up`，而送信的應用程式第一步就會失敗。`smtp` plugin 走一遍送信前的交談（不真的送信），讓這些情況都會被發現。

## What Changes

- `emfor-check` 新增 `smtp` plugin，每次檢查：讀 220 招呼 → `EHLO` → 可選 `STARTTLS` 與 TLS 握手、再 `EHLO` → `QUIT`。
- 選項：`helo`、`tls`（直接 TLS，例如 465 port）、`starttls`（與 `tls` 互斥）、`tls_verify`、`server_name`、`ca_file`。
- 原因：`greeting: `、`ehlo: `、`tls: STARTTLS not offered`、`tls: STARTTLS refused: `、`tls: `、`query: `、`connect: `、`timeout`。
- 實機測試加入 Postfix；文件（docs/deploy/plugins.md、手冊第 4 章中英文）、examples/emfor.toml、CHANGELOG 一併更新。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `database-checks`：Plugin selection by argument 接受 `smtp`。
- `network-checks`：新增 SMTP check options 與 SMTP check。

## Impact

- 程式：crates/emfor-check/src/smtp.rs（新）、crates/emfor-check/src/lib.rs、crates/emfor-check/src/main.rs（使用說明）、crates/emfor-check/Cargo.toml（description）。
- 測試：smtp.rs 單元測試（本機假伺服器）、crates/emfor-check/tests/live.rs、crates/emfor-check/tests/cli.rs、tests/plugins/Dockerfile（加 postfix，需重建映像）、tests/plugins/inside.sh。
- 文件：docs/deploy/plugins.md、docs/manual/04-health-checks.md、docs/manual.zh_TW/04-health-checks.md、examples/emfor.toml、CHANGELOG.md。
- 不新增 Rust 相依套件（TLS 用 emfor-tls）。
- 順序：smtp、amqp、mongodb、grpc 四個 change 都修改 database-checks 的 Plugin selection by argument，必須依 emfor-check-smtp → emfor-check-amqp → emfor-check-mongodb → emfor-check-grpc 的順序實作與封存。
