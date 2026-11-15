## 1. 輸出函式與旗標

- [x] 1.1 設計「輸出格式」與「沒有預設值」：`config.rs` 新增把 `&Layers` 轉成帶註解 TOML 字串的函式（純量與陣列先、子表後，標頭用 `key_path`，值用 `toml::Value` 的 `Display`，設了 `include_dir` 時行尾加來源檔案、沒設時開頭一行 `# merged from`）；驗證目標：單元測試涵蓋 Round trip（去掉註解後解析等於 `Layers::table`，含鍵含 `.`、內嵌表、陣列、字串含 `#`）、Valid config with fragments（後面的檔案與被取代的清單的來源）、Without include_dir、Defaults are not shown。
- [x] 1.2 [after: 1.1] 設計「旗標與輸出的分工」，即需求「Showing the merged config」：`Command::CheckConfig` 新增 `--show`，有效時標準輸出只有那份 TOML、`config ok` 與 `read` 行寫到標準錯誤，無效時標準輸出為空且結束碼 2，不加旗標的輸出逐位元組不變；驗證目標：整合測試 Round trip（`--show` 的輸出存檔後 `check-config --config` 通過）、Invalid config、Without the flag（現有 `check_config_*` 整合測試不修改仍通過）。

## 2. 文件與驗證

- [x] 2.1 [after: 1.2] 手冊第 3 章（檢查設定檔與把設定拆到目錄）與第 8 章指令表（中英）、`docs/deploy/daemon.md`、CHANGELOG 的 Unreleased：說明 `--show`、輸出分工、來源註解是盡力而為、不套用預設值、不遮蔽（`command` 裡不要放祕密）、格式不承諾穩定；驗證目標：`tools/check-manual-links.py` 通過，手冊範例的輸出用實際指令核對過。
- [x] 2.2 [after: 1.2, 2.1] Debian docker 閘門：`tools/docker-test.sh` 的 fmt、clippy `-D warnings`、`cargo test --workspace --all-features` 全過；macOS 與 FreeBSD 等發版前再測，標為未驗證。
