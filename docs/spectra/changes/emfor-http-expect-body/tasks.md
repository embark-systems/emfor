## 1. HTTP checker

- [x] 1.1 HTTP checker 選項：`expect_body` 為 1–16384 位元組的字串（錯誤訊息 `must not be empty`、`must be at most 16384 bytes`、`must be a string`），依「expect_body 沿用 tcp expect 的比對方式」加入 crates/emfor-daemon/src/checker/http.rs 的 `HttpOptions` 與允許的 key；以 http.rs 的 `validation_errors` 測試涵蓋 spec 範例表 expect_body validation 每一列
- [x] 1.2 HTTP checker 比對內容：依「狀態碼先判斷」與「內容最多讀 65536 位元組，找到即停止」，狀態碼符合後逐段讀內容、累積比對，找不到為 `expect_body: got "<前 64 位元組>"`，讀取失敗為 `body: ` 開頭；以 http.rs 單元測試（本機假伺服器）涵蓋 spec 範例表 body matching 每一列，並確認拿掉比對時新測試失敗 [after: 1.1]

## 2. 文件與發行說明

- [x] 2.1 docs/manual/04-health-checks.md 與 docs/manual.zh_TW/04-health-checks.md 的 http 選項表加入 `expect_body`（上限、只在狀態碼符合時讀、延遲包含讀內容的時間），附一個健康 JSON 的範例；examples/emfor.toml 加入一個用法；CHANGELOG.md 未發行段落記錄；以 `emfor check-config` 檢查範例設定、macOS 與 Debian 映像的 fmt、clippy、test 全部通過驗證 [after: 1.2]
