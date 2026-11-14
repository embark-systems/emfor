## Problem

client 以 mmap 讀 store。資料檔比 LMDB meta 頁記錄的最後一頁短時，讀到檔尾之後的頁面會收到 SIGBUS，整個應用程式 process 直接結束；`catch_unwind` 擋不住訊號。client-lookup 規格寫「A malformed store SHALL NOT make the lookup crash, panic or abort」，現況做不到。daemon 啟動時逐筆讀過 store 檢查損壞（`open_env`），遇到截短的檔案同樣收到 SIGBUS，在 systemd 下會一直重啟，永遠走不到「把損壞的 store 移到 `.corrupt`」那一步；`emfor status` 也一樣。

## Root Cause

三個開檔的地方（`clients/rust/src/store.rs` 的 `Env::open`、`crates/emfor-daemon/src/store.rs` 的 `open_env`、`crates/emfor-daemon/src/status.rs` 的 `Reader::open`）都只靠 LMDB 回報錯誤。LMDB 讀頁前只檢查頁號是否小於 meta 記錄的下一頁（超出時回 `MDB_PAGE_NOTFOUND`），不檢查檔案長度，所以頁號合法、檔案卻比較短時，讀取就落在 mmap 的檔尾之後。

daemon 本身不會讓檔案變短（用 pwrite 寫入、沒開 WRITEMAP；損壞時用 rename 移走）。但 LMDB 也不保證健康的檔案夠長：同一交易內配置又釋放的頁面不會寫入磁碟，meta 頁卻把它們算進最後一頁（實測最多短 45 頁，見 design.md）。這些頁面沒人會讀，不會造成 SIGBUS，但讓「只比對長度」會誤判。真正會造成 SIGBUS 的截短只在兩種情況發生：

- 有人原地覆寫或截短檔案（`cp 備份 state.mdb`、`truncate`、shell 的 `>` 重導向）；
- `[store] sync = false` 時斷電，meta 頁寫到了，檔案長度卻沒有。

release 版的 LMDB 定義了 `NDEBUG`（`lmdb-master-sys` 的 build.rs），`mdb_assert` 不會 abort，所以不需要處理 abort。

## Proposed Solution

- daemon 讓自己寫出的檔案一定不短於 LMDB 記錄的長度：每次 commit 之後，短了就補零延長。
- 三個開檔的地方在 LMDB 開檔成功後、第一次讀取之前，比對資料檔長度與 `(最後一頁 + 1) × 頁大小`；最後一頁取自 `mdb_env_info` 的 `me_last_pgno`，頁大小取自 `mdb_env_stat` 的 `ms_psize`，之後才量 LMDB 開著的那個檔案的長度。檔案太短就判定為被截短：
  - client：這次開檔失敗，回 `UNAVAILABLE`，訊息寫明 store 被截短、檔案長度與應有長度；照現有規則每秒最多重試一次。
  - daemon 啟動：當成損壞，照「Damaged store is replaced at start」移到 `<path>.corrupt` 並建新 store，警告訊息寫明被截短。只短了沒人讀的頁面的 store 無法安全地和真的被截短區分，也一併移走，只損失狀態歷史。
  - `emfor status`：印出 `cannot open store <path>: ...` 並結束碼 2；`--watch` 顯示錯誤行並在下次重畫時重試。
- client-lookup 規格把「不得 crash」改窄：LMDB 回報損壞的 store 與開檔時就太短的檔案都回 `UNAVAILABLE`；client 開檔之後才被原地修改的檔案、以及磁碟 I/O 錯誤，不在保證範圍內。
- 文件（manual 英文與 zh_TW、`docs/deploy/daemon.md`）寫明：不可原地覆寫或截短 store，要換檔用 rename；原地修改會讓 daemon 和所有 client 因 SIGBUS 結束。

## Non-Goals

另見 design.md。不做每次查詢檢查長度、不在函式庫裡裝 SIGBUS handler、不改用非 mmap 的讀法。

## Success Criteria

- 把一個有資料的 store 截掉最後一頁後，Rust client 查詢回 `UNAVAILABLE`、訊息含 `truncated`，測試 process 沒有被 SIGBUS 結束；改動前同一個測試會因 SIGBUS 失敗。
- 同樣截短的 store，daemon 啟動時把它移到 `<path>.corrupt` 並以新 store 正常啟動；改動前會因 SIGBUS 結束。
- 在同一交易內大量新增再刪除的工作負載下，每次 daemon commit 之後檔案長度都不小於 LMDB 記錄的長度。
- 同樣截短的 store，`emfor status` 結束碼 2、訊息含 `truncated`。
- Debian 映像 `emfor-clients-test` 中 fmt、clippy、`cargo test --workspace --all-features`、`make -C clients test` 全部通過。

## Impact

- Affected code:
  - Modified: crates/emfor-format/src/lib.rs, clients/rust/src/store.rs, clients/rust/src/client.rs, crates/emfor-daemon/src/store.rs, crates/emfor-daemon/src/status.rs, docs/manual/08-operations.md, docs/manual.zh_TW/08-operations.md, docs/manual/09-troubleshooting.md, docs/manual.zh_TW/09-troubleshooting.md, docs/deploy/daemon.md, CHANGELOG.md; the client-lookup, state-store and status-command specs change when this change is archived
  - New: none
  - Removed: none
