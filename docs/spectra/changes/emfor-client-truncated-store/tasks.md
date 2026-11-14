## 1. 共用的長度判斷

- [x] 1.1 依 design「截短的說明文字放在 emfor-format 共用」，在 `crates/emfor-format` 新增 `truncated_store(len, last_page, page_size) -> Option<String>`。
  - 檔案長度小於 `(last_page + 1) × page_size`，或這個乘法溢位時，回傳 `truncated: <len> bytes, LMDB records <expected> bytes`；其餘情況回 `None`。
  - 驗證：emfor-format 的單元測試涵蓋 client-lookup 規格「Example: length check」的三列，以及 `len = 0` 和乘法溢位。

## 2. client 開檔檢查

- [x] 2.1 [after: 1.1] 實作 client-lookup 規格「Lookup failures use four shared error codes」的截短檢查：依 design「開檔後立即比對檔案長度與 meta 記錄的最後一頁」「長度取自 LMDB 開著的檔案描述子」「client 的截短是一次失敗的開檔」，修改 `clients/rust/src/store.rs` 的 `Env::open`。
  - 檢查時機：`mdb_env_open` 成功後、第一個 `mdb_txn_begin` 之前。
  - 取值：`mdb_env_get_fd` 加 `fstat` 取檔案長度，`mdb_env_info` 取最後一頁，`mdb_env_stat` 取頁大小，再交給 `truncated_store` 判斷。
  - 檔案太短時：關閉環境，回 `UNAVAILABLE`，訊息為 `cannot open store <path>: truncated: ...`。
  - 這個失敗與其他開檔失敗一樣，經 `OPEN_RETRY` 每秒最多重試一次；`reopen_if_replaced` 遇到截短的新檔時，繼續用原本開著的 store。
  - 驗證：`clients/rust/src/client.rs` 新增測試 `truncated_store_is_unavailable`，滿足 client-lookup 規格「Lookup failures use four shared error codes」的 Scenario「Truncated store fails instead of crashing」：
    - 建一個含 `api.svc.emfor` 的 store，截掉最後一頁；截掉的長度是作業系統頁大小，也就是 LMDB 的頁大小；
    - 查詢回 `UNAVAILABLE`，訊息含路徑與 `truncated`；
    - 先在改動前執行，確認測試 process 因 SIGBUS 失敗，表示這個測試真的測得到問題。

## 3. daemon 與 emfor status 開檔檢查

- [x] 3.1 [after: 1.1] 實作 state-store 規格「Damaged store is replaced at start」的截短情況：依 design「daemon 把截短當成損壞」，修改 `crates/emfor-daemon/src/store.rs` 的 `open_env`。
  - 檢查時機：heed 開檔後、逐筆讀取之前。
  - 取值：`Env::real_disk_size()`、`Env::info().last_page_number`、`Env::stat().page_size`，交給 `truncated_store` 判斷。
  - 截短時走與 `MDB_CORRUPTED` 相同的 `set_aside` 路徑，warn 訊息的原因是 `truncated: ...`。
  - 驗證：`store.rs` 新增測試，滿足 state-store 規格「Damaged store is replaced at start」的 Scenario「Truncated store at start」：
    - 用 `reopen_damaged` 截掉最後一頁（長度取自 `page`）；
    - 結果應為 `state.mdb.corrupt` 存在、log 含 `truncated`、新 store 可寫入；
    - 先在改動前執行，確認因 SIGBUS 失敗。
- [x] 3.2 [after: 1.1] 實作 status-command 規格「Status reads the store read-only」的截短情況：依 design「開檔後立即比對檔案長度與 meta 記錄的最後一頁」，修改 `crates/emfor-daemon/src/status.rs` 的 `Reader::open`。
  - 檢查時機：heed 開檔後、`read_txn` 之前。
  - 截短時回 `Failure("cannot open store <path>: truncated: ...")`，`emfor status` 結束碼 2；`--watch` 把它當成讀取失敗，下次重畫時重試。
  - 驗證：`status.rs` 新增測試，滿足 status-command 規格「Status reads the store read-only」的 Scenario「Truncated store」：截短的 store 讓 `Reader::open` 回傳的訊息以 `cannot open store <path>: truncated:` 開頭。

## 4. 文件

- [x] 4.1 [after: 2.1, 3.1] 在 `docs/manual/08-operations.md` 與 `docs/manual.zh_TW/08-operations.md` 的 store 段落、`docs/deploy/daemon.md` 的損壞 store 段落，說明以下幾點：
  - 不可原地覆寫或截短 store（`cp` 蓋過去、`truncate`、shell 的 `>` 重導向）；
  - 要換檔就先寫到同一目錄的另一個檔案，再用 rename 換上；
  - 原地修改會讓 daemon 和所有 client 的 process 因 SIGBUS 結束；
  - 開檔時就已經被截短的 store，daemon 會移到 `.corrupt`，client 會回 `UNAVAILABLE`。

  驗證：人工審閱三處內容一致，英文與 zh_TW 的內容相符。
