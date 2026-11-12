## 1. 歷史格式與 store 讀取

- [x] 1.1 歷史型別放在 emfor-format，並依「歷史存在獨立的 hist key」使用 `hist/<service>`，不改 `svc/` 記錄：在 `crates/emfor-format/src/lib.rs` 新增 `HIST_KEY_PREFIX`、`hist_key`、`HISTORY_LIMIT = 20`、`HistoryEntry`、`ServiceHistory`，以及 `ServiceHistory::push`（超過 20 筆丟最舊）與 `ServiceHistory::retain`（只留給定後端，空的後端從 `backends` 移除）。完成時序列化形狀符合 spec「Backend state history」的 `{"backends":{"<id>":[{"at","from","to","reason"}]}}`；以 emfor-format 單元測試驗證 JSON 形狀、「cap at 20」Example（`at` 1–20 加 21 後為 2–21）與 `retain` 結果。
- [x] 1.2 新增 `Store::prefix(prefix)`，在一個讀取交易內回傳該前綴下所有 key 與值；以 `crates/emfor-daemon/src/store.rs` 的單元測試驗證只回傳 `hist/` 開頭的 key。

## 2. daemon 寫入歷史

- [x] 2.1 [after: 1.1] 歷史與服務記錄同一交易寫入：writer 在 `update` 遇到 `transition` 時把 entry（`at` = `checked_at`、`reason` = 檢查後的 reason）加到該後端，並在 `flush` 中與 dirty 的 `svc/` 記錄一起輸出 `hist/` 的 put（歷史為空時 delete）；commit 失敗時 `pending` 丟掉 `hist/` 的 op、保留 hist dirty，下次 heartbeat 以當時記憶體重寫。reset（`ServiceLayout.reset`）不產生 entry。以 `crates/emfor-daemon/src/writer.rs` 單元測試驗證：一次轉換在同一次 commit 同時寫 `svc/` 與 `hist/`（spec「Transition recorded with its record」）、沒有轉換的更新不寫 `hist/`、reset 不增加 entry、commit 失敗後重試寫入最新歷史。
- [x] 2.2 [after: 2.1] 啟動時由 cli 讀出既有歷史交給 writer，並落實「Fresh state on every start」的例外：`writer::spawn_live` 多一個初始歷史參數；`set_layout(fresh_start = true)` 以 `Op::DeletePrefix("hist/")` 加上修剪後的 put 寫回，只留設定中的服務與後端；reload 時刪除移除服務的 `hist/` key、修剪移除的後端。以 writer 單元測試驗證 spec「History pruned on start and reload」與「History kept across a restart」（啟動後 `svc/` 全為 `unknown`、`hist/` 保留）。
- [x] 2.3 [after: 1.2, 2.2] `crates/emfor-daemon/src/cli.rs` 在 `Store::open` 之後以 `Store::prefix("hist/")` 讀出並解析歷史，無法解析的值以 warning 記下 key 後丟棄，再傳給 `writer::spawn_live`。以 `crates/emfor-daemon/tests/daemon.rs` 整合測試驗證：先寫入 `hist/api.svc.emfor` = `not json` 再啟動 daemon，log 含該 key 的 warning 且 daemon 正常運作（spec「Unparseable history discarded」）。

## 3. emfor status 顯示歷史

- [x] 3.1 [after: 1.1] 狀態讀取一併讀歷史：`Snapshot` 新增 `history`，`Reader::read` 在同一讀取交易讀 `hist/*`，無法解析的值當作不存在；`filter` 依 `--service` 同時篩選。以 `crates/emfor-daemon/src/status.rs` 單元測試驗證讀到的歷史與篩選結果，以及損壞的 `hist/` 不使讀取失敗。
- [x] 3.2 [after: 3.1] 抖動由 emfor status 讀取時計算，實作 spec「Flapping backends」：`flap_count` 只算 healthy↔unhealthy 且 `now - at <= 600000` 的 entry，達 5 次的後端在文字輸出加 `FLAPPING (<n> changes in 10m)`（`pool` 之後、`reason` 之前），結束碼不變。以單元測試驗證「counting rule」Example 的五列，以及「Flapping does not change the exit status」。
- [x] 3.3 [after: 3.2] 實作 spec「Status filtering and JSON output」新增的 `history` 與 `flapping` 欄位，受 `--service` 篩選，沒有歷史時為 `{}` 與 `[]`。以單元測試驗證「JSON includes history and flapping」與「JSON without history」。
- [x] 3.4 [after: 3.3] 實作 spec「Status history output」：`StatusArgs.history`（`--history`，與 `--json`、`--watch` 互斥），輸出 `store:`、daemon 行，再依服務與後端列出 entry（新的在上，`<YYYY-MM-DD HH:MM:SS> <from> -> <to>`，有 reason 時加兩空格與 reason，沒有 entry 印 `no changes recorded`）。以單元測試驗證「History of one service」的輸出，以整合測試驗證 `emfor status --history --watch` 結束碼 2（「History with watch refused」）。
- [x] 3.5 [after: 3.4] watch 的變化清單由歷史與 daemon 變化合併，實作修改後的 spec「Status watch mode」：`Watch` 只記錄 daemon 狀態變化，每幀把 snapshot 歷史與 daemon 變化依時間合併、新的在上取 10 筆，非今天的加 `YYYY-MM-DD ` 前綴；高亮規則不變。以單元測試驗證「merged change list」Example 與「Earlier changes shown at start」，並調整既有 `watch_marks_changes_and_lists_transitions`、`watch_keeps_the_last_ten_changes` 使其以歷史為來源。

## 4. 整合測試、文件與 gate

- [x] 4.1 [after: 2.3, 3.5] 在 `crates/emfor-daemon/tests/daemon.rs` 新增整合測試：daemon 經歷一次轉換後停止再啟動，`hist/` 仍在且 `emfor status --history` 列出該轉換；`emfor status --json` 含 `history`。實測一個 10 台後端、每台 20 筆的 `hist/` 值長度，記錄在 PR 說明，以確認 design 中「每筆約 100 位元組」的估計。
- [x] 4.2 [after: 4.1] 更新 `docs/deploy/daemon.md`（`hist/` key、保留與修剪規則、`--history`、`FLAPPING` 標示、JSON 新欄位）與 `docs/spectra/config.yaml` 的狀態指令說明（`--watch` 清單改含 store 歷史）；以人工檢閱確認文件與 spec 一致。
- [x] 4.3 [after: 4.2] 本機 gate 全數通過：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`，以及 Debian 映像 `emfor-clients-test` 中的同一組指令。
