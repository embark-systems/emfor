## 1. smtp plugin

- [x] 1.1 Plugin selection by argument 接受 `smtp`：`Kind::Smtp` 加入 crates/emfor-check/src/lib.rs（`from_name`、`name`、validate 與 check 的分派），usage 與 Cargo.toml description 列出 smtp；以 tests/cli.rs 的 handshake 測試（`emfor-check smtp` 回 `name` 為 `smtp`）驗證
- [x] 1.2 SMTP check options：在新的 crates/emfor-check/src/smtp.rs 依「helo 預設用本機位址」解析 `helo`、`tls`、`starttls` 與 TLS 選項（TLS 鍵沿用 `options::tls_settings` 的規則，錯誤改為 `requires tls = true or starttls = true`）；以 smtp.rs 單元測試涵蓋 spec 範例表 SMTP option validation 每一列 [after: 1.1]
- [x] 1.3 SMTP check：依「交談步驟固定為招呼、EHLO、可選 STARTTLS、QUIT」、「starttls 要求 EHLO 宣告 STARTTLS」與「回覆解析有長度上限」實作交談與回覆解析，TLS 以 emfor-tls 握手；以 smtp.rs 單元測試（本機假伺服器，含以測試 CA 完成的 STARTTLS 與 `tls = true`）涵蓋 spec 範例表 SMTP replies 每一列，並確認拿掉 STARTTLS 宣告的檢查時 `tls: STARTTLS not offered` 的測試失敗 [after: 1.2]

## 2. 實機測試

- [x] 2.1 tests/plugins/Dockerfile 加入 postfix（重建 emfor-plugins-test 映像前先徵求同意），tests/plugins/inside.sh 設定 Postfix：2525 明文且有 STARTTLS（測試憑證）、4465 wrapper mode、2526 沒有 STARTTLS；crates/emfor-check/tests/live.rs 新增 `smtp_live`：2525 預設為 up、2525 `starttls = true` 加測試 CA 為 up、4465 `tls = true` 為 up、2526 `starttls = true` 為 `tls: STARTTLS not offered`；以 tests/plugins/run.sh 通過驗證 [after: 1.3]

## 3. 文件與發行說明

- [x] 3.1 docs/deploy/plugins.md 新增 smtp 一節（選項、交談步驟、所有原因、速率限制與 log 的提醒、只以 Postfix 驗證），docs/manual/04-health-checks.md 與 docs/manual.zh_TW/04-health-checks.md 的 plugin 表加入 smtp，examples/emfor.toml 加一個 smtp 服務，CHANGELOG.md 未發行段落記錄；以 `emfor check-config` 檢查範例設定、macOS 與 Debian 映像的 fmt、clippy、test 全部通過驗證 [after: 2.1]
