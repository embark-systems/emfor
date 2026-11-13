## Context

2026-11-13 的全專案審查（Fable）列出的 A2、A3、B2，由使用者決定一起修：

- A2：`crates/emfor-daemon/src/store.rs` 的 `Store::open` 只設 `NO_SUB_DIR`，每次 commit 都 fsync；`writer.rs` 的 `Writer::flush` 直接在 tokio task 裡呼叫 `Sink::commit`。
- A3：`CheckResult::Down(String)` / `Error(String)` 的字串直接進 `BackendHealth`、`BackendUpdate`、`hist/*` 與 log。`exec` 自己截 200 字元（`REASON_CHARS`），plugin 的 reason 原樣採用。`plugin/host.rs` 的 stdout 與 stderr 都用 `BufReader::lines()`，沒有行長上限；stderr 的 log 只顯示前 1000 字元，但整行都先讀進記憶體。
- B2：`schedule.rs` 的 `first_delay` 是 `[0, interval)`，之後一律 `next_wait`（`interval ± jitter`）。

使用者的判斷：emfor 是輔助軟體，真正的服務跑在同一台主機上；主機當機時應用一起停，emfor 的資料不需要跨當機保存。

## Goals / Non-Goals

**Goals:**

- 磁碟延遲不再讓整台主機的 client 判定 store 過期。
- 不 fsync 後，主機斷電造成的 store 損毀由 daemon 自行恢復，不需人工刪檔。
- 任何來源的 reason 都不會把控制字元送進 store、log 或終端機，長度有上限；plugin 的單行輸出不會讓 daemon 記憶體無上限成長。
- daemon 啟動（及新增 backend、狀態回到 unknown）後，backend 在約 0.5 秒內得到確定狀態。

**Non-Goals:**

- 不改 store 格式（`meta/schema_version` 仍為 1），client 不改。
- 不處理 store 在 daemon 執行中損毀（維持既有的 commit 失敗處理與 metric）。
- 不讓 DNS 在 writer 卡住時回 SERVFAIL（審查 A2 的延伸建議，另案）。
- `emfor status` 不另外做輸出跳脫：reason 在寫入前已處理；舊 store 中殘留的 `hist/*` 會在 20 筆轉換後自然汰換。
- 不新增 `fastinter` 之類的設定鍵：快速間隔固定為 `min(interval, 200ms)`。
- client 開啟 store 後檔案被重建時，長駐 client 仍讀舊檔（審查 A8，另案）。

## Decisions

### store 預設以 MDB_NOSYNC 開啟

新增 `[store] sync`（bool，預設 `false`）。`false` 時 `EnvFlags::NO_SYNC`，`true` 時不設。不選 `MDB_NOMETASYNC`：它仍每次 fsync 資料頁，磁碟卡住的問題還在。不選「只寫文件建議 tmpfs」：多數主機的預設路徑 `/var/lib/emfor` 在磁碟上，預設行為就該避開 fsync。`[store]` 的任何變更已規定要重啟，`sync` 沿用。

### commit 在 blocking 執行緒上執行

`Writer` 把 sink 包在 `Option<S>`，`flush` 用 `tokio::task::spawn_blocking(move || { let r = sink.commit(&ops); (sink, ops, r) })` 執行並 await，取回 sink 與 ops（失敗時 ops 仍要進 pending 篩選）。不選 `block_in_place`：單執行緒 runtime（`#[tokio::test]` 預設）會 panic。`clear_stale_readers` 很快，維持在 task 內呼叫。writer 一次仍只有一個 commit，順序不變。

### 損毀判定只看 LMDB 的三種錯誤碼，資料檔改名保留

`Store::open` 在取得 writer lock 之後，把「開 env、讀 default db、清 stale reader」包成一個嘗試；cli 啟動時讀 `hist/*`（`load_history`）以及 writer 的第一次 commit（fresh start）也可能碰到損毀頁。做法：`Store::open` 成功後立刻做一次完整驗證——一個讀交易走訪所有 key（store 很小，預設 64MiB 上限、實際數十 KB，估計；以啟動時間量測確認），遇到 `heed::Error::Mdb(MdbError::Corrupted | PageNotFound | Invalid)` 就關掉 env、改名資料檔為 `<path>.corrupt`（`std::fs::rename`，覆蓋舊的）、刪除 `<path>-lock`、重開一次。重開仍失敗就照原本的錯誤離開（exit 1），不做第二次重建。不直接刪除資料檔：設定指錯路徑時（`MDB_INVALID` 也會出現在非 LMDB 檔），改名可以還原。`MDB_VERSION_MISMATCH`、權限錯誤、`MDB_MAP_FULL` 不算損毀。

### reason 在檢查結果離開檢查器時統一處理

