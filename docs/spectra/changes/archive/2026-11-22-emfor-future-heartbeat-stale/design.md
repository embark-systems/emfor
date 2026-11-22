## Context

daemon 每秒把 `meta/heartbeat`（Unix 毫秒）寫進 store；乾淨關閉時寫 `0`。讀取端拿自己的時鐘與它比較：

- `clients/rust/src/select.rs` 的 `is_stale(heartbeat, now_ms, max_ms)`：`heartbeat == 0 || now_ms.saturating_sub(heartbeat) > max_ms`。`clients/rust/src/client.rs` 的 lookup 與 explain 都呼叫它；libemfor 的 C API 與 Go、Python、Perl、PHP 的 wrapper 都包同一個實作，沒有各語言自己的判斷。
- `crates/emfor-daemon/src/status.rs` 的 `daemon_state(heartbeat_ms, now_ms)`：用 `saturating_sub` 算年齡，超過 `STALE_AFTER_MS`（3000）為 stale；`emfor status` 的文字、`--json`、`--watch` 與退出碼都由它決定。

兩處都把未來的 heartbeat 年齡飽和成 0，因此未來的 heartbeat 永遠新鮮。client-lookup spec 把它寫成明確要求，2026-11-12-emfor-clients 的 design.md 在 Risks 裡以「daemon 每秒更新 heartbeat」為由接受。這個理由只在 daemon 活著時成立：daemon 死掉或寫入卡住時，heartbeat 停在未來，讀取端會把死掉的資料當成新鮮，時間為「回調幅度 + max_staleness」。

此變更是從 TODO 73（封閉環境的檢討）的時鐘問題討論得出的決定：選 B（界線 = `max_staleness`，預設 3 秒）。

## Goals / Non-Goals

**Goals:**

- 讓過期判斷對稱：heartbeat 比讀取端現在晚超過 `max_staleness`，同樣算 stale；所有語言的 client 與 `emfor status` 一致。
- 讓 `emfor status` 把這種情況顯示出來（文字與 `--json` 都看得出 heartbeat 在未來），而不是印成 `heartbeat 0.0s ago`。
- 用一致性資料讓每個語言的 client 都測到這個案例。
- 手冊說明時鐘往回調時的行為。

**Non-Goals:**

- 單調時鐘或 boot id。heartbeat 要讓另一個行程（client、`emfor status`）讀，單調時鐘值跨行程不可比；boot id 要多一個 store 欄位與格式變更（schema 版本），而且 daemon 重啟後仍要靠 wall clock 比較，擋不住「daemon 沒重啟、時鐘被調回」這個情境。成本高於收益，已放棄。
- 聯防訊息的 5 秒時鐘限制（`MAX_SKEW_MS`）：只補手冊，另做，不在此變更。
- daemon 端偵測時鐘回調並記 warn。
- `emfor lookup --dns` 說明模式：它本來就聲明「store 視為新鮮」，不動。
- `emfor lookup --explain`（client 模式）的 `heartbeat_age_ms`：維持目前「未來時年齡顯示 0」，只有 `stale` 旗標會因共用 `is_stale` 而變。見 Risks。

## Decisions

### 界線取 max_staleness，不取 0，也不另設參數

未來的 heartbeat 超過 `max_staleness` 才算 stale。理由：

- 與「過期」對稱：「3000 ms 老」是 fresh、「3001 ms 老」是 stale；未來也是 3000 ms 內 fresh、超過才 stale。一條規則（`|now − heartbeat| ≤ max` 為 fresh）就能說明。
- 容許小幅時鐘修正：NTP slew 或 step 的回調通常是毫秒到數百毫秒。界線取 0 會讓每一次小調整都讓 client 有一小段 stale；取 `max_staleness` 則完全不受影響。
- 不另設參數：`EMFOR_MAX_STALENESS` 已是使用者調整敏感度的地方，少一個旋鈕。

替代方案：界線 0（任何未來都 stale；小幅調整會誤判）；固定 1 秒（多一個數字要解釋，與過期不對稱）。

### emfor status 用同一條界線，並顯示「in the future」

`daemon_state` 的年齡改成帶符號（`i64`，負數 = 在未來），`Stale` 的條件變成 `|age| > STALE_AFTER_MS`（`heartbeat == 0` 仍是 Stopped）。年齡為負時文字寫 `heartbeat <絕對值> in the future`（例如 `daemon: stale, heartbeat 5.0s in the future`），`--json` 的 `heartbeat_age_ms` 為負數。未來但在界線內時，狀態仍是 `running`，文字同樣寫 `in the future`（`daemon: running, heartbeat 0.9s in the future, up …`），因為印 `0.0s ago` 會隱瞞時鐘剛回調。

這是原本討論沒有細談的呈現方式，是有意識的選擇：負的 `heartbeat_age_ms` 是欄位值域的擴充（原本為 `null` 或非負整數）。依 status-command spec 的「Later releases SHALL add fields only」精神，這是對值域的放寬而非移除欄位；讀這個欄位的自動化若假設非負，需要知道這點（手冊與 CHANGELOG 要寫）。替代方案：JSON 維持夾在 0（簡單但與文字不一致，且讓「stale 但年齡 0」難以解釋）。

### 讀取端判斷改為對稱的絕對差

`is_stale` 改成：`heartbeat == 0` 為 stale；否則 `now_ms.abs_diff(heartbeat) > max_ms` 為 stale。`abs_diff` 對 `u64` 不會溢位（不再需要 `saturating_sub`），也不需要分支判斷方向。`max_ms` 為 0（`EMFOR_MAX_STALENESS=0s`）時，任何非零差都 stale，與現在對過去的行為一致。

