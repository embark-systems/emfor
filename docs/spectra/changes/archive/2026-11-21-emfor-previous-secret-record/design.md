## Context

`start_keys` 以 `secret_file` 的修改時間（mtime）決定是否採用 `previous_secret_file`（見 proposal 的 Why）。審查提出的修法是讓 daemon 記下自己的輪換。手冊的輪換流程是 `cp -p peers.key peers.key.old` 再寫入新內容，所以 previous 檔自己的 mtime 是舊的（`cp -p` 保留），不能用「previous 檔也要是最近改的」當條件。

## Goals / Non-Goals

**Goals:**

- 只有 daemon 自己換掉的那一把密鑰，才能在重啟後當 previous 使用。
- 停機期間換了 `secret_file` 再啟動（手冊的標準流程）仍然可用。
- 記錄裡不放密鑰本身。

**Non-Goals:**

- 見 proposal 的 Non-Goals。

## Decisions

以下是使用者決定的（採 C、升級後第一次啟動不採用 previous）與我做的設計選擇，使用者可以改。

### 記錄的內容與規則

記錄是一個小文字檔，欄位：`current`（現行密鑰的 SHA-256 指紋，hex）、`replaced`（被換掉的密鑰的指紋，可能沒有）、`changed_at`（那次變更的 Unix 秒，`replaced` 存在時才有）。

- 啟動：讀記錄。`current` 與現行密鑰指紋相同 → 記錄不動（daemon 上次就是用它，變更是 daemon 自己看到的）。不同 → 新記錄：`current` 為現行指紋，`replaced` 為舊記錄的 `current`，`changed_at` 為 `secret_file` 的修改時間（停機時換檔的情況）。沒有記錄或讀不懂 → 新記錄只有 `current`，`replaced` 為空。
- 採用條件：`previous_secret_file` 的內容 ≠ 現行密鑰，指紋等於記錄的 `replaced`，且 `changed_at` 距現在小於 120 秒（時間在未來視為現在）。否則 `previous_secret_file` 無效；設了它卻無效時記一行 `info` 說明原因（沒有記錄、記錄讀不懂、或它不是被換掉的那一把）。
- 執行中：`Keys::rotate` 成功（現行密鑰變了）時寫新記錄：`replaced` 為舊的現行指紋，`changed_at` 為現在。

這樣，「舊 previous 在下一次輪換後復活」（記錄的 `replaced` 是 K1，previous 檔是 K0）與「`touch` 或同內容重寫」（現行指紋與記錄相同，`changed_at` 是很久以前）都被擋下；「執行中換檔後重啟」與「停機換檔後啟動」都被允許。

### 檔案位置與格式

`<store.path>-peers`，與 store 的 `<path>-lock` 同一個慣例；模式 0600，寫到同目錄的暫存檔再 rename，避免寫一半。放在 store 旁而不是 LMDB 裡：store 每次啟動重建，且 `writer` 是 `Option`。deb／rpm 的 `/var/lib/emfor` 由 unit 的 `StateDirectory` 建立且可寫；自訂的 `store.path` 的目錄 store 本來就要能寫。格式用每行一個 `key=value` 的文字，不新增依賴；雜湊用已依賴的 `sha2`。

### 寫入失敗與讀取失敗

寫入失敗只警告（`warn`，每分鐘最多一次，沿用 `WARN_EVERY`），daemon 照常執行；下次啟動就沒有（或舊的）記錄，`previous_secret_file` 因此無效。讀取失敗視為沒有記錄。執行中換密鑰時的寫入放在獨立的執行緒（`std::thread::spawn`，因為 `Keys::rotate` 在同步的測試裡也會被呼叫，沒有 tokio runtime 可用），不阻塞 peer 迴圈（與 `emfor-secret-read-timeout` 的原則一致：目錄可能在慢的檔案系統上）：持有「最新記錄」的鎖只用來讀或換，不在寫檔時持有；寫檔另有一個只有寫的執行緒會拿的鎖。啟動時的那一次寫入是同步的（只發生一次，在還沒開始服務前）。

### 升級後第一次啟動不採用 previous

沒有記錄時 `previous_secret_file` 無效，不退回 mtime 的做法（使用者決定）。代價：升級當下剛好在輪換的 120 秒內重啟的那一台，行為和沒設 `previous_secret_file` 一樣；手冊的升級說明寫明。

## Implementation Contract

- `peers::start` 多收 store 路徑（`cli.rs` 的呼叫處已有 `store.path`），`start_keys` 依上述規則決定 `Keys`，並回傳需要寫入的記錄；`Keys::resumed` 的條件改由呼叫端先判斷（或改簽名），原本的單元測試（`previous_secret_file_survives_a_restart` 等）依新規則調整。
- `apply_secret` 在 `rotate` 回傳 true 時排程寫記錄。
- 記錄檔的讀寫與指紋放在 `peers/` 下的小模組或 `crypto.rs`，有單元測試：不存在、讀不懂、指紋相符與不符、寫入後讀回、模式 0600。
- 啟動測試用臨時目錄；不得依賴真實時間 120 秒（`start_keys` 已接受 `now`，記錄的 `changed_at` 以注入的時鐘或相對值測）。

## Risks / Trade-offs

- 記錄檔不見（目錄被清掉、換到新磁碟）：daemon 當作沒有記錄，只失去 `previous_secret_file` 的保護，與沒設它時相同，不會比現在更危險。
- 停機換檔的情況用 `secret_file` 的 mtime 當變更時間：可靠度與現在相同（那是沒有別的時間來源時的唯一依據）。
- 記錄存的是指紋，不是密鑰；指紋可被離線猜測的前提是密鑰本身很弱，而密鑰至少 32 位元組。
- 重啟中若 `emfor-secret-read-timeout` 的讀檔卡住，行為不變（本 change 只在啟動時讀記錄）。

## Open Questions

- 無。記錄檔名 `-peers` 是我選的，若與其他檔案衝突可再改。
