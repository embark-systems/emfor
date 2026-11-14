## Context

store 是單一 LMDB 檔（`MDB_NOSUBDIR`），daemon 寫入，client、daemon 本身與 `emfor status` 都以 mmap 讀取。LMDB 讀頁前只檢查頁號小於 meta 記錄的下一頁，不檢查檔案長度；資料檔比 meta 記錄的長度短時，讀到檔尾之後的頁面會收到 SIGBUS。

store 會被開啟的地方有三個：

- client：`clients/rust/src/store.rs` 的 `Env::open`，直接呼叫 `lmdb-master-sys`。`Client::open` 與 `reopen_if_replaced` 都經過它，C、Go、Python、Perl、PHP 透過 libemfor 也用它。
- daemon 啟動：`crates/emfor-daemon/src/store.rs` 的 `open_env`，透過 heed 0.22 開檔後逐筆讀過整個資料庫，用來偵測損壞；`is_damage` 判定為損壞時，`set_aside` 把檔案移成 `<path>.corrupt`。
- `emfor status`：`crates/emfor-daemon/src/status.rs` 的 `Reader::open`，透過 heed 唯讀開檔；`--watch` 在檔案被換掉時重開。

daemon 不會讓檔案變短：用 pwrite 寫入、沒開 `WRITEMAP`，損壞時用 rename 移走。

但健康的檔案也可能比 LMDB 記錄的長度短。同一個交易裡配置後又釋放的頁面（loose page）不會寫入磁碟（`mdb_page_flush` 跳過 `P_LOOSE`），meta 頁仍把它們算進最後一頁，LMDB 也不延長檔案。實測（2026-11-15，Debian 映像，scratchpad 的 loose-probe）：隨機寫入 4 × 50,000 次 commit 都沒有發生；同一交易內先大量新增再刪除時，6 × 30,000 次 commit 中有 2 到 50 次 commit 後檔案比記錄短，最多短 45 頁。daemon 啟動時刪掉全部 `svc/` 再重寫，屬於後者。這些頁面沒有任何讀取者會讀，所以不會造成 SIGBUS，但只比對長度就會誤判。

真正被截短的檔案只有兩種來源：有人原地覆寫或截短檔案，或是 `sync = false` 時斷電。

## Goals / Non-Goals

**Goals:**

- 開檔時就太短的 store 不會讓 client、daemon 或 `emfor status` 收到 SIGBUS，而是各自走現有的錯誤處理。
- client-lookup 規格的「不得 crash」改成做得到的範圍。
- 文件告訴操作的人：不要原地修改 store，要換檔用 rename。

**Non-Goals:**

- 每次查詢都檢查長度：查詢是熱路徑，檢查和讀取之間仍可能被截短，只能縮小風險、擋不住。
- 在函式庫裡裝 SIGBUS handler：訊號處理是整個 process 共用的，會和 Go runtime、Python、PHP-FPM 自己的 handler 衝突。
- 不用 mmap 讀取，或每次查詢複製一份檔案：LMDB 本身靠 mmap 運作。
- 檔案在 client 開檔後才被原地截短、磁碟 I/O 錯誤：仍可能收到 SIGBUS，只寫進文件。
- 頁面內容損壞（bit flip）的完整驗證：LMDB 已對超出範圍的頁號回錯誤，其餘情況不在此 change 處理。

## Decisions

### daemon 把檔案補足到 LMDB 記錄的長度

daemon 讓自己寫出的檔案一定不短於 `(最後一頁 + 1) × 頁大小`，讀取端的長度檢查才不會誤判：

- 只在每次 commit 之後（`Store::write`）做：開一個寫入交易（拿到 LMDB 的寫入鎖，避免和其他 commit 同時寫檔），比對長度，短了就用 `set_len` 延長到記錄的長度，補上的部分是全零，然後放棄這個交易。
- 啟動時不補足：比記錄短的 store 一律當成損壞（見「daemon 把截短當成損壞」）。曾考慮補零後再逐筆讀取，讓真的截短由 LMDB 回報損壞、只短了沒人讀的頁面就保留；但實測 LMDB 把零頁當成有上萬筆資料的葉頁，debug 版因斷言 `IS_LEAF(mp)` abort，release 版會讀到頁面範圍外，所以不採用（2026-11-15，Debian 映像中 `truncated_store_is_replaced` 的實測）。
- 只延長、不縮短；延長的檔案 LMDB 之後照常用 pwrite 覆寫那些頁面。

