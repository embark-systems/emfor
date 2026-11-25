## Problem

三個小地方的訊息或輸出和實際情況不符，都是 2026-11-25 對 v0.7.1 之後全部變更做 Opus review 時找到的（下面「已用程式碼確認」的三項，我已讀過程式碼；第四項是 review 的推論，我也讀過相關程式碼）：

1. **時鐘量測過期時，log 說「時鐘回到範圍內」。** 成員的時鐘偏差超過 2 秒會進入「clock offset」狀態並記一行 `warn`。若這個成員之後不再送訊息（當機、網路斷了），10 秒後量測被丟掉，daemon 記一行 `info`：「clock of peer X is within limits again」。事實是沒有資料，不知道時鐘有沒有回來。已確認：`peers/group.rs` 的 `update_clock` 對「量測過期」和「偏差回到 1500 ms 以內」用同一句訊息。
2. **`emfor status` 在本機其實在中間時，仍叫人檢查本機時鐘。** 有兩個成員，一個領先 3000 ms、一個落後 3000 ms 時，status 印出「clock of this host differs from every member …; check this host's clock」。本機在兩者之間，該查的是那兩台。已確認：`status.rs` 的 `peer_lines` 只比較「有偏差的成員數是否等於成員數」，不看方向。
3. **JSON 模式下，啟動時的 `config file read`（含 `sha256`）仍是純文字。** 重載時同樣的行是 JSON。已確認：`cli.rs` 的 `start` 在 `log_files_read` 之後才呼叫 `format_handle.reload`。用 Vector 或 jq 處理 log 的人要為這幾行另外處理。
4. **時鐘量測的重放防護有缺口（只是記錄限制，不改程式碼）。** `Group::check` 在檢查訊息過期之前就量測時鐘，為的是看得到超過 5 秒的偏差；量測前只用「不是重放」把關，而這個把關依賴「此成員之前被接受過的訊息」。成員自 daemon 啟動後沒有被接受過訊息，或它的時鐘已偏差超過 5 秒（每則訊息都被當 `stale` 丟掉，`last` 不前進）時，有人偽造來源位址、重放一則舊封包，就能讓這個成員被標成時鐘偏差。只影響監控（`emfor status` 的行、一行 `warn`、`emfor_peer_clock_offset_seconds`），不影響投票。程式註解說「抓到封包的人不該能讓成員的時鐘看起來不對」，在這兩種情況下不成立；文件（threat model、peer-group spec）也沒寫這個限制。

## Root Cause

1. `update_clock` 對兩種離開狀態的原因共用一句訊息；「量測過期」沒有被當成獨立的情況。
2. `peer_lines` 的「每個成員都有偏差」條件沒有包含方向，把「本機時鐘錯」和「成員之間互相不同」混在一起。
3. `start` 的呼叫順序：`log_files_read` 排在格式切換之前。
4. 重放防護依賴 `Last`（已接受訊息的狀態），在它還不存在或不前進時沒有任何把關；要在丟棄前量測是設計用意（為了讓人看到超過 5 秒的偏差），所以不能只改成「只量被接受的訊息」。

## Proposed Solution

1. `update_clock`：量測過期時記另一句 `info`（例如「no clock offset measurement of peer X for 10 s; the state is cleared」，措辭定在 design）；偏差真的回到 1500 ms 以內時維持原句。兩者都仍是離開狀態時記一行 `info`，peer-group spec 的「離開狀態記 info」要求不變。
2. `peer_lines`：只有所有成員的偏差同號（全部領先或全部落後本機）時才印「clock of this host differs from every member」那一行；否則照成員逐行印。成員數至少 2 的條件不變。
3. `start`：把 `log_files_read` 移到格式切換之後，讓 JSON 模式下這幾行是 JSON。讀檔警告（`log_read_warnings`）仍在設定載入前寫，維持純文字（那時格式還不知道）。手冊 12 章（en、zh_TW）與 CHANGELOG 目前依現狀寫成「這幾行是文字」，要改。
4. 不改程式碼：在 `docs/threat-model.md` 與 peer-group spec 的「Clock offset of members」寫明這個限制（偽造來源位址加舊封包重放，可以讓沒有被接受過訊息、或時鐘已偏差超過 5 秒的成員被標成時鐘偏差；只影響監控），並把程式註解改成不誇大。

## Non-Goals (optional)

- 不改成「只量被接受的訊息」：那會讓超過 5 秒的偏差（正是最需要看到的）完全看不到。
- 不處理 `meta/peers` 每秒重寫（有成員被標成偏差時，`offset_ms` 每秒微變；`silent_checkers` 清單沒有上限，值可能到數百 KB）。這是低衝擊的設計取捨，記入 TODO，不在此變更。
- 不改穩定欄位清單，不改 `emfor_peer_clock_offset_seconds` 的意義。
- 不改心跳、store 格式、客戶端的過期判斷。

## Success Criteria

- 單元測試：量測過期時 `update_clock` 記的 `info` 訊息不含「within limits again」，偏差真的回到範圍內時仍含；兩種情況都只記一行（`peers/group.rs` 的測試，擴充「The measurement expires」與「Hysteresis」）。
- 單元測試：兩個成員 +3000 ms 與 −3000 ms 時，`emfor status` 逐成員印兩行，不印「differs from every member」；兩個成員都 −4100、−3900 時仍印那一行（`status.rs` 的測試，擴充「Every member differs」）。
- 整合測試：`[log] format = "json"` 時，啟動後第一批 `config file read` 行是帶 `file` 與 `sha256` 欄位的 JSON；讀檔警告仍是文字（`tests/audit_log.rs` 或 `tests/log_format.rs`）。
- 文件：手冊 12 章（en、zh_TW）、CHANGELOG、threat-model 的對應句子與程式一致；`python3 tools/check-manual-links.py` 回 0 個問題。
- 完整測試：`tools/docker-test.sh`（不帶參數）全部通過。

## Impact

- Affected specs: peer-group、status-command、service-config
- Affected code:
  - Modified: crates/emfor-daemon/src/peers/group.rs, crates/emfor-daemon/src/status.rs, crates/emfor-daemon/src/cli.rs, crates/emfor-daemon/tests/log_format.rs, crates/emfor-daemon/tests/audit_log.rs, docs/manual/12-operating.md, docs/manual.zh_TW/12-operating.md, docs/threat-model.md, CHANGELOG.md
  - New: none
  - Removed: none
