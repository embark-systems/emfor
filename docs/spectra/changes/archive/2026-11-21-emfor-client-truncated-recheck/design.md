## Context

daemon 的寫入路徑（`crates/emfor-daemon/src/store.rs`）：`Store::write` 先 `wtxn.commit()`，再 `pad(&self.env, &self.data)`。`pad` 開一個寫入交易（會等 commit 完成），算出 `(last_page_number + 1) × page_size`，只在資料檔比它短時 `set_len`，從不縮短檔案。註解說明了原因：LMDB 在同一個交易裡配置又釋放頁面之後，檔案可能比 meta page 記錄的短。所以 commit 完成到 `pad` 完成之間有一個窗口，檔案比記錄的短。daemon 自己開 store 時對同樣的情形也有 `truncation` 檢查（短就移到 `<path>.corrupt` 重建），但那是 daemon 啟動時，窗口不存在。

client 這一側只有 Rust 實作（`clients/rust`）：`Env::open`（`clients/rust/src/store.rs`）在 `mdb_env_open` 成功之後、讀任何資料頁之前呼叫 `check_length`：先 `mdb_env_info`／`mdb_env_stat` 讀 meta page，再對 LMDB 開的那個檔案描述子 `fstat` 取長度（先讀 meta 再量長度，所以 daemon 在中間 commit 的話，量到的長度涵蓋較新的頁面），然後 `emfor_format::truncated_store(len, last_pgno, psize)`：短就回 `UNAVAILABLE`。`Client::open` 把失敗連同時間記在 `open_failure`，`OPEN_RETRY`（1 秒）內的查詢直接回同一個錯誤。C、Go、Python、Perl、PHP 的 client 都呼叫 libemfor 的 `emfor_lookup`，沒有自己的開檔或重試。現有測試：`truncated_store_is_unavailable`（`clients/rust/src/client.rs`，先截短檔案再開）、`unopenable_store_is_retried_after_a_pause`、`crates/emfor-format` 的 `truncated_store_rule`；沒有任何測試涵蓋「檔案一會兒之後就補長」的情形。

已查證：窗口真的存在於程式結構上（commit 與 `pad` 是兩個步驟）。**未量測**：窗口實際有多長，TODO 寫「微秒到毫秒」，沒有數據；下面的次數與間隔是估計值，第一個任務先量。

## Goals / Non-Goals

**Goals:**

- client 在窗口內第一次開 store 時不再失敗（也不再被快取 1 秒）。
- 真正被截短的 store 仍然以 `UNAVAILABLE`（訊息不變）失敗，不會 `SIGBUS`、不會卡住。
- 失敗的額外延遲有上限且很小。

**Non-Goals:**

- 不改 daemon 的 commit／`pad` 順序或時機。
- 不改 `OPEN_RETRY`、檔案被換掉的偵測、已經開著的 store 被改短（`SIGBUS`，在保證之外）。
- 不改 C、Go、Python、Perl、PHP 的包裝（它們沒有開檔邏輯）。

## Decisions

### 在 `Env::open` 內重查長度，不在外層重開

`check_length` 改成：把「讀 meta 與長度並比對」拆成一個回傳 `Result<Option<String>, Error>` 的內部函式（`Some(why)` 表示 truncated，錯誤表示 LMDB 呼叫失敗），外層迴圈在 `Some` 時睡 `LENGTH_RECHECK_PAUSE` 後重新呼叫它，最多重查 `LENGTH_RECHECKS` 次；仍然是 `Some` 才回 `UNAVAILABLE`，訊息用最後一次的 `why`（與現在的格式相同）。`mdb_env_info` 每次呼叫都重新取最新的 meta page，所以重查同時看到 daemon 之後 commit 的 meta 與新的檔案長度。其他錯誤（不是 truncated）不重查，立刻失敗。LMDB 的環境保持開著，不必每次重開。

理由：窗口的原因是檔案長度還沒補上，等一下再量就好；重開整個環境沒有任何額外資訊，還要重做 `mdb_env_open`。

### 次數與間隔：暫定 5 次、每次 20 ms，先量測再定

這是估計值（TODO 說窗口是微秒到毫秒）：總上限約 100 ms，大約是「正常窗口的 10 倍以上」。任務 1.1 在 Docker 裡用一個不 commit 的暫時測試量出 daemon 這一側檔案「比記錄短」的最長時間（寫入迴圈加一個取樣執行緒，記錄最長連續短的時間）；若量到的最長值超過約 30 ms，就回來修改這份 design 的數字（與 tasks）再往下做。量不到任何短的瞬間，也要把「量測方法能否看到窗口」寫進 design（沒看到不等於不存在）。

