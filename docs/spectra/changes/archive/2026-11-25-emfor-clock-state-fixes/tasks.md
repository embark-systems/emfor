## 1. 時鐘量測過期的訊息

要求：peer-group 的 Clock offset of members。

- [x] 1.1 要求 Clock offset of members：`peers/group.rs` 的 `update_clock`：成員因量測過期（10 秒沒有訊息）離開「clock offset」狀態時，記一行 `info`，內容說 10 秒沒有量到偏差，不含「within limits」；因偏差回到 1500 ms 以內離開時維持「clock of peer X is within limits again」。兩種情況都只記一行。驗證：擴充 `the_measurement_expires` 與 Hysteresis 的測試，斷言兩種 `info` 訊息的內容與行數（標 `// spectra-scenario: peer-group / The measurement expires` 與 `Hysteresis`）。
- [x] 1.2 [after: 1.1] 新增測試：沒有被接受過任何訊息的成員 C，收到三則帶 C 來源位址、4 小時前的錄製封包時，每則都以 `stale` 丟棄、C 進入「clock offset」狀態（偏差約 -14400000）、投票與存活不變（標 `// spectra-scenario: peer-group / A replay for a member without an accepted message`）。這是在記錄現況，不改行為。驗證：該測試通過。
- [x] 1.3 `Group::check` 中「A replay is not measured」旁的註解改成不誇大：說明防護需要先前被接受過的訊息，沒有時不成立，影響只限監控（狀態、log、`emfor status`、`emfor_peer_clock_offset_seconds`）。`docs/threat-model.md`「A network attacker on the peer UDP port」一節和「Known gaps」各加一句相同限制。驗證：內容審閱（註解與兩處文字）；`python3 tools/check-manual-links.py` 回 0 個問題。

## 2. status 的「本機時鐘」警告看方向

要求：status-command 的 Clock offset line in status text。

- [x] 2.1 要求 Clock offset line in status text：`status.rs` 的 `peer_lines`：只有 `clock_offsets` 至少兩筆、筆數等於 `members`、且所有 `offset_ms` 同號時，才印「clock of this host differs from every member …」；其他情況逐成員印行。驗證：新增「Members on both sides of this host」測試（偏差 3000 與 -3000 印兩行成員行、不印本機行），既有「Every member differs」（-4100 與 -3900）仍印本機行（標 `// spectra-scenario: status-command / Members on both sides of this host`）。

## 3. 啟動時的 `config file read` 行在 JSON 模式下是 JSON

要求：service-config 的 Log format。

- [x] 3.1 要求 Log format：`cli.rs` 的 `start`：把 `log_files_read` 移到格式切換（`format_handle.reload`）之後，`log_read_warnings` 與設定錯誤的順序不變。`[log] format = "json"` 時啟動後的 `config file read` 行是帶 `file` 與 `sha256` 欄位的 JSON；`format = "text"` 時行為不變。驗證：`tests/log_format.rs` 新增測試啟動 JSON 模式，斷言該行是 JSON 物件並含 `file`、`sha256`（標 `// spectra-scenario: service-config / Config file read lines at start`）；`tests/audit_log.rs` 的「Daemon logs the hash of every file read」仍通過。
- [x] 3.2 [after: 3.1] 手冊 12 章（`docs/manual/12-operating.md`、`docs/manual.zh_TW/12-operating.md`）「JSON lines」兩段與 `CHANGELOG.md` Unreleased 中講「啟動時這些行是文字」的句子，改成與程式一致：設定套用前的行（啟動 plugin、設定錯誤、讀檔警告）是文字，啟動後的 `config file read` 行是 JSON。驗證：內容審閱（三處文字與 3.1 的測試一致）；`python3 tools/check-manual-links.py` 回 0 個問題。

## 4. 完整檢查

- [x] 4.1 [after: 1.1, 1.2, 2.1, 3.1] 跑完整測試與格式檢查；驗證：`tools/docker-test.sh`（不帶參數）全部通過，`cargo fmt --check` 與 `cargo clippy --locked --all-targets` 無新增警告。
- [x] 4.2 [after: 1.3, 3.2] CHANGELOG.md Unreleased 加一條：時鐘量測過期的 log 訊息、status 的本機警告看方向、JSON 模式下啟動的 `config file read` 行是 JSON、重放量測的限制寫進 threat model；不列為 Breaking（log 行不在穩定欄位內）。驗證：內容審閱（無數字或欄位與程式不符）。
