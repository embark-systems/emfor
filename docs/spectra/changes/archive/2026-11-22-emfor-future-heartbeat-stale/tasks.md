## 1. 讀取端（client）

- [x] 1.1 實作 requirement「Stale heartbeat makes every backend unknown」（design：「界線取 max_staleness，不取 0，也不另設參數」與「讀取端判斷改為對稱的絕對差」）。`clients/rust/src/select.rs` 的 `is_stale` 改為：`heartbeat == 0` 為 stale；否則 `now_ms.abs_diff(heartbeat) > max_ms` 為 stale（過去與未來對稱，未來剛好 `max_ms` 為 fresh）。單元測試 `heartbeat_boundaries` 改寫並加註解 `// spectra-scenario: client-lookup / Heartbeat boundaries / heartbeat values`，列出：未來 3000 ms 為 fresh、未來 3001 ms 為 stale、未來 5000 ms 為 stale，其餘列（3000 ms 老 fresh、3001 ms 老 stale、`0` stale）不變；驗證：`tools/docker-test.sh` 內 `cargo test -p emfor-client heartbeat_boundaries` 通過，且改寫前（只改測試）該測試對「未來 5000 ms」的斷言先失敗。
- [x] 1.2 [after: 1.1] 新增 client 層情境測試：以 `Client` 對一份 heartbeat 為「現在 + 60 秒」、服務 `on_all_down = "closed"`、全部 backend `healthy` 的 store 做 lookup，回 `ALL_DOWN`；同一份 store 的服務改為 `on_all_down = "open"` 時回其中一個 backend。測試加註解 `// spectra-scenario: client-lookup / Future heartbeat of a stopped daemon`；驗證：該測試在 `cargo test -p emfor-client` 內通過。

## 2. 一致性資料與各語言 client

- [x] 2.1 [after: 1.1] `tools/emfor-fixture/src/lib.rs` 的 `heartbeat_value` 新增形式 `future`（值為 `now_ms + 2 * max_staleness_ms`）；`tests/conformance/v1/README.md` 的 `store.heartbeat` 說明加入 `future`；`tools/emfor-fixture/tests/conformance_data.rs` 的形式集合斷言加入 `future`。驗證：`cargo test -p emfor-fixture` 通過，且未知形式仍回錯誤。
- [x] 2.2 [after: 2.1] `tests/conformance/v1/cases.json` 新增案例 `heartbeat in the future`：以現有 `stale` 案例（`on_all_down = "closed"`、全部 healthy）為底，`store.heartbeat` 為 `future`，預期 `ALL_DOWN`；再加一個 `on_all_down = "open"` 的版本，預期 `expect_one_of` 該服務的 backend。驗證：`tools/docker-test.sh` 跑 Rust、C、Go、Python、Perl、PHP 各 client 的一致性測試全部通過（它們共用同一份 `is_stale`，不需要改各語言程式）。

## 3. emfor status

- [x] 3.1 實作 requirements「Status text output」與「Status filtering and JSON output」（design：「emfor status 用同一條界線，並顯示「in the future」」）。`crates/emfor-daemon/src/status.rs` 的 `daemon_state` 回傳 `(Daemon, Option<i64>)`：年齡為帶符號毫秒（未來為負數）；`heartbeat == 0` 為 Stopped；`|age| > STALE_AFTER_MS` 為 Stale，否則 Running。文字輸出：年齡為負時把 `heartbeat <age> ago` 改為 `heartbeat <|age|> in the future`；`--json` 的 `heartbeat_age_ms` 為該帶符號整數。更新所有呼叫端（text、json、`--watch` 的狀態變化、退出碼）使其型別通過。驗證：`cargo test -p emfor-daemon daemon_states`（`NOW + 900` → `(Running, Some(-900))`、`NOW + 3000` → Running、`NOW + 5000` → `(Stale, Some(-5000))`，原有過去列不變）。
- [x] 3.2 [after: 3.1] 加輸出層測試並標註：文字測試對 `heartbeat 900 ms / 3000 ms / 5000 ms in the future` 得 `daemon: running, heartbeat 0.9s in the future`、`daemon: running, heartbeat 3.0s in the future`、`daemon: stale, heartbeat 5.0s in the future`（註解 `// spectra-scenario: status-command / Heartbeat in the future`）；JSON 測試對未來 5000 ms 得 `daemon == "stale"`、`heartbeat_age_ms == -5000`、`heartbeat_ms` 為原值、退出碼 1（註解 `// spectra-scenario: status-command / Heartbeat in the future in JSON output`）。驗證：`cargo test -p emfor-daemon` 內這些測試通過。

## 4. 文件

- [x] 4.1 `docs/manual/03-concepts.md` 的「The state store and the heartbeat」一節加一段：heartbeat 比 client 時鐘晚超過 `EMFOR_MAX_STALENESS` 也算 stale（時鐘往回調時，daemon 還活著約 1 秒內會看到 stale，之後恢復）；`docs/manual/16-troubleshooting.md` 的「The store looks stale while the daemon runs」一節把「The host's clock jumped」改成說明兩個方向都會造成 stale、`emfor status` 會寫 `in the future`；`docs/manual.zh_TW/03-concepts.md` 與 `docs/manual.zh_TW/16-troubleshooting.md` 對應修改，用語依記憶中「繁中用語要自然」的規則。驗證：內容審閱（中英對照，連結與標題不變），並跑 `tools/docker-test.sh` 中檢查手冊連結與 clients 版本的測試（若有）。
- [x] 4.2 [after: 3.1] `docs/manual/12-operating.md` 與 `docs/manual.zh_TW/12-operating.md` 中描述 `emfor status` 輸出與 `--json` 的段落，說明 `daemon: ... in the future` 與 `heartbeat_age_ms` 可為負數；`CHANGELOG.md` 的未發布段加一條。驗證：內容審閱，確認三處用語一致，並確認 `heartbeat_age_ms` 的描述與 status-command spec 的 JSON 要求相同。
- [x] 4.3 [after: 1.1, 3.1] 確認舊 spec 與設計文件對「未來 heartbeat 為 fresh」的敘述已不存在於現行文件（`docs/spectra/specs/`、`docs/manual*`、`clients/*/README*`、`docs/deploy/`）；封存的舊變更（`docs/spectra/changes/archive/`）不改。驗證：`grep -rni "later than the reader\|in the future" docs clients --include=*.md` 的結果每一筆都已對齊新規則。
