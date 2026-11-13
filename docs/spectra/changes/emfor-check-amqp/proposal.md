## Why

用 `tcp` 檢查 RabbitMQ 只能確認 port 有開。broker 拒絕登入、vhost 不存在、節點因記憶體或磁碟警報拒收連線時，它都還是 `up`，但應用程式連上去會立刻失敗。`amqp` plugin 完成一次 AMQP 0-9-1 的連線握手（登入並開啟 vhost），能看到這些情況。

## What Changes

- `emfor-check` 新增 `amqp` plugin，每次檢查：送 protocol header → Connection.Start／Start-Ok（PLAIN 登入）→ Tune／Tune-Ok → Open（vhost）／Open-Ok → Close／Close-Ok。
- 選項：`user`、`password_file`（兩者必填）、`vhost`（預設 `/`）、`tls`、`tls_verify`、`server_name`、`ca_file`。
- 原因：`auth: `、`vhost: `、`query: `、`connect: `、`tls: `、`timeout`。
- 實機測試加入 RabbitMQ；文件（docs/deploy/plugins.md、手冊第 4 章中英文）、examples/emfor.toml、CHANGELOG 一併更新。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `database-checks`：Plugin selection by argument 接受 `amqp`。
- `network-checks`：新增 AMQP check options 與 AMQP check。

## Impact

- 程式：crates/emfor-check/src/amqp.rs（新）、crates/emfor-check/src/lib.rs、crates/emfor-check/src/main.rs、crates/emfor-check/Cargo.toml（description）。
- 測試：amqp.rs 單元測試（本機假 broker）、crates/emfor-check/tests/live.rs、crates/emfor-check/tests/cli.rs、tests/plugins/Dockerfile（加 rabbitmq-server，需重建映像）、tests/plugins/inside.sh。
- 文件：docs/deploy/plugins.md、docs/manual/04-health-checks.md、docs/manual.zh_TW/04-health-checks.md、examples/emfor.toml、CHANGELOG.md。
- 不新增 Rust 相依套件：frame 自己編解碼，TLS 用 emfor-tls。
- 順序：必須在 emfor-check-smtp 之後、emfor-check-mongodb 之前實作與封存（同樣修改 Plugin selection by argument）。
