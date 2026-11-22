## 1. Client explain

- [x] 1.1 （design：「Explanation.heartbeat_age_ms 改為 i64，不另開欄位」）實作 requirement「Explain shows the tier and the reason of every backend」與「JSON output is stable and only grows」在 client 層的資料：`Explanation.heartbeat_age_ms`（`clients/rust/src/select.rs`）改為 `i64`，預設值 0；`clients/rust/src/client.rs` 的 explain 以帶符號差（`now - heartbeat`，超出 `i64` 範圍時夾住）填入。驗證：`client.rs` 新增單元測試，heartbeat 領先 60 s 的 store 做 explain 得 `heartbeat_age_ms < -59_000` 且 `stale` 為 true；領先 1 s 得 `stale` 為 false 且年齡為負；過去 heartbeat 的既有 explain 測試不變並通過（`tools/docker-test.sh 'cargo test -p emfor-client'`）。新測試先在只改測試時因型別或數值失敗。

## 2. lookup 輸出

- [x] 2.1 [after: 1.1] （design：「文字輸出依符號分兩種說法」）讓 `crates/emfor-daemon/src/lookup.rs` 的輸出符合 spec：`heartbeat_text` 接受 `i64`，負值寫 `heartbeat <|age|> in the future`，正值與「no heartbeat (the daemon has stopped)」的輸出逐字不變；`stale:` 說明在年齡為負時寫 `stale: the heartbeat is more than <max> ahead of the clock`，否則維持 `older than`；`--json` 的 `store.heartbeat_age_ms` 輸出帶符號整數；`--dns` 模式（同檔的 DNS explain 路徑）改用帶符號年齡但 stale 規則不變（`0` 或過去超過 3 秒才 stale）。驗證：`crates/emfor-daemon/tests/lookup.rs` 新增整合測試，未來 60 s 的 store 的 `--explain` 文字含 `heartbeat 1m00s in the future` 與 `ahead of the clock`、不含 `older than`，`--explain --json` 的 `store.heartbeat_age_ms` 為負且 `store.stale` 為 true，領先 1 s 為 fresh；`--dns A` 對未來 store 的 `store.stale` 為 false；測試帶 `// spectra-scenario: lookup-command / Heartbeat in the future` 與 `... in JSON output` 註解。

## 3. 文件

- [x] 3.1 [after: 2.1] 手冊 `docs/manual/12-operating.md` 與 `docs/manual.zh_TW/12-operating.md` 說明 `--explain` 在時鐘往回調時寫 `heartbeat <age> in the future`、`--json` 的 `store.heartbeat_age_ms` 可為負；`CHANGELOG.md` 的 Unreleased 加一條，寫明 Rust `Explanation.heartbeat_age_ms` 由 `u64` 改為 `i64`。驗證：內容審閱，中英與 spec 用語一致；再跑 `tools/docker-test.sh` 的完整檢查（fmt、clippy、`cargo test --workspace --all-features`）通過。