**量測結果（task 1.1，Debian Docker 內，aarch64，一次執行）**：暫時測試對 `Store::write` 連續寫 2000 次（每次放入並刪除同一交易內的大值，以引出「配置又釋放」的情形），另一個自旋執行緒依 client 的順序（先讀 meta 的 last page、再量檔案長度）取樣約 96900 次。看到 1730 次「偏短」的樣本（所以量測方法看得到窗口），最長的連續偏短時間為 11.9 µs（受取樣間隔限制）。同樣的測試跑 20000 次寫入時 11 分鐘沒有結束，已中止，沒有取得數據。限制：只在 Docker 的 Linux 上量，沒有 macOS／FreeBSD；寫入執行緒在 commit 與 `pad` 之間被排程器搶走時，窗口會更長，這個測試沒有刻意製造該情形。結論：最長值遠低於 30 ms 的門檻，維持 5 次 × 20 ms（約 100 ms，約為量到的最大值的 8000 倍，足以涵蓋被排程搶走的情形）。暫時測試已刪除。

### 失敗的代價

重查發生在 `Client::open` 持有寫鎖的時候（第一次開 store 與每秒一次的重開）。真正被截短的 store，每次重開多等最多約 100 ms，在這段時間內同一個 client 上的其他執行緒的查詢會等這把鎖；失敗之後 1 秒內的查詢仍直接回快取的錯誤，不受影響。這是可接受的：被截短的 store 本來就是故障狀態（daemon 重啟會把它移到 `.corrupt`）。

**更替路徑不重查**：`Client::reopen_if_replaced`（偵測到 store 檔被換掉時切換過去）也呼叫 `Env::open`，而且在持有寫鎖時、約每 `REPLACE_CHECK_MS`（100 ms）就會再試一次。若那裡也重查，偏短的替換檔會讓每次嘗試在鎖內睡約 100 ms，該 client 的查詢幾乎一直被擋住。所以那條路徑改用 `Env::open_rechecking(path, 0)`：偏短就立刻拒絕、保留原本開著的 store，下一次嘗試（約 100 ms 後）再看；`Client::open` 才使用預設的重查次數。測試：`truncated_replacement_keeps_the_open_store` 量兩次 lookup 的總時間小於重查的總時間（把 0 改回預設次數時它必須失敗）。

## Implementation Contract

**行為**

- 開 store 時資料檔比 meta page 記錄的短，但在重查期間內被補長：這次開成功，查詢回地址，`open_failure` 沒有被設。
- 一直都短（超過重查的總時間）：`UNAVAILABLE`，訊息含 store 路徑、`truncated`、檔案長度與記錄長度（與現在相同的文字），並照原樣被快取 1 秒。
- 其他開檔錯誤（不存在、權限、格式不符）：不重查，行為不變。

**介面**

- `clients/rust/src/store.rs`：`LENGTH_RECHECKS`、`LENGTH_RECHECK_PAUSE` 常數；`check_length` 的簽名保持（對外仍是 `Result<(), Error>`），內部多一個可傳入次數與間隔的函式供測試使用。沒有公開 API、C API、標頭檔或 JSON 的變動。

**失敗模式**

- 重查時 LMDB 呼叫失敗：立刻回該錯誤（與現在對 `mdb_env_info` 失敗的處理相同）。
- 睡眠發生在呼叫 `Env::open` 的執行緒上；不使用非同步或背景執行緒（client 是同步程式庫）。

**驗收**

- 單元測試：把檔案截短一頁，另一個執行緒在約 10 ms 後把長度還原，`Env::open`（或 `Client::lookup`）成功；對照組：不還原，失敗訊息含 `truncated`，所花的時間不超過 `LENGTH_RECHECKS × LENGTH_RECHECK_PAUSE` 加上合理餘量。現有的 `truncated_store_is_unavailable` 與 `unopenable_store_is_retried_after_a_pause` 不改而通過。
- 破壞檢查：把重查次數設成 0，「補長」的測試要失敗。
- 完整 gate（`tools/docker-test.sh`）、`clients/` 的 conformance 測試在 Docker 內通過。

**範圍**

- 在範圍內：`clients/rust/src/store.rs` 與其測試、手冊 06 章（中英）、CHANGELOG、`client-lookup` spec 的 delta。
- 不在範圍內：見 Non-Goals。

## Risks / Trade-offs

- **窗口長度未量測**：若實際窗口比 100 ms 長，重查不夠，行為與現在相同（失敗並快取），沒有變壞；若量測顯示窗口遠比估計短，次數可以更少。任務 1.1 處理。
- **真正壞掉的 store 多等最多約 100 ms**：見上，只發生在每秒一次的重開。
- **在鎖內睡眠**：同一 client 的其他執行緒會等。只在 store 已經壞的情況下，可接受。
- 重查只看長度，若 daemon 在重查期間又 commit 並再次讓檔案暫時變短，下一次重查可能又短：這由「仍然短才失敗」涵蓋，最壞情況是與現在一樣失敗一次。

## Migration Plan

沒有資料或設定遷移，也沒有介面變動。回退：換回上一版 libemfor 即可。

## Open Questions

None