新增 `checker::normalize_reason(&str) -> String`，在 `schedule.rs` 的 `run_check` 回傳後、`health.apply` 之前，把 `CheckResult` 的 reason 換成處理後的字串（`CheckResult::normalized(self) -> CheckResult`）。所有檢查器（tcp、http、exec、plugin，以及 timeout 等內部產生的 reason）都經過這一個出口。上限選 512 字元：tcp `expect` 摘錄最長約 270 字元（`expect: got "` + 64 位元組 × 最多 4 字元跳脫 + `"`），200 會切掉既有格式。跳脫格式沿用 Rust `char::escape_default` 對控制字元的寫法（`\t` `\r` `\n` 與 `\u{1b}`），反斜線不跳脫，避免把 tcp 摘錄已跳脫的 `\x00` 再加一層。`exec` 的 200 字元截斷保留（它是第一行摘錄的規則）。

### plugin 行讀取改用有上限的讀法

新增 `read_line_limited(reader, &mut Vec<u8>, limit) -> LineRead { Line, TooLong, Eof }`：以 `fill_buf` / `consume` 逐段找 `\n`，累積超過 65536 位元組即停止累積；stdout 回 `TooLong` 時 router 記 warn 並結束，supervisor 看到 router 結束（既有的 `Event::StdoutClosed`）就殺掉 plugin、走既有的重啟與 pending 處理；stderr 回 `TooLong` 時繼續 consume 到下一個 `\n` 丟棄，已累積的部分照常記 log（仍顯示前 1000 字元）。握手讀第一行也用同一函式，超長即握手失敗。位元組轉字串用 `String::from_utf8_lossy`。

### unknown 期間的檢查間隔

`schedule.rs` 新增 `fast_interval(interval) = min(interval, 200ms)`；`first_delay` 改為 `[0, fast)`；迴圈結尾依 `health.state() == Unknown && !matches!(result, Error(_))` 選 `fast` 或 `next_wait`。`BackendHealth` 需要公開目前狀態（若尚無 getter 則新增）。

## Implementation Contract

**行為：**

- `[store] sync` 缺省或 `false`：env 帶 `MDB_NOSYNC`；`true`：不帶。`examples/emfor.toml` 與 `docs/deploy/daemon.md` 說明代價（主機斷電後可能損毀，daemon 會改名重建，`hist/*` 遺失）。
- writer 的 commit 不在 tokio worker 上執行；commit 失敗的處理（pending 篩選、`commit_failures`、清 stale reader）不變。
- 啟動時 store 損毀：warn log 含 store 路徑與 LMDB 錯誤名稱；`<path>.corrupt` 為原檔；新 store 正常寫入心跳與每個服務的記錄。第二個 daemon 搶 writer lock 失敗時不碰資料檔。
- 所有 reason 依 health-checking spec「Reasons are bounded and printable」處理。
- plugin stdout 單行超過 65536 位元組：warn log `plugin <name> wrote a line longer than 65536 bytes`，之後同 stdout 關閉；stderr 單行超過：記前段、丟棄其餘，plugin 繼續執行。
- 首次檢查延遲 `[0, min(interval, 200ms))`；unknown 且結果為 up/down 時下次在 `min(interval, 200ms)` 後。

**驗收：**

- 單元測試：`store` 以 `sync` 兩值開啟後檢查 `env.get_flags()`；`normalize_reason` 的範例表；`fast_interval` 與排程選擇的範例表（用 tokio 暫停時鐘）；`read_line_limited` 的正常行、剛好 65536、超過、EOF。
- 整合測試（`crates/emfor-daemon/tests/daemon.rs`）：寫入垃圾 meta page 的 store 啟動後產生 `.corrupt` 並有心跳；`interval = "2s"`、`rise = 2` 的健康 backend 1 秒內 healthy；`exec` 印出含 ESC 的行，store 的 reason 為 `\u{1b}` 形式。
- 整合測試（`crates/emfor-daemon/tests/plugins.rs`）：測試外掛新增送超長 stdout 行與超長 stderr 行的模式，驗證 spec 的兩個情境。
- 既有測試全部通過；macOS 與 Debian 映像 `emfor-clients-test` 的 gate 通過。

**範圍：** 只動 daemon（store、writer、cli 啟動、schedule、checker、plugin host、測試外掛）與文件。不動 client、store 格式、DNS、metrics、`emfor status`。

## Risks / Trade-offs

- [不 fsync 時主機斷電，store 可能損毀但未被三種錯誤碼偵測到（讀到舊但結構完整的頁）] → `svc/*` 啟動時全部重寫，舊值不會被使用；`hist/*` 解析失敗本來就丟棄並記 warn。
- [啟動時走訪整個 store 增加啟動時間] → store 實際只有數十 KB（估計，依服務數），走訪是記憶體映射讀取；以整合測試的啟動時間觀察。
- [unknown 期間 200ms 一次的檢查對大量 backend 造成瞬間負載] → 只在 unknown 且最近結果為 up/down 時發生，最多持續 `max(rise, fall)` 次檢查；error 結果（plugin 未就緒等本機問題）照原本間隔，不會變成每 200ms 重試。
- [改名 `.corrupt` 佔用磁碟] → 只保留一份，最大為 `map_size`。

## Migration Plan

升級後 store 改為不 fsync，不需操作；需要舊行為的主機在 `[store]` 加 `sync = true` 後重啟。CHANGELOG 的 Unreleased 記錄三項變更。回滾：裝回舊版即可，store 格式不變。

## Open Questions

（無）
