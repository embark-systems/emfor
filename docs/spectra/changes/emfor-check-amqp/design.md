## Context

AMQP 0-9-1 連線一開始：

1. 用戶端送 8 位元組的 protocol header `AMQP\x00\x00\x09\x01`；
2. broker 回 Connection.Start（列出支援的登入方式），用戶端回 Start-Ok（帶帳密）；
3. broker 回 Tune，用戶端回 Tune-Ok；
4. 用戶端送 Open 指定 vhost，broker 回 Open-Ok。

broker 拒絕時送 Connection.Close，帶回覆代碼與文字，例如 403 ACCESS_REFUSED、530 NOT_ALLOWED。header 版本不符時，broker 回自己的 protocol header 後關閉。

RabbitMQ 只有在用戶端的 client-properties 宣告 `capabilities.authentication_failure_close = true` 時，登入失敗才會送 Close 403；否則直接關閉連線。

每個 frame 的格式是：type（1 位元組）、channel（2）、size（4）、payload、結尾 `0xCE`。method frame 的 type 是 1，payload 以 class-id 與 method-id 開頭。

## Goals / Non-Goals

**Goals:**

- 確認 broker 接受這個帳號登入並開啟指定的 vhost；
- 原因分辨帳號問題（`auth:`）、vhost 問題（`vhost:`）與協定問題（`query:`）。

**Non-Goals:**

- AMQP 1.0（ActiveMQ Artemis、Azure Service Bus 等）、MQTT、STOMP；
- 開 channel、宣告或檢查 queue、發送或接收訊息；
- PLAIN 以外的登入方式（EXTERNAL、AMQPLAIN）與用戶端憑證。

## Decisions

### 握手做到 Open-Ok 後以 Close 結束

Open-Ok 代表 broker 已接受登入與 vhost，是應用程式開始工作前的最後一步，所以做到這裡就是 `up`。之後送 Connection.Close（reply-code 200），最多等 200 ms 收 Close-Ok 再關閉連線，讓 broker 的 log 記錄正常關閉，而不是 `client unexpectedly closed TCP connection`。

替代方案：連開 channel 一起檢查。否決，因為 channel 層級的問題（例如 channel 數上限）不代表 broker 不能用。

### 宣告 authentication_failure_close 以取得登入失敗的原因

Start-Ok 的 client-properties 帶 `product = "emfor"` 與 `capabilities = { authentication_failure_close: true }`，讓 RabbitMQ 在登入失敗時送 Close 403，原因就能是 `auth: 403 ACCESS_REFUSED - ...`。若 broker 仍在 Start-Ok 之後直接關閉連線，原因為 `auth: connection closed after login`。

### 只用 PLAIN 登入

Connection.Start 的 mechanisms 沒有 `PLAIN` 時為 `down`，原因 `auth: server does not offer PLAIN`。PLAIN 的 response 是 `\0<user>\0<password>`。密碼只從 `password_file` 讀，規則與其他 plugin 相同。

### frame 自己編解碼並限制大小

frame 讀取有上限：size 超過 131072 位元組為 `query: frame too large`，結尾不是 `0xCE` 為 `query: malformed frame`。

- 收到 heartbeat frame（type 8）時略過；
- 收到非預期的 method 時為 `query: unexpected method <class>.<method>`；
- broker 回 protocol header 而不是 frame 時為 `query: server speaks AMQP <a>-<b>-<c>`，三個數字是 header 最後三個位元組。

Tune-Ok 的值：channel-max 與 frame-max 回送 broker 提議的值（frame-max 為 0 時用 131072），heartbeat 回 0，因為連線只存在幾毫秒。

### broker 送 Close 時依階段決定原因前綴

- Start-Ok 之後、Tune 之前收到 Close：`auth: <代碼> <文字>`；
- Open 之後收到 Close：`vhost: <代碼> <文字>`；
- 其他時候：`query: <代碼> <文字>`。

收到 Close 時回 Close-Ok 再關閉連線。

## Implementation Contract

**行為**

- 每次檢查開一條新連線（`tls = true` 時先完成 TLS 握手），完成到 Open-Ok 為 `up`，再以 Close 正常結束；
- 任何一步失敗為 `down`；`password_file` 或 `ca_file` 在檢查時讀不到為 `error`。

**選項**

- `user`：必填，1 到 255 位元組；
- `password_file`：必填，規則與其他 plugin 相同；
- `vhost`：1 到 255 位元組，預設 `/`；
- `tls`、`tls_verify`、`server_name`、`ca_file`：規則與 redis、mysql、pgsql 相同（`requires tls = true`）；
- 其他 key 為 `unknown key`。

**原因**

- `auth: <代碼> <文字>`
- `auth: connection closed after login`
- `auth: server does not offer PLAIN`
- `vhost: <代碼> <文字>`
- `query: <代碼> <文字>`
- `query: unexpected method <class>.<method>`
- `query: frame too large`、`query: malformed frame`、`query: connection closed`
- `query: server speaks AMQP <a>-<b>-<c>`
- `connect: ...`、`tls: ...`、`timeout`

**驗收**

- amqp.rs 單元測試以本機假 broker 涵蓋 spec 範例表每一列，並確認送出的 Start-Ok 帶 PLAIN response 與 `authentication_failure_close`；
- tests/plugins/run.sh 對 RabbitMQ 驗證：
  - 正確帳密與 vhost 為 `up`；
  - 錯誤密碼為 `auth: 403 ...`；
  - 沒有權限的 vhost 為 `vhost: 530 ...`；
  - 5671 以 `tls = true` 加測試 CA 為 `up`；
- tests/cli.rs 的 handshake 測試、macOS 與 Debian 映像的 fmt、clippy、test 全部通過。

**範圍**

- 範圍內：amqp plugin、選項、文件、範例、CHANGELOG、實機測試；
- 範圍外：Non-Goals 所列各項。

## Risks / Trade-offs

- [RabbitMQ 在記憶體或磁碟警報時仍接受連線，只是阻擋發佈] → 這個狀態只在發佈時才看得出來（connection.blocked），不在本次範圍；文件註明 `amqp` 檢查不涵蓋警報。
- [Debian 的 rabbitmq-server 連同 Erlang 很大，映像會變大] → 實作時先徵求同意再重建映像；只用於本機實機測試。
- [每次檢查在 RabbitMQ log 留下連線與關閉的紀錄] → 文件說明，並建議視需要調整 `interval`。
