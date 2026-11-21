## 1. 測試與 CI

- [x] 1.1 實作設計決定「flaky 測試：輪詢等失敗，不改 SecretWatch」：`crates/emfor-daemon/src/peers/watch.rs` 的 `tests` 模組新增 `failed` 輔助函式，`removed_directory_is_a_failure` 改用它。完成時：該測試在 Debian 容器單獨跑通過，完整 gate 連跑 3 次不再失敗（若仍失敗，回報輸出而不是調整期限）；`peers/watch.rs` 的非測試程式碼沒有變動。
- [x] 1.2 實作設計決定「fuzz.yml：env 變數加數字檢查」：`.github/workflows/fuzz.yml` 的 `Fuzz` 步驟。完成時：YAML 可被解析，`run:` 區塊通過 `sh -n`，`seconds` 空值、字母、`1;id` 這三種輸入以 1 結束（以同一段 `case` 在本機 shell 驗證），`300` 通過；檔案裡不再有 `github.event.inputs` 出現在 `run:` 裡。

## 2. 範例腳本

- [x] 2.1 實作設計決定「pf 腳本：在 awk 裡驗證，無效的略過並說一行」：`examples/pf/emfor-pf-table.sh`，並新增 `tests/pf/validate.sh`（以假 `drill` 與假 `pfctl` 放在 `PATH` 前面，驗證設計「行為（pf）」列出的四種輸入、`EMPTY` 與表名的檢查、pfctl 失敗訊息）。完成時：`tests/pf/validate.sh` 在 Docker 的 Debian 容器通過；腳本對一個全是合法地址的回答，輸出與修改前相同（以同一個假 `drill` 比對修改前後的 `pfctl` 參數）；`docs/manual/07-dns-recipes.md` 與 zh_TW 版先讀確認「Using it with pf」沒有描述被改的行為，需要才改。
- [x] 2.2 實作設計決定「Nagios 腳本：在 jq 裡做兩個小函式」：`examples/nagios/check_emfor.sh`，並在 `tests/nagios/inside.sh` 加一個情境：`fake_check` 回 `CRITICAL - a|b` 的後端，檢查輸出恰有一個 `|` 且文字含 `a/b`（先讀 `inside.sh` 既有的斷言與 `fake_check` 的狀態檔寫法再加，不發明新框架）。完成時：`tests/nagios/run.sh` 在 Docker 通過；既有情境的輸出不變；`docs/manual/08-operations.md` 與 zh_TW 版「Watching emfor from Nagios」先讀確認，需要才改。

## 3. 驗證

- [x] 3.1 [after: 1.1, 1.2, 2.1, 2.2] `CHANGELOG.md` Unreleased 加條目（四項，Checked 與 Not verified：未在 FreeBSD 實機與真實 Nagios 驗證、`fuzz.yml` 未在 Actions 上執行、新測試對舊程式的結果如實寫）；`python3 tools/check-manual-links.py`。完成時：`tools/docker-test.sh` 全過，`spectra analyze` 與 `spectra validate` 乾淨。
