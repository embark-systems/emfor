## 1. 前置

- [x] 1.1 確認 `emfor-secret-read-timeout` 已 archive 並 commit（本 change 的 delta 以它的 "Peer secret reload" 為底，且兩者改同一個檔案 `crates/emfor-daemon/src/peers/task.rs`）；之後把本 change 的 `specs/peer-group/spec.md` 與主 spec 的該需求逐字比對，只有「記錄檔」「啟動時採用 previous 的條件」與新增四個情境不同。完成時：比對結果寫在回報裡，不一致就先 ingest 修正 delta。

## 2. 記錄檔

- [x] 2.1 [after: 1.1] 實作 spec peer-group "Peer secret reload" 的記錄（設計決定「記錄的內容與規則」「檔案位置與格式」）：在 `crates/emfor-daemon/src/peers/` 新增小模組（讀、寫、指紋；`key=value` 文字、SHA-256 用已依賴的 `sha2`、暫存檔加 rename、模式 0600）。完成時：單元測試涵蓋不存在、讀不懂、寫入後讀回、模式 0600，且記錄檔的內容中找不到密鑰本身。
- [x] 2.2 [after: 2.1] `peers::start`／`start_keys`（`task.rs`）與 `cli.rs` 的呼叫處：傳入 store 路徑，依設計決定的規則決定是否採用 `previous_secret_file`（沒有記錄就忽略並記一行 `info`；設了卻無效時記一行 `info` 說明原因），並寫入啟動時的記錄；`Keys::resumed`（`crypto.rs`）的條件隨之調整。完成時：既有的 `previous_secret_file_survives_a_restart` 依新規則更新後通過。
- [x] 2.3 [after: 2.2] （設計決定「寫入失敗與讀取失敗」）`apply_secret` 在 `Keys::rotate` 回傳 true 時，以獨立執行緒寫新記錄（`replaced` 為舊的現行指紋、`changed_at` 為現在）；寫入失敗用 `WARN_EVERY` 限制每分鐘最多一次 `warn`。完成時：測試中換密鑰後記錄檔的 `current` 與 `replaced` 正確，記錄目錄不可寫時 daemon 仍照常執行且只警告一次。

## 3. 測試

- [x] 3.1 [after: 2.3] 依 spec 新增的四個情境加測試（停機換檔後啟動、previous 被更早的輪換退役、`secret_file` 被 touch、升級後第一次啟動），加上「執行中換檔後重啟」。完成時：每個情境有一個測試，其中「previous 被更早的輪換退役」與「touch」兩個在舊程式碼上失敗（用 git stash 或暫時還原判斷式確認並寫進回報）。Debian 容器跑 `tools/docker-test.sh` 的完整 gate（fmt、clippy -D warnings、測試）通過。

## 4. 文件

- [x] 4.1 [after: 2.3] `docs/manual/03-configuration.md` 與 `docs/manual.zh_TW/03-configuration.md`：改寫「previous secret 只在記憶體」那段（記錄檔位置 `<store.path>-peers`、只有被換掉的那一把才會被採用、升級後第一次啟動不採用、不再說「可以一直留在設定裡」而說明留著也無妨但不會被誤用），`docs/manual/08-operations.md` 的升級段（與 zh_TW）提到升級後第一次啟動；`CHANGELOG.md` Unreleased 加條目（Checked／Not verified，真實多主機輪換未試）。完成時：`python3 tools/check-manual-links.py` 報 0 個問題。
