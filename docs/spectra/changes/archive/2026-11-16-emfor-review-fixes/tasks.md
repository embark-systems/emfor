## 1. 已實作的修正（事後記錄）

- [x] 1.1 需求「Config directory permissions」，設計「擁有者與模式一起判斷，對已開啟的檔案做」：片段與目錄的擁有者規則、符號連結看目標、對已開啟的檔案檢查與讀取；驗證目標：單元測試 `owner_must_be_root_or_the_daemon_user`、`symlink_to_a_group_writable_file_is_refused`、`foreign_owner_is_refused`（root 才實際執行）。
- [x] 1.2 需求「Cancelled requests do not count as unanswered」，設計「請求只有等滿它的等待時間才算『不回答』」：`PluginHost::call_waiting`，`PluginChecker::check` 與 `validate` 使用它；驗證目標：整合測試 `aborted_requests_do_not_restart_the_plugin`，既有的 `plugin_that_stops_answering_is_restarted` 仍通過。
- [x] 1.3 需求「Bounded validation answer」，設計「`validate` 回答以位元組為準」：12 筆、512 字元、48 KiB 的上限；驗證目標：`validate_answer_is_bounded`、`validate_answer_is_bounded_in_bytes`。
- [x] 1.4 需求「Check-config output errors」與「Lookup output errors」，設計「輸出寫入失敗回報而不是 panic」：`write_stdout`，`check-config` 失敗為 1、`lookup` 為 3；驗證目標：Linux 整合測試 `check_config_reports_a_full_disk`。
- [x] 1.5 需求「Peer secret file must not be writable」與「Peer failure reason in the log」，設計「小修正」：`secret_file` 不可被 group 或其他人寫入，peer 失敗原因寫日誌前截斷與跳脫，`boot` 只收小寫十六進位；驗證目標：`writable_peer_secret_is_refused`、`boot_is_lowercase_hex`。
- [x] 1.6 設計「小修正」：`check-config --show` 註解跳脫控制字元、`meta/peers` 寫入去重、DNS 速率限制器每秒最多掃描一次；驗證目標：`show_escapes_control_characters_in_comments`、`full_table_is_swept_once_a_second`，writer 既有測試仍通過。
- [x] 1.7 需求「Naming the store is not public API」：`Client::for_store` 只在 `daemon-internal` feature 下存在，`Client::new` 為 crate 內部；驗證目標：`cargo clippy -p emfor-client --all-targets`（預設與 `--features capi`）與整個 workspace 都通過。
- [x] 1.8 文件：手冊第 3、6 章（中英）、`docs/deploy/daemon.md`、`CHANGELOG.md`；驗證目標：`tools/check-manual-links.py` 通過。
- [x] 1.9 [after: 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8] Debian docker 閘門：`tools/docker-test.sh` 的 fmt、clippy `-D warnings`、`cargo test --workspace --all-features` 全過；macOS 與 FreeBSD 等發版前再測，標為未驗證。
