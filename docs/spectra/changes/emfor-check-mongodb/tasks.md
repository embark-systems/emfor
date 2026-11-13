## 1. mongodb plugin

- [x] 1.1 Plugin selection by argument 接受 `mongodb`：`Kind::Mongodb` 加入 crates/emfor-check/src/lib.rs，usage 與 Cargo.toml description 列出 mongodb；以 tests/cli.rs 的 handshake 測試驗證（本 change 必須在 emfor-check-amqp 封存之後開始）
- [x] 1.2 MongoDB check options：在新的 crates/emfor-check/src/mongodb.rs 解析 `user`、`password_file`、`auth_source`、`role` 與 TLS 選項；以 mongodb.rs 單元測試涵蓋 spec 範例表 MongoDB option validation 每一列 [after: 1.1]
- [x] 1.3 依「BSON 只做需要的型別」寫 BSON 編解碼與 OP_MSG 讀寫（長度上限、checksum 位元）；以單元測試驗證編解碼來回、跳過不需要的型別、不認得的型別碼為 `query: malformed reply` [after: 1.2]
- [x] 1.4 依「SCRAM-SHA-256 自己實作並用 RFC 7677 範例測試」與「密碼限定可列印 ASCII」實作 SCRAM（Cargo.toml 加入已在 Cargo.lock 的 `sha2 = "0.11"` 與 `base64`，以 `cargo tree` 確認沒有新增版本）；以 spec 的 SCRAM example from RFC 7677 情境為單元測試，並確認拿掉 server signature 驗證時測試失敗 [after: 1.3]
- [x] 1.5 MongoDB check：依「先送 hello，有帳號再登入」與「角色取自 hello 的欄位」組合整個檢查；以 mongodb.rs 單元測試（本機假伺服器）涵蓋 spec 範例表 MongoDB replies 每一列 [after: 1.4]

## 2. 實機測試

- [x] 2.1 tests/plugins/Dockerfile 加入 MongoDB 官方套件庫的 mongodb-org 與 mongosh（改 Dockerfile 與重建 emfor-plugins-test 映像前先徵求同意，記錄版本），tests/plugins/inside.sh 啟動兩台 replica set `rs0`（27017、27018，27017 以 TLS 另開 27019 或以 `--tlsMode allowTLS` 接受 TLS）與一台需登入的伺服器（27020，帳號 `emfor`）；crates/emfor-check/tests/live.rs 新增 `mongodb_live`：primary 與 secondary 的兩種 `role`、登入成功與錯誤密碼、`tls = true` 加測試 CA；以 tests/plugins/run.sh 通過驗證 [after: 1.5]

## 3. 文件與發行說明

- [x] 3.1 docs/deploy/plugins.md 新增 mongodb 一節（選項、角色判斷表、所需權限、最低版本 4.4、只支援 SCRAM-SHA-256 與 ASCII 密碼、驗證用的 MongoDB 版本），docs/manual/04-health-checks.md 與 docs/manual.zh_TW/04-health-checks.md 的 plugin 表與 role 說明加入 mongodb，examples/emfor.toml 加一個 `role = "primary"` 搭配 `failover` 的服務，CHANGELOG.md 未發行段落記錄；以 `emfor check-config` 檢查範例設定、macOS 與 Debian 映像的 fmt、clippy、test 全部通過驗證 [after: 2.1]