- [x] 4.2 [after: 2.1, 3.1, 3.2] 在 `CHANGELOG.md` 的 Clients、Daemon、emfor status 段落各加一條，說明截短的 store 如今回 `UNAVAILABLE`、被移到 `.corrupt`、讓 `emfor status` 結束碼 2，不再收到 SIGBUS。驗證：人工審閱。

## 5. 驗證

- [x] 5.1 [after: 4.1, 4.2] 在 Debian 映像 `emfor-clients-test` 中跑完整檢查，全部通過：
  - `cargo fmt --all --check` 與 `(cd fuzz && cargo fmt --check)`；
  - `cargo clippy --workspace --all-targets --all-features -q -- -D warnings`；
  - `RUSTFLAGS="--cfg fuzzing" cargo clippy -q -p emfor-check -p emfor-daemon --target-dir target/fuzzcheck -- -D warnings`；
  - `cargo test --workspace --all-features --no-fail-fast`；
  - `make -C clients test`，涵蓋 C、Go、Python、Perl、PHP 與 conformance。

  記下通過數，以及 2.1 與 3.1 在改動前因 SIGBUS 失敗的證據。

## 6. review 後的修正：健康的 store 不得被判成截短

變更說明：review 發現 LMDB 不寫入同一交易內配置又釋放的頁面，健康的檔案也可能比記錄短（實測最多 45 頁），3.1 與 3.2 完成時的前提不成立。以下任務補上 daemon 端的保證，並修正讀取順序。3.1、3.2 的已完成紀錄保留不變。

- [x] 6.1 [after: 3.1, 3.2] 依 design「長度取自 LMDB 開著的檔案描述子」，client 的 `check_length`、daemon 的 `truncation` 改成先讀 meta 頁（`mdb_env_info`/`mdb_env_stat`、`env.info()`/`env.stat()`），再量長度，避免 daemon 在兩步之間 commit 時造成誤判。驗證：程式審閱確認順序與註解；client-lookup 規格「Lookup failures use four shared error codes」相關的既有測試 `truncated_store_is_unavailable`、`reader_refuses_a_truncated_store` 照樣通過。
- [x] 6.2 [after: 6.1] 實作 state-store 規格「Damaged store is replaced at start」的補足長度：依 design「daemon 把檔案補足到 LMDB 記錄的長度」，在 `crates/emfor-daemon/src/store.rs` 讓 `Store::write` 每次 commit 後，在寫入交易內把短的資料檔用 `set_len` 補零延長到 `(最後一頁 + 1) × 頁大小`，然後放棄該交易；只延長、不縮短。`open_env` 照 3.1 在逐筆讀取前把比記錄短的 store 當成損壞，不補零（補零後讀取會讓 LMDB abort 或讀到頁面外）。驗證：
  - 新測試 `commits_keep_the_file_as_long_as_recorded`（Scenario「Commits keep the file as long as LMDB records」）：持有舊讀取交易，反覆在同一交易內大量新增再刪除，每次 `Store::write` 後檔案長度不小於記錄；先在拿掉補足時執行，確認失敗。
  - 新測試 `short_file_with_unread_pages_is_set_aside`（Scenario「Store left short by pages nobody reads is set aside」）：直接用 heed 以同一工作負載產生比記錄短、但讀取正常的檔案，`Store::open` 把它移到 `.corrupt` 並以新 store 正常運作。
  - 既有的 `truncated_store_is_replaced` 照樣通過。
- [x] 6.3 [after: 6.1] Rust client 新增測試 `truncated_replacement_keeps_the_open_store`，對應 client-lookup 規格「Clients are safe across threads and fork」中「新檔開不了時繼續用原本的 store」：查詢過的 stale store 被換成一個截短的新檔（另一個 inode），查詢仍回原本 store 的後端。驗證：該測試通過。
- [x] 6.4 [after: 6.2] 依 design 的 Context 更新文件：`docs/deploy/daemon.md` 與 manual 第 08 章（英文、zh_TW）說明 daemon 每次 commit 後會把比記錄短的檔案補零延長，啟動時遇到比記錄短的 store 一律移到 `.corrupt`（包括舊版 daemon 留下、只短了沒人讀的頁面的 store，只損失狀態歷史）；CHANGELOG 的 Daemon 條目同步。驗證：人工審閱英文與 zh_TW 一致。
- [x] 6.5 [after: 6.2, 6.3, 6.4] 在 Debian 映像 `emfor-clients-test` 重跑 5.1 的完整檢查，全部通過，記下通過數，並更新 commit 訊息草稿。
