## Why

MongoDB replica set 的應用程式需要知道哪一台是 primary，也需要確認帳號登得進去。用 `tcp` 只能確認 port 有開。用 `exec` 跑 `mongosh` 則每次檢查都要起一個很重的程序。`mongodb` plugin 讓 MongoDB 和 redis、MySQL、PostgreSQL 一樣，可以用 `role` 搭配 `failover` 跟著 primary 走。

## What Changes

- `emfor-check` 新增 `mongodb` plugin，每次檢查：送 `hello` 指令 → 有帳號時以 SCRAM-SHA-256 登入 → 依 `role` 比對角色。
- 選項：`user` 與 `password_file`（同時設或同時不設）、`auth_source`（預設 `admin`）、`role`（`primary` 或 `secondary`）、`tls`、`tls_verify`、`server_name`、`ca_file`。
- 原因：`auth: `、`role is <實際>, want <要求>`、`query: `、`connect: `、`tls: `、`timeout`。
- 實機測試加入 MongoDB（兩台的 replica set 與一台需登入的伺服器）；文件、範例、CHANGELOG 一併更新。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `database-checks`：Plugin selection by argument 接受 `mongodb`。
- `network-checks`：新增 MongoDB check options 與 MongoDB check。

## Impact

- 程式：crates/emfor-check/src/mongodb.rs（新，含最小的 BSON 編解碼與 SCRAM-SHA-256）、crates/emfor-check/src/lib.rs、crates/emfor-check/src/main.rs、crates/emfor-check/Cargo.toml。
- 相依：新增直接相依 `sha2 = "0.11"` 與 `base64`，兩者都已在 Cargo.lock 中（版本不變，不需下載新的版本）；HMAC 用既有的 `hmac`。
- 測試：mongodb.rs 單元測試（RFC 7677 的 SCRAM 範例、本機假伺服器）、crates/emfor-check/tests/live.rs、crates/emfor-check/tests/cli.rs、tests/plugins/Dockerfile（加入 MongoDB 官方套件庫，需重建映像並下載）、tests/plugins/inside.sh。
- 文件：docs/deploy/plugins.md、docs/manual/04-health-checks.md、docs/manual.zh_TW/04-health-checks.md、examples/emfor.toml、CHANGELOG.md。
- 順序：必須在 emfor-check-amqp 之後、emfor-check-grpc 之前實作與封存（同樣修改 Plugin selection by argument）。