## Implementation Contract

**行為（使用者觀察得到的）**

- 讀取端：heartbeat 比讀取端時鐘晚 ≤ `max_staleness` → fresh；晚 > `max_staleness` → stale（每個 backend 都不算 healthy，`on_all_down` 決定結果，沒有新的錯誤碼）。過去方向的規則、`0` 為 stale 的規則不變。
- `emfor status`：同樣以 3 秒為界。未來 > 3 秒 → `daemon: stale, heartbeat <age> in the future`，退出碼 1；未來 ≤ 3 秒 → `daemon: running, heartbeat <age> in the future, …`，退出碼依其他條件（同 running）。`--watch` 的 `running -> stale` 變化行照常出現。
- `emfor status --json`：`daemon` 為 `stale`；`heartbeat_age_ms` 為負整數（毫秒）；`heartbeat_ms` 仍是 store 裡的原值。

**介面與資料形狀**

- `clients/rust/src/select.rs`：`pub fn is_stale(heartbeat: u64, now_ms: u64, max_ms: u64) -> bool`，簽名不變。
- `crates/emfor-daemon/src/status.rs`：`daemon_state(heartbeat_ms: u64, now_ms: u64) -> (Daemon, Option<i64>)`（年齡由 `u64` 改為 `i64`；`None` 仍表示 Stopped）。
- 一致性資料（`tests/conformance/v1/cases.json`）的 `store.heartbeat` 多一個形式 `future`：值為「現在 + 2 × max_staleness」（與 `stale` 形式鏡像）；`tools/emfor-fixture/src/lib.rs` 的 `heartbeat_value` 處理它，README 與 `conformance_data.rs` 的形式集合一併更新。

**失敗模式**

- store 裡的 heartbeat 不是數字時的行為不變（現狀由 store 讀取決定）。
- 時鐘回調超過 `max_staleness`、daemon 仍活著：client 與 `emfor status` 看到 stale，直到 daemon 下一次寫 heartbeat（最多約 1 秒）。這是接受的代價，見 Risks。

**驗收標準**

- `cargo test -p emfor-client`（`select.rs` 的 `heartbeat_boundaries` 單元測試）：now = 1700000010000、max = 3000 時，heartbeat `1700000013000`（未來 3000 ms）為 fresh，`1700000013001`（未來 3001 ms）與 `1700000015000` 為 stale。
- `cargo test -p emfor-daemon`（`status.rs` 的 `daemon_states`）：`daemon_state(NOW + 900, NOW) == (Running, Some(-900))`、`daemon_state(NOW + 3000, NOW)` 為 Running、`daemon_state(NOW + 5000, NOW) == (Stale, Some(-5000))`；文字與 JSON 測試各補一筆未來的案例。
- 一致性資料含 `future` 案例，`tools/emfor-fixture` 的 `conformance_data` 測試與各語言一致性測試通過（所有測試依專案規定經 `tools/docker-test.sh` 在 Debian Docker 內跑）。
- `docs/manual/03-concepts.md`、`docs/manual/16-troubleshooting.md` 與 `docs/manual.zh_TW/` 對應檔寫出新規則；`spectra validate` 通過。

**範圍**

- 範圍內：`is_stale`、`daemon_state` 與它的文字／JSON 輸出、一致性資料與其載入程式碼、上列手冊章節（中英）、兩份 delta spec。
- 範圍外：Non-Goals 列的各項、store 格式、daemon 寫入端、peer 訊息的時鐘檢查、`--dns` 說明模式。

## Risks / Trade-offs

- **daemon 活著但時鐘往回調超過 `max_staleness`：client 約 1 秒內看到 stale。** daemon 每秒寫一次 heartbeat，下一次寫入用新的（較早的）時鐘，heartbeat 隨即回到界線內。這 1 秒內 `closed` 服務回 `ALL_DOWN`、`open` 服務改用全部後端。相對的好處是：daemon 死掉或卡住時，過期偵測不再被回調幅度拖長（原本是回調幅度 + `max_staleness`）。時鐘回調超過 3 秒本身是異常事件（正常 NTP 不會這樣），取這個代價換掉「死掉的 daemon 看起來還活著」。這與 2026-11-12-emfor-clients design.md 當初接受風險的取捨相反，這個變更取代它，那段 Risks 對死掉的 daemon 不成立。
- **`emfor lookup --explain`（client 模式）在未來 heartbeat 時顯示「年齡 0 但 stale」。** `client.rs` 的 explain 以 `saturating_sub` 算 `heartbeat_age_ms`，`stale` 旗標則來自 `is_stale`，兩者會互相看起來矛盾。這是已知的呈現缺口，不在本次範圍，留作後續（若要改，需要動 lookup-command spec 的 `--explain` 與 `--json` 要求）。
- **`heartbeat_age_ms` 出現負值。** 讀它的外部自動化若假設非負，會看到負數。只在時鐘回調時才會發生，且那時 `daemon` 欄位本來就會是 `stale` 或可疑；手冊與 CHANGELOG 要寫明。
- **數字標示：** 「約 1 秒」是由 daemon 每秒寫一次 heartbeat 推得（estimated），可用 `emfor status --watch` 搭配手動回調時鐘量測確認；決定本身（界線取 `max_staleness`）不依賴這個數字的精確值。

## Open Questions

- `emfor lookup --explain` 的年齡顯示是否要同樣改成帶符號？本次先不做，留給後續決定。
