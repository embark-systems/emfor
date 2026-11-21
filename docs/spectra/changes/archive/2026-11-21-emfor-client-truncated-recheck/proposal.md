## Why

daemon 每次 commit 之後才用 `pad()` 把資料檔補到 LMDB meta page 記錄的長度；LMDB 在同一個交易裡配置又釋放頁面時，commit 之後檔案會暫時比記錄的短（`crates/emfor-daemon/src/store.rs` 的 `Store::write`：`wtxn.commit()` 之後才 `pad(&self.env, &self.data)`）。client 在這個短暫的窗口第一次開 store，`check_length`（`clients/rust/src/store.rs`）看到檔案比記錄的短，回 `UNAVAILABLE`（`truncated`），而 `Client::open` 又把這個失敗快取 1 秒（`OPEN_RETRY`）：一次本來不該失敗的開檔，讓這個 client 的查詢在接下來 1 秒都失敗。已經開著 store 的 client 不受影響。這是 Fable review 留下的低嚴重度項目（TODO 51／58 ⑤：觀察，「要處理是 client 遇 truncated 時重試一次」），窗口只有微秒到毫秒，所以先沒修。

## What Changes

- `clients/rust` 開 store 時，若 `check_length` 判定資料檔比 LMDB 記錄的短，不立刻失敗：短暫停頓後重新讀 meta page 與檔案長度，再檢查，最多重查幾次；仍然短才回 `UNAVAILABLE`（訊息與現在相同）並進入 1 秒的失敗快取。
- 停頓的次數與時間先暫定 5 次、每次 20 ms（總共最多約 100 ms），這是估計值：實作的第一個任務先量 daemon 這一側檔案實際維持「比記錄短」的最長時間，再決定數字。
- C、Go、Python、Perl、PHP 的 client 都是 libemfor（`clients/rust` 的 C API）的包裝，沒有自己的開檔邏輯，所以不用各自改。
- 手冊 06 章（中英）加一句，CHANGELOG 記一筆。

## Non-Goals

- 不改 daemon：不改 `pad()` 的時機，也不改 commit 與補長度的順序（那會動到 store 的寫入路徑，風險遠大於這個窗口）。
- 不處理 client 已經開著 store 之後檔案被改短（會 `SIGBUS`，spec 已明說在保證之外）。
- 不改 `OPEN_RETRY` 的 1 秒快取，也不改檔案被換掉時的偵測（`REPLACE_CHECK_MS`）。
- 被否決的做法：把 truncated 一律當成「暫時」而不快取（真的壞掉的 store 會每次查詢都重開並重試，讓每次查詢多 100 ms）；讓 daemon 在 commit 時就保證長度（LMDB 沒有這個保證，要改寫入路徑）。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `client-lookup`: `Lookup failures use four shared error codes` 需求：開 store 時資料檔比 meta page 記錄的短，要先重查幾次，仍然短才以 `UNAVAILABLE` 失敗。

## Impact

- 程式：`clients/rust/src/store.rs`（`check_length`、`Env::open`）。
- 測試：`clients/rust/src/client.rs` 或 `store.rs` 內的單元測試；現有 `truncated_store_is_unavailable` 的行為不變（仍然失敗，只是多等最多約 100 ms）。
- 文件：`docs/manual/06-clients.md` 與 `docs/manual.zh_TW/06-clients.md`、`CHANGELOG.md`；`docs/spectra/specs/client-lookup/spec.md`（透過 delta）。
- 相容性：不改任何介面；真正被截短的 store，失敗時間多最多約 100 ms，而且只在每秒一次的重開時。
