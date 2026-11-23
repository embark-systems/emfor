## 1. 輪換稽核行

- [x] 1.1 依 design「在 `apply_secret` 記，由呼叫端傳入讀的路徑」「欄位與格式」「不經 reload，也不經 `audit.rs`」，在 `crates/emfor-daemon/src/peers/task.rs` 讓 `apply_secret` 多收一個讀取來源的 `path: &Path`，`SecretReader::refresh` 的兩個呼叫點傳讀取當時的路徑，並在 `group.keys.rotate` 回 `true` 時，把原本那行 `info` 改成帶結構化欄位 `change = %"peers_secret_rotated"` 與 `secret_file = %path.display()`，訊息文字不變；該檔既有測試對 `apply_secret` 的呼叫跟著補上路徑參數，行為斷言不變。驗證（對應 design「不含密鑰的證明方式」）：peer-group「Peer secret rotation log line」的四個 scenario 各有一個 `tracing_test::traced_test` 單元測試，標註 `// spectra-scenario: peer-group / <scenario>`：換成新密鑰時有 `change=peers_secret_rotated` 與路徑、且整份 log 不含舊或新密鑰的原文與十六進位；相同內容不記；無效內容只有既有的 `warn`、沒有這行；reload 改路徑後記新路徑。`tools/docker-test.sh 'cargo test -p emfor-daemon peers::task'` 通過。
- [x] 1.2 在 `crates/emfor-daemon/src/audit/tests.rs` 補一個測試：兩份設定只有 `[peers] secret_file` 路徑不同時，`audit::changes` 與 `log_changes` 產生一行 `change=peers_changed`、`field=secret_file`、`from`、`to` 是兩個路徑、`applies=now`，且 log 不含任何密鑰內容或 `sha256`（現有輔助函式一直用同一個路徑，沒覆蓋這種差異）。驗證：`tools/docker-test.sh 'cargo test -p emfor-daemon audit'` 通過。

## 2. 文件

- [x] 2.1 [after: 1.1] 手冊 `docs/manual/11-peer-groups.md`「Rotating the secret」與 `docs/manual/12-operating.md`「What a reload changed」及其穩定欄位表，加入 `change=peers_secret_rotated`（欄位 `secret_file`）的說明：輪換不經 reload，這行在輪換套用時記一次，不含密鑰內容或其雜湊，不計入 reload 的 50 行；`docs/manual.zh_TW/` 對應章節照 `docs/spectra/LANGUAGE.md` 與自然用語改寫；`CHANGELOG.md` 的 Unreleased 加一條（中英章節並列，寫明只新增欄位與 `change` 值）。驗證：內容審閱對照 design 的欄位與 spec 的行文；`python3 tools/check-manual-links.py` 通過。

## 3. 收尾

- [x] 3.1 [after: 1.1, 1.2, 2.1] 完整檢查：`tools/docker-test.sh`（不帶參數，含 fmt、clippy、全部測試）通過，既有的 peers、audit 測試沒有因新欄位而改動預期。驗證：docker-test.sh 回報全部通過。
