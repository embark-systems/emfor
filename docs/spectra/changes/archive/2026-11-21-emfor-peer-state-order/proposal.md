## Problem

對等節點送來的 `result` 與 `heartbeat` 訊息可能亂序到達：`crates/emfor-daemon/src/peers/group.rs` 的重放視窗（spec peer-group "Peer message authentication and replay protection"）會接受 `seq` 較低、視窗內、尚未收過的訊息。接受之後，`crates/emfor-daemon/src/peers/task.rs` 的 `received` 把 `Kind::Result` 交給 `Quorum::record`、把 `Kind::Heartbeat` 交給 `Quorum::set_member_tags`（兩個都在 `crates/emfor-daemon/src/quorum.rs`），兩者都無條件覆蓋已存的值。所以一個被網路超車的舊結果，在較新的結果之後才被接受時，會蓋掉較新的票（並且因為 `Kept.at` 取接收當下的本機時間，看起來比較新、重新被算作新鮮）；舊的 heartbeat 則會蓋掉較新的服務標籤。後果是一次投票或「誰宣告了離開群組的服務」暫時依據過時的資料，直到該送端的下一則訊息（約一個 interval 或一秒內）才修正。

## Root Cause

「最新」在程式與 spec 裡都被定義成「最後被接受的」（spec："latest result … together with … the time the receiver accepted it"、"the tags of the latest heartbeat it accepted"），而不是「送端最後送出的」。訊息被接受時 `received` 手上有 `msg.boot` 與 `msg.seq`，但沒有把它們傳進 `record` 與 `set_member_tags`，狀態裡也沒有地方放。

## Proposed Solution

不改線上格式：`seq` 已是經過驗證、每個送端單調遞增的（`boot` 不同時重放檢查要求 `ts` 更大）。在 `Quorum` 裡保存「每筆保留的狀態來自哪一個 `(boot, seq)`」，較舊的就丟掉：

- `Kept` 加一個 `stamp`（`boot` 與 `seq`）；`record` 收到同一個 `boot`、`seq` 較低的結果時不覆蓋。
- 每個成員另存一個標籤用的 `stamp`；`set_member_tags` 對同一個 `boot`、`seq` 較低的 heartbeat 不動作。這個 `stamp` 在成員不再 live 時保留（標籤被忘記，序號不忘記），在成員離開 `members` 時刪除。
- `boot` 不同一律視為較新：重放檢查只接受 `ts` 更大的另一個 `boot` 的訊息，所以送端重啟後的訊息會取代舊的。

被丟掉的訊息仍然算作「接受」：它讓成員保持 live，也計入 `results_received` 等計數；只是不改變票與標籤。

## Non-Goals (optional)

- 不改線上格式與重放視窗（視窗大小、`REPLAY_WINDOW` 不動）。
- 不處理 `down` 訊息：它是事件，不是狀態，以到達順序交給 backend 任務；亂序的 `down` 不會留下過時的狀態。
- 不改 `Group.peers[..].seen`（以到達時間計的 liveness）。
- 不處理其他低嚴重度項目（pf 範例腳本、CI、Nagios 範例、發版簽章、`env_clear`、威脅模型）。

## Success Criteria

- 同一個送端、同一個 `boot`，先接受 `seq` 11 的 `down` 結果、再接受 `seq` 10 的 `up` 結果後，該送端對該後端保留的仍是 `down`；先接受 `seq` 10 再接受 `seq` 11 時保留 `down`（與修改前相同）。
- 先接受 `seq` 11 的 heartbeat（宣告服務標籤 T）、再接受 `seq` 10 的 heartbeat（沒有標籤）後，該成員的保留標籤仍是 T。
- 送端重啟（另一個 `boot`、`seq` 1、較大的 `ts`）之後的結果與 heartbeat 取代舊的。
- 既有的 quorum 與 peers 測試不改斷言地通過；`tools/docker-test.sh` 全過。

## Impact

- Affected specs: peer-group
- Affected code:
  - Modified: `crates/emfor-daemon/src/quorum.rs`、`crates/emfor-daemon/src/peers/task.rs`、`crates/emfor-daemon/src/schedule.rs`（只改呼叫 `record` 的測試輔助函式）、`CHANGELOG.md`
  - New: (none)
  - Removed: (none)
