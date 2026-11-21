## 1. 量測與實作

- [x] 1.1 量出 daemon 在 commit 與 `pad` 之間讓資料檔比 meta page 記錄的短多久。寫一個不進版控的暫時測試：寫入迴圈不斷更新 store，另一個執行緒取樣檔案長度與 meta page 記錄，記下最長的連續「短」時間；結果寫進 `docs/spectra/changes/emfor-client-truncated-recheck/design.md`（「Decisions」的「次數與間隔：暫定 5 次、每次 20 ms，先量測再定」一節）。若最長值超過約 30 ms，先改 design 的次數與間隔再繼續；沒有看到任何短的瞬間也要記下「量測方法能否看到窗口」。驗證：design.md 內有量測數字或明確的「未觀察到」說明，暫時測試已刪除（`git status` 沒有多出檔案）。
- [x] 1.2 [after: 1.1] 在 `clients/rust/src/store.rs` 實作長度重查（design 的「在 `Env::open` 內重查長度，不在外層重開」與「次數與間隔」）：`LENGTH_RECHECKS`、`LENGTH_RECHECK_PAUSE` 常數，內部函式回傳 `Result<Option<String>, Error>`，仍短才回 `UNAVAILABLE`（訊息格式不變），其他錯誤不重查。驗證：`tools/docker-test.sh` 內 `clients/rust` 的現有測試 `truncated_store_is_unavailable` 與 `unopenable_store_is_retried_after_a_pause` 不改而通過。
- [x] 1.3 [after: 1.2] 加單元測試：（a）檔案截短一頁、約 30 ms 後另一個執行緒把長度還原，lookup 成功且緊接著的 lookup 沒有被快取的 `UNAVAILABLE`（對應 spec 情境「Store padded while the client opens it」）；（b）不還原時回 `UNAVAILABLE` 且訊息含 `truncated`，所花時間不超過 `LENGTH_RECHECKS × LENGTH_RECHECK_PAUSE` 加合理餘量（design「失敗的代價」：真正壞掉的 store 多等的時間有上限）。破壞檢查：把重查次數暫時設成 0，（a）必須失敗，之後還原。驗證：Docker 內測試通過，並記下破壞檢查的失敗輸出。

## 2. 文件與收尾

- [x] 2.1 [after: 1.3] 更新 `docs/manual/06-clients.md` 與 zh_TW 版（「存在但打不開的 store」一段加上長度重查與約 100 ms 上限，數字以 1.1 之後的定案為準）、`CHANGELOG.md` Unreleased 加一行，並讓 `client-lookup` 的「Lookup failures use four shared error codes」需求（delta spec）的次數與間隔與定案一致。驗證：`python3 tools/check-manual-links.py` 通過、`tools/docker-test.sh` 完整 gate 通過、`spectra analyze` 與 `spectra validate` 無 Critical。
