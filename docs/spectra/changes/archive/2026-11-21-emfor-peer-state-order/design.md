## Context

`peers/group.rs` 的 `Replay::accepts` 接受同一個 `boot` 下 `seq` 在視窗內（`REPLAY_WINDOW` 個數字，包含最高的那個）且尚未收過的訊息；`boot` 不同時要求 `ts` 大於最高已接受的 `ts`。所以對每個送端，被接受的訊息在 `boot` 上單向前進，在同一個 `boot` 內可以有限度地亂序。`peers/task.rs` 的 `received` 手上有 `msg.boot`、`msg.seq`，但只把 `Kind::Result` 的 `ResultMsg` 交給 `Quorum::record`、把 `Kind::Heartbeat` 的標籤交給 `Quorum::set_member_tags`，兩者都無條件覆蓋。

## Goals / Non-Goals

**Goals:**

- 較舊（同一個 `boot`、`seq` 較低）的 `result` 與 `heartbeat` 不覆蓋較新的狀態。
- 不改線上格式、不改重放視窗。

**Non-Goals:**

- `down` 訊息（事件，不是狀態）、`seen` 的 liveness 時間、視窗大小。
- 其他低嚴重度項目。

## Decisions

### 比較 (boot, seq)，boot 不同就當作較新

新增 `Stamp { boot: u64, seq: u64 }`（`quorum.rs` 內）與 `Stamp::older_than(self, kept: Stamp) -> bool`：同一個 `boot` 且 `seq` 低於 `kept.seq` 才回 `true`；`boot` 不同或 `seq` 相等都回 `false`（相等不會真的重複到達，重放檢查會擋；這樣測試用的固定 stamp 行為與修改前相同）。理由：重放檢查已保證另一個 `boot` 的被接受訊息 `ts` 更晚，也就是送端之後重啟過；同一個 `boot` 的 `seq` 由送端遞增，大者較晚。不比較 `ts`：`ts` 來自送端時鐘，只用來限制 5 秒偏差，不是排序依據。

替代方案：比較 `ts`。不採用：兩台主機的時鐘有偏差，同一個送端內 `seq` 已足夠且更精確。

### 結果的 stamp 放在 Kept，標籤的 stamp 另存一份

`Kept` 增加 `stamp: Stamp`；`Quorum::record(from, r, stamp, now)` 在同一個 `(service, backend)` 下，已有 `from` 的 `Kept` 且 `stamp` 不勝過它時直接返回（不改 `at`、`counted`）。標籤：`State` 增加 `tag_stamps: HashMap<SocketAddr, Stamp>`；`set_member_tags(member, tags, stamp)` 對已有的 stamp 不勝過時直接返回，否則更新標籤與 stamp。標籤在 `set_live` 因成員不再 live 被忘記時，stamp 不刪：舊的 heartbeat 在成員被判定為不 live 之後才到（`ts` 容許 5 秒偏差），不應該讓標籤復活。`retain_members`（reload 時成員變動）同時刪除 `tag_stamps` 中不在 `members` 的項目；`Kept` 隨 `votes` 既有的清理被刪。

替代方案：把 stamp 存在 `Group` 的重放狀態、由 `Group` 回報「這則訊息是否為該送端目前最新」。不採用：`Group` 沒有 I/O 也不認識 `Kind` 的語意，而且 `record` 與 `set_member_tags` 才是狀態的擁有者，由它們各自判斷（結果是每個後端各一份、標籤是每個成員一份）才正確；全域「最新」不能代表「這個後端的最新結果」。

### 被丟掉的訊息仍算接受

`received` 的其餘動作（`results_received` 計數、`note_mismatch`、liveness 由 `Group::accept` 已更新）不變；只有 `record` 與 `set_member_tags` 內部略過寫入。這樣 `meta/peers` 與 `/metrics` 的接受計數不因本修改而變。

## Implementation Contract

- **行為**：同一個送端、同一個 `boot` 下，`seq` 較低的 `result`／`heartbeat` 在較高者之後被接受時，保留的結果與標籤不變；順序正常或 `boot` 不同時與修改前完全相同。
- **介面**：`Quorum::record_at(&self, from, r: &ResultMsg, stamp: Stamp, now)`、`Quorum::set_member_tags_at(&self, member, tags, stamp: Stamp)`；`received` 以 `Stamp { boot: msg.boot, seq: msg.seq }` 呼叫。原本的 `record` 與 `set_member_tags` 只留給測試（`#[cfg(test)]`），以 `Stamp::default()` 呼叫，既有的約 35 個測試呼叫點不用改。`Stamp` 為 `pub`（`task.rs` 需要）。
- **失敗模式**：無新的錯誤路徑；`Stamp` 比較不可能失敗。
- **驗收**：(a) `quorum.rs` 單元測試：`Stamp::older_than` 的表（相同 boot 較高／較低／相等、不同 boot 任意 seq）；`record` 先 seq 11 `down` 再 seq 10 `up` 保留 `down` 且 `counted` 與 `at` 不動；`set_member_tags` 先 seq 11 有標籤再 seq 10 無標籤保留標籤；成員不 live 後舊 heartbeat 不讓標籤復活；`retain_members` 刪除 stamp。(b) `peers/task.rs` 的既有測試風格（`result_reaches_the_votes`、`kept_tags_follow_the_latest_heartbeat`）下的兩個測試：以 `received` 或同一路徑餵入亂序的兩則訊息，狀態為較新者；另一個 `boot` 的訊息取代舊的。(c) 既有 quorum 與 peers 測試不改斷言地通過；`schedule.rs` 的測試輔助函式只改呼叫簽章。(d) `tools/docker-test.sh` 全過。
- **範圍**：`quorum.rs`、`peers/task.rs`、`schedule.rs` 的測試輔助函式、`CHANGELOG.md`；範圍外見 Non-Goals。

## Risks / Trade-offs

- 記憶體：每個成員多一個 `Stamp`（16 位元組）、每個保留的結果多一個；成員數受 `members` 限制，後端數受設定限制，增量可忽略（估計，未量測）。
- 成員離開 `members` 後重新加入：`tag_stamps` 已刪，新訊息一律被接受，等同第一次見到。
- 時序上與 `set_live` 的相互作用：`tag_stamps` 不隨 live 消失，所以一個成員在不 live 一段時間後以相同 `boot` 回來，第一則 heartbeat 的 `seq` 必然更高（`seq` 只增不減），會被接受；未驗證的情形：送端的 `seq` 回繞（`u64`，實務上不會）。
- 未驗證：多台主機之間的真實網路亂序（測試以直接餵入訊息模擬）。