另一個選擇是讀取端不檢查長度（client 開到被截短的檔案仍會 SIGBUS），範圍較小但保護較少，未採用。

### 開檔後立即比對檔案長度與 meta 記錄的最後一頁

這個比對的前提是「健康的檔案一定夠長」。這只對補足長度之後的 daemon 寫出的檔案成立（見 Context 與上一個決定），初版設計誤以為 LMDB 本身就保證。

LMDB 開檔成功、還沒開始第一個讀取交易之前，比對三個值：

- 檔案長度：LMDB 開著的那個檔案；
- 最後一頁：`mdb_env_info` 的 `me_last_pgno`，heed 的 `Env::info().last_page_number`；
- 頁大小：`mdb_env_stat` 的 `ms_psize`，heed 的 `Env::stat().page_size`。

檔案長度小於 `(最後一頁 + 1) × 頁大小` 時，判定為被截短。乘法溢位也算被截短。

這個時間點是安全的：

- `mdb_env_open` 讀 meta 頁用的是 pread，兩個 meta 頁讀不到時，開檔就已經失敗；
- `mdb_env_info` 與 `mdb_env_stat` 只讀 meta 頁，不碰資料頁。

另一個選擇是自己用 pread 解析 meta 頁，但那要複製 LMDB 的 meta 格式，所以不採用。

### 長度取自 LMDB 開著的檔案描述子

先讀 meta 頁（`mdb_env_info`、`mdb_env_stat`），再量長度。LMDB 先寫資料頁、最後才寫指向它們的 meta 頁，所以之後量到的長度一定涵蓋先前讀到的 meta；順序反過來的話，daemon 在兩步之間 commit 會造成誤判。

- client 用 `mdb_env_get_fd` 加 `fstat`。
- daemon 與 `emfor status` 用 heed 的 `Env::real_disk_size()`，內部同樣是對 LMDB 的 fd 做 metadata。

改用路徑 `stat` 的話，開檔和 `stat` 之間檔案可能被換掉，量到的會是另一個檔案。

### 截短的說明文字放在 emfor-format 共用

在 `crates/emfor-format` 加一個純函式 `truncated_store(len: u64, last_page: u64, page_size: u64) -> Option<String>`。檔案太短時回傳 `truncated: <len> bytes, LMDB records <expected> bytes`，否則回 `None`。乘法溢位時沒有應有長度可寫，改為 `truncated: <len> bytes, LMDB records page <last_page> of <page_size> bytes`。

client、daemon、`emfor status` 都用它，三處的判斷與用詞因此一致，也能單獨做單元測試。emfor-format 已經同時被 daemon 和 client 使用，不需要新增相依。

### daemon 把截短當成損壞

daemon 啟動時遇到比記錄短的 store（逐筆讀取之前檢查），走與 `MDB_CORRUPTED` 等錯誤相同的路徑：

- 移到 `<path>.corrupt`、刪掉 `<path>-lock`、建新 store；
- 警告訊息照現有格式 `state store <path> is damaged (<原因>); moved it to <path>.corrupt and starting with an empty store`。`<原因>` 是 `truncated_store` 的文字。只短了沒人讀的頁面的 store 無法在不讀到檔尾之後的情況下和真的被截短區分，所以也一併移走；這只會發生在舊版 daemon 留下的 store，或 daemon 在 commit 之後、補足之前結束時，損失的只有狀態歷史。

`open_env` 的錯誤型別需要能同時表達 heed 錯誤與截短，例如新增一個列舉，讓 `is_damage` 判定為 true。截短檢查要在逐筆讀取之前做。

### client 的截短是一次失敗的開檔

`Env::open` 回 `UNAVAILABLE`，訊息為 `cannot open store <path>: truncated: <len> bytes, LMDB records <expected> bytes`。它和其他「檔案存在但開不了」的錯誤一樣：

- 記進 `open_failure`，照 `OPEN_RETRY` 每秒最多重試一次；
- `reopen_if_replaced` 遇到截短的新檔時，繼續用原本開著的 store。

`Env` 要在回傳錯誤前關掉環境，與現有錯誤路徑相同。

## Implementation Contract

**行為**

