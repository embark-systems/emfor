## 1. 主檔的權限與擁有者檢查

- [x] 1.1 實作 requirement「Config directory permissions」對主檔的新規則，依 design 的決定「拒絕，不是警告」與「錯誤訊息附修正指令」：`read_layers_with` 讀主檔改用 `read_trusted`，`untrusted` 的原因文字對 mode 附 `chmod g-w,o-w <path>`、對擁有者附 `chown root <path>`；`load_store` 與 `read_table` 不動。完成時：`emfor check-config` 對 mode 0664 的主檔以結束碼 2 拒絕並印出路徑、mode 與修正指令；片段檔的現有拒絕訊息只多了修正指令。驗證：新的單元測試（見 1.2）與 `cargo test -p emfor-daemon config::` 在 Debian 容器通過（`tools/docker-test.sh`）。
- [x] 1.2 [after: 1.1] 把 `config.rs` 的測試 `main_file_mode_is_not_checked` 改成新規則的測試：mode 0600、0640、0644、0604 被接受，0620、0602、0664、0666 被拒絕且訊息含路徑與 mode；擁有者為 uid 12345 的主檔被拒絕（只在 root 時執行，做法同 `foreign_owner_is_refused`）；指向 0664 檔案的符號連結被拒絕；不存在的主檔得到 `cannot read <path>`（沒有現有測試鎖定它）。驗證：這些測試在 Debian 容器通過，而且改動前的程式碼會讓其中的拒絕案例失敗。

## 2. daemon 的行為

- [x] 2.1 [after: 1.1] 在 `crates/emfor-daemon/tests/daemon.rs` 加整合測試：用 0664 的主檔啟動 daemon，結束碼為 2、log 有主檔的路徑與 mode、store 檔案沒有被建立；`emfor check-config` 對同一個主檔結束碼為 2 並在 stderr 有訊息。驗證：測試在 Debian 容器通過。
- [x] 2.2 [after: 1.1] 驗證 design 的「錯誤歸屬與重載」（主檔被拒絕是整個載入的錯誤，重載時保留舊設定，不需要新機制），在同一個檔案加整合測試：daemon 以 0644 的主檔執行，把主檔改成 0664 並送 SIGHUP，log 出現 `reload rejected`，群組與服務照舊（`emfor status` 仍顯示原來的服務）；改回 0644 再送 SIGHUP 後重載成功。驗證：測試在 Debian 容器通過。
- [x] 2.3 [after: 1.1] 在同一個檔案加整合測試：主檔為 0664 時，`emfor status` 與 `emfor lookup` 仍然成功讀到 `[store]`（它們不檢查主檔）。驗證：測試在 Debian 容器通過。
- [x] 2.4 [after: 1.2, 2.1, 2.2, 2.3] 跑整個 workspace 的測試與 clippy：既有測試或輔助函式建立的主檔如果 mode 較寬或擁有者特殊，會在這裡失敗，逐一修正（改測試的建立方式，不放寬檢查）。驗證：`tools/docker-test.sh`（fmt、clippy `-D warnings`、全部測試）在 Debian 容器通過，並記下通過的測試數。

## 3. 文件與規格

- [x] 3.1 更新手冊中說「主檔不檢查」的段落：`docs/manual/03-configuration.md` 的 "The main file is not checked this way, so protect it yourself" 改成主檔同樣被檢查（含以 root 手動執行 `check-config` 時只接受 root 擁有的檔案），`docs/manual.zh_TW/03-configuration.md` 對應段落同步；先 grep 兩個手冊目錄與 `docs/deploy/daemon.md`、`SECURITY.md`、`packages/` 的文件有沒有其他描述主檔權限的地方，有就一起改。驗證：`python3 tools/check-manual-links.py` 報 0 個問題，並檢視 grep 的結果沒有殘留「主檔不檢查」的說法。
- [x] 3.2 [after: 3.1] 在 `docs/manual/10-migrating.md` 或第 8 章的升級說明（中英文）寫 0.7.0 的相容性變更：主檔的 mode 或擁有者不合格時 daemon 起不來，修正指令是 `chmod g-w,o-w /etc/emfor/emfor.toml` 與 `chown root /etc/emfor/emfor.toml`，套件安裝的主檔（root 0644）不受影響。先看升級說明現在放在哪一章再決定位置。驗證：內容檢視，連結檢查報 0 個問題。
- [x] 3.3 [after: 2.4] 在 `CHANGELOG.md` 的 Unreleased 最前面加一條 "The main config file is checked like the fragments (compatibility change)"：規則、原因（主檔能宣告 plugin command）、升級前要做什麼、`status` 與 `lookup` 不受影響，以及 Checked 與 Not verified（沒在 macOS、FreeBSD 手動跑，沒試真實的 deb、rpm 升級）。驗證：內容檢視，數字與測試數和 2.4 的結果一致。
- [x] 3.4 [after: 3.3] 以 `spectra validate emfor-main-file-trust` 驗證 change；`spectra analyze emfor-main-file-trust` 沒有 Critical 或 Warning。驗證：兩個指令的輸出。
