## 1. 合併與讀取（先新增，不動現有路徑）

- [x] 1.1 純函式合併兩份 `toml::Table`，實作「Fragment merge rules」：表加表逐鍵遞迴、純量／清單／型別不同整個取代、行內表與 `[x.y]` 同樣處理；驗證目標：`config.rs` 新增的單元測試涵蓋 List is replaced、Table merges key by key、型別不同取代、行內表對 `[x.y]`。
- [x] 1.2 [after: 1.1] 讀取函式：讀主設定檔與 `include_dir`，依設計「哪些檔案、什麼順序」與「Config directory with ordered fragments」選檔（`.toml` 結尾、非 `.` 開頭、一般檔案或指向一般檔案的符號連結、位元組字典序、非 UTF-8 名稱略過並警告、空目錄合法、相對路徑相對於主檔目錄），回傳合併後的 table、鍵路徑對檔案的對照表、讀過的檔案清單與警告；驗證目標：用暫存目錄的測試涵蓋 Files are read in filename order（`01`、`10`、`9`）、Unselected entries are ignored、Symbolic link、Relative include_dir、Non-UTF-8 file name。
- [x] 1.3 [after: 1.2] 設計「只准在主設定檔的鍵」「權限：直接拒絕」，即「Keys allowed only in the main file」與「Config directory permissions」與「Missing config directory」：片段含 `[store]` 或 `include_dir` 為錯誤並指出檔名；目錄或被讀檔案 `mode & 0o022` 非零就拒絕並指出路徑與模式（符號連結取指向的檔案）；目錄不存在或不是目錄為錯誤；主設定檔不加新檢查；驗證目標：對應 Scenario 的測試，包含 Main file mode unchanged。

## 2. 錯誤歸屬

- [x] 2.1 [after: 1.2] 「Errors name their file」的語法錯誤部分：每個檔案單獨解析，TOML 語法錯誤顯示 `檔名:行:欄`（`toml::de::Error::span()` 換算）；驗證目標：測試 Syntax error in a fragment，行欄正確。
- [x] 2.2 [after: 1.2] 「Errors name their file」的驗證錯誤部分：`ConfigError` 新增選用的檔案欄位並在 `Display` 顯示；`Validator::err` 依「錯誤歸屬，不用位置資訊」決定向上找最近有紀錄的祖先鍵；驗證目標：測試 Invalid value in a fragment（檔名與 `services."api.svc.emfor".interval`），並確認沒有檔案資訊時（單一檔案）錯誤的輸出與現在逐字相同。

## 3. 遷移載入路徑（先新增吃 table 的版本，再搬呼叫端）

- [x] 3.1 [after: 1.3] 為 `parse`、`parse_plugins`、`parse_store`、`parse_dns_listen`、`parse_metrics_listen`、`parse_peers_listen` 新增吃 `&toml::Table` 的版本，文字版保留並轉呼叫 table 版；驗證目標：現有 32 個設定測試（`parse_ok`／`parse_err`）不改仍通過。
- [x] 3.2 [after: 3.1, 2.2] 把 `cli.rs` 的 `load` 改成呼叫一次新的讀取函式並把合併後的 table 交給上述六個輔助函式（「一個函式讀全部檔案，合併成一份 table，所有解析都吃它」）；驗證目標：測試 Plugin declared in a fragment、Fragment changes a listen address（reload 回報需要重啟）、One bad fragment rejects a reload。
- [x] 3.3 [after: 3.2] 啟動與 reload 的失敗處理：啟動時 `include_dir` 錯誤記錄並以結束碼 2 結束且不開 store；reload 時（含目錄消失）拒絕並保留舊設定、daemon 不結束；`load_store` 只讀主設定檔；驗證目標：測試 Missing directory at start、Directory removed before a reload、status and lookup need only the main file。
- [x] 3.4 [after: 3.2] 設計「check-config 與日誌」，即「Listing the files read」：`emfor check-config` 在設定了 `include_dir` 時於 `config ok:` 行之後印每個檔案的 `read <path>`，沒設定時輸出與現在相同；daemon 在啟動與每次成功 reload 以 `info` 記錄同一份清單；驗證目標：測試 check-config with fragments 與 check-config without include_dir（輸出逐字相同）。
- [x] 3.5 [after: 3.2] 設計「不受影響的東西」，聯防群組 digest：測試同一個服務在單一檔案與拆成兩個檔案時 `check_digest` 的結果相同（Peer digest does not depend on file splitting）。
- [x] 3.6 [after: 3.3, 3.4, 3.5] 收尾：移除不再被呼叫的文字專用路徑（保留 `parse(text, &registry)` 供測試），確認 `emfor-daemon` 沒有殘留重新解析主檔文字的地方；驗證目標：`cargo clippy --workspace --all-targets --all-features -- -D warnings` 沒有 dead code 警告，`rg` 找不到殘留的呼叫。

## 4. 文件與套件

- [x] 4.1 [after: 3.4] 手冊第 2、3、8、10 章（英文與繁中）：安裝時建立 `conf.d`、`include_dir` 與合併規則表、主檔專屬鍵與權限不對稱、原子寫入建議、`check-config` 輸出、第 10 章拆檔範例；驗證目標：`tools/check-manual-links.py` 通過，範例 TOML 用 `emfor check-config` 驗證過。
- [x] 4.2 [after: 3.4] `docs/deploy/daemon.md`、`examples/emfor.toml`（註解掉的 `include_dir`）、`CHANGELOG.md`（Unreleased）；驗證目標：範例設定用 `emfor check-config` 通過、內容審閱。
- [x] 4.3 [after: 3.4] FreeBSD port（`examples/freebsd/Makefile` 的 PLIST 加 `@dir etc/emfor/conf.d` 並在安裝步驟建立目錄、README 說明）與其他套件的建立目錄說明；驗證目標：內容審閱；port 的 `make stage check-plist` 發版前在 FreeBSD VM 驗證，此處標為未驗證。

## 5. 驗證

- [x] 5.1 [after: 3.6, 4.1, 4.2, 4.3] Debian docker 閘門：`tools/docker-test.sh` 跑 `cargo fmt --check`、clippy `-D warnings`、`cargo test --workspace --all-features` 全過；macOS 與 FreeBSD 不在此次範圍，發版前再測。

## 6. 審查後補的行為（ingest，已實作）

- [x] 6.1 空 `include_dir` 拒絕，與目錄內檔案 metadata 錯誤（非不存在）為載入錯誤；驗證目標：單元測試 `empty_include_dir_is_an_error`、`unreadable_fragment_link_is_an_error`（含懸空連結被略過）。
- [x] 6.2 plugin 啟動失敗與 reload 的需要重啟錯誤帶檔名；驗證目標：整合測試 `listen_change_in_a_fragment_requires_restart` 斷言輸出含 `20-dns.toml`。
- [x] 6.3 略過檔名的警告：`check-config` 印到 stderr，daemon 在啟動與 reload 不論成敗都記錄；驗證目標：Linux 整合測試 `check_config_warns_about_a_non_utf8_file_name`；Debian `tools/docker-test.sh` 全過。
