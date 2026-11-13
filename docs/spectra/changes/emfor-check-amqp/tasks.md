## 1. amqp plugin

- [x] 1.1 Plugin selection by argument 接受 `amqp`：`Kind::Amqp` 加入 crates/emfor-check/src/lib.rs，usage 與 Cargo.toml description 列出 amqp；以 tests/cli.rs 的 handshake 測試驗證（本 change 必須在 emfor-check-smtp 封存之後開始）
- [x] 1.2 AMQP check options：在新的 crates/emfor-check/src/amqp.rs 解析 `user`、`password_file`、`vhost` 與 TLS 選項；以 amqp.rs 單元測試涵蓋 spec 範例表 AMQP option validation 每一列 [after: 1.1]
- [x] 1.3 AMQP check：依「frame 自己編解碼並限制大小」寫 frame 與 method 的編解碼，依「握手做到 Open-Ok 後以 Close 結束」、「只用 PLAIN 登入」、「宣告 authentication_failure_close 以取得登入失敗的原因」與「broker 送 Close 時依階段決定原因前綴」實作握手；以 amqp.rs 單元測試（本機假 broker）涵蓋 spec 範例表 AMQP broker answers 每一列，檢查送出的 Start-Ok 與 Tune-Ok 內容，並確認拿掉階段前綴時 `vhost:` 的測試失敗 [after: 1.2]

## 2. 實機測試

- [x] 2.1 tests/plugins/Dockerfile 加入 rabbitmq-server（重建 emfor-plugins-test 映像前先徵求同意），tests/plugins/inside.sh 啟動 RabbitMQ（5672 明文、5671 TLS 用測試憑證），建立帳號 `emfor`、vhost `app`（有權限）與 `locked`（無權限）；crates/emfor-check/tests/live.rs 新增 `amqp_live`：`app` 為 up、錯誤密碼為 `auth: 403`、`locked` 為 `vhost: 530`、5671 `tls = true` 加測試 CA 為 up；以 tests/plugins/run.sh 通過驗證 [after: 1.3]

## 3. 文件與發行說明

- [x] 3.1 docs/deploy/plugins.md 新增 amqp 一節（選項、握手步驟、所有原因、不涵蓋記憶體或磁碟警報、只以 RabbitMQ 驗證），docs/manual/04-health-checks.md 與 docs/manual.zh_TW/04-health-checks.md 的 plugin 表加入 amqp，examples/emfor.toml 加一個 amqp 服務，CHANGELOG.md 未發行段落記錄；以 `emfor check-config` 檢查範例設定、macOS 與 Debian 映像的 fmt、clippy、test 全部通過驗證 [after: 2.1]
