## 1. 狀態的順序

- [x] 1.1 實作設計決定「比較 (boot, seq)，boot 不同就當作較新」與「結果的 stamp 放在 Kept，標籤的 stamp 另存一份」：`crates/emfor-daemon/src/quorum.rs` 新增 `Stamp` 與 `older_than`、`Kept.stamp`、`State.tag_stamps`，新增 `record_at` 與 `set_member_tags_at`（帶 `stamp`，較舊時略過；原本的兩個函式改為 `#[cfg(test)]` 並以預設 stamp 呼叫），`retain_members` 清理 `tag_stamps`；`crates/emfor-daemon/src/peers/task.rs` 的 `received` 傳入 `Stamp { boot: msg.boot, seq: msg.seq }`；`schedule.rs`、`supervisor.rs` 與 quorum 既有測試的呼叫點不用改。完成時：既有 quorum、schedule、peers 測試不改斷言地通過；clippy 乾淨。
- [x] 1.2 [after: 1.1] 實作 spec peer-group "Peer heartbeats and liveness" 與 "Assigned checkers and check results" 的四個新情境與設計決定「被丟掉的訊息仍算接受」：`quorum.rs` 的單元測試（`Stamp::older_than` 表；先 seq 11 `down` 再 seq 10 `up` 保留 `down` 且 `counted`、`at` 不動；先 seq 11 有標籤再 seq 10 無標籤保留標籤；成員不 live 後舊 heartbeat 不讓標籤復活；`retain_members` 刪除 stamp）與 `peers/task.rs` 的兩個測試，沿用 `result_reaches_the_votes` 與 `kept_tags_follow_the_latest_heartbeat` 的寫法（先讀這兩個測試再寫）：亂序的兩則訊息經 `received` 之後狀態為較新者，且 `results_received` 計數兩則都算；另一個 `boot` 的訊息取代舊的。完成時：測試通過；新測試在舊程式下的行為以「如預期失敗」或「未對舊程式跑過」如實回報。

## 2. 文件與驗證

- [x] 2.1 [after: 1.2] `CHANGELOG.md` Unreleased 加條目：亂序到達的較舊 `result`／`heartbeat` 不再覆蓋較新的票與標籤；Checked（Debian 容器 fmt、clippy `-D warnings`、測試數字）與 Not verified（真實網路亂序；新測試對舊程式的結果）。`docs/manual/` 先 grep 是否有描述「最新結果」的地方，沒有就不改。完成時：`tools/docker-test.sh` 全過，`spectra analyze` 與 `spectra validate` 乾淨。