- Rust client 與 libemfor（含 Go、Python、Perl、PHP 包裝）：開一個比 meta 記錄短的 store 時，查詢回 `UNAVAILABLE`，訊息含 `truncated`、store 路徑、實際長度與應有長度，process 繼續執行。一秒內的後續查詢回同一個錯誤、不重開檔；檔案換成正常的 store 後，下一次重試就讀得到。
- daemon 啟動：比記錄短的 store 被移到 `<path>.corrupt`，log 一行 warn，內容含路徑與 `truncated`，然後以新 store 正常運作，不會收到 SIGBUS。
- daemon 每次 commit 之後：資料檔長度不小於 `(最後一頁 + 1) × 頁大小`。
- `emfor status`：印出 `cannot open store <path>: truncated: ...` 並結束碼 2。`--watch` 顯示錯誤行，下次重畫時重試。
- 長度正常的 store，三處行為都和改動前相同；daemon 寫出的健康 store 不會被 client 或 `emfor status` 判成截短。

**介面**

- `emfor_format::truncated_store(len, last_page, page_size) -> Option<String>`。
- 錯誤碼與 C 狀態值不新增。

**驗收**

- `emfor_format` 單元測試涵蓋：長度剛好、少一頁、`len = 0`、乘法溢位。
- Rust client 測試（`clients/rust/src/client.rs`）：
  - 建 store、截掉最後一頁（截掉的長度用 store 自己的頁大小，也就是作業系統的頁大小），查詢回 `UNAVAILABLE`、訊息含 `truncated`；
  - 改動前這個測試會因 SIGBUS 失敗，實作時要先確認這點。
- daemon `store.rs` 測試：
  - 用現有的 `reopen_damaged` 截掉最後一頁，`state.mdb.corrupt` 存在，log 含 `truncated`，新 store 可寫入；
  - 改動前同樣因 SIGBUS 失敗。
- `status.rs` 測試：截短的 store 讓 `Reader::open` 失敗，訊息含 `truncated`。
- daemon `store.rs` 測試 `commits_keep_the_file_as_long_as_recorded`：在持有舊讀取交易時，反覆在同一交易內先大量新增再刪除，每次 `Store::write` 之後檔案長度都不小於 `(最後一頁 + 1) × 頁大小`；拿掉補足時這個測試要失敗。
- daemon `store.rs` 測試 `short_file_with_unread_pages_is_set_aside`：直接用 heed 以上一個測試的工作負載產生比記錄短、但讀取正常的檔案，`Store::open` 把它移到 `.corrupt` 並以新 store 正常運作。
- Rust client 測試 `truncated_replacement_keeps_the_open_store`：查詢過的 store 被換成一個截短的新檔後，查詢仍回原本 store 的後端。
- Debian 映像 `emfor-clients-test` 中，完整檢查全部通過：
  - `cargo fmt --check`（含 fuzz/）；
  - clippy `-D warnings`（workspace，以及 `--cfg fuzzing`）；
  - `cargo test --workspace --all-features`；
  - `make -C clients test`。

**範圍**

- 範圍內：上述三處開檔、daemon 每次 commit 後補足長度、`emfor-format` 的函式、client-lookup／state-store／status-command 規格、manual 第 08 與 09 章（英文與 zh_TW）、`docs/deploy/daemon.md`、CHANGELOG。
- 範圍外：每次查詢的檢查、訊號處理、執行中被截短的偵測、頁面內容驗證。

## Risks / Trade-offs

- [client 剛好在 daemon commit 之後、補足長度之前開檔，健康的 store 被判成截短] → 這段時間只有一次 `fstat` 加可能一次 `ftruncate`（估計幾微秒，沒有量過）；client 拿到 `UNAVAILABLE` 後一秒內重試就會成功；`reopen_if_replaced` 則繼續用原本的 store。
- [daemon 在 commit 之後、補足之前結束] → 下次啟動時把 store 移到 `.corrupt`，只損失狀態歷史；在那之前開檔的 client 拿到 `UNAVAILABLE`。
- [舊版 daemon 寫出、剛好比記錄短的 store] → 升級後第一次啟動時移到 `.corrupt`，只損失狀態歷史；機率依實測，隨機寫入時未觀察到。

- [client 開檔之後檔案才被原地截短，仍會收到 SIGBUS] → 文件寫明不可原地修改 store。daemon 自己也會在同一時間收到 SIGBUS，所以這本來就是要操作的人避免的錯誤。
- [檔案比 meta 記錄的長度長，例如 LMDB 預先配置的空間] → 只判定「比較短」的情況，比較長一律視為正常。
- [`me_last_pgno` 在開檔後會隨 daemon 寫入而增加] → 只在開檔時比對一次。daemon 只會讓檔案變長，所以之後的正常成長不會誤判。
