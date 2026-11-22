## Problem

`emfor lookup <service> --explain`（client 模式）在 heartbeat 比現在晚時，會印出互相矛盾的資訊：年齡顯示 0（`heartbeat 0.0s ago`），`stale` 卻是 true，還寫著 `stale: the heartbeat is older than 3.0s`。自 emfor-future-heartbeat-stale（2026-11-22 封存）起，heartbeat 比讀取端時鐘晚超過 `EMFOR_MAX_STALENESS` 就算 stale，所以這個組合在時鐘往回調之後一定會出現，而它正是操作人員用 `--explain` 來查問題的時候。

重現方式：把 heartbeat 寫成現在加 60 秒，執行 `emfor lookup api.svc.emfor --explain`；`--json` 的 `store.heartbeat_age_ms` 為 0、`store.stale` 為 true。

## Root Cause

`Explanation.heartbeat_age_ms` 是 `u64`（`clients/rust/src/select.rs`），註解寫明「heartbeat 在未來時為 0」；`clients/rust/src/client.rs` 的 explain 用 `saturating_sub` 算它。`stale` 旗標則來自 `is_stale`，已經對稱。文字輸出（`crates/emfor-daemon/src/lookup.rs` 的 `heartbeat_text` 和 `stale:` 那行）也只有「ago」和「older than」兩種寫法。型別與文字都假設 heartbeat 不會在未來。`emfor status` 在上一個 change 已改成帶符號年齡，這裡沒跟上。

## Proposed Solution

讓 `--explain` 與 `emfor status` 一致：

- `Explanation.heartbeat_age_ms` 改為帶符號（`i64`，未來為負數）；client 的 explain 用帶符號的差。
- 文字輸出：年齡為負時寫 `heartbeat <|age|> in the future`；stale 的說明在未來時寫 `stale: the heartbeat is more than <max> ahead of the clock`，過去維持 `older than`。
- `--json`：`store.heartbeat_age_ms` 在未來時為負數；`--dns` 模式共用同一型別，年齡也是帶符號，但 `--dns` 的 stale 規則（只看 `0` 與 3 秒的過去界線）不變。

這是根因修正：拿掉型別與文字裡「heartbeat 不會在未來」的假設。

## Non-Goals

- `--dns` 說明模式的 stale 判斷：它本來就說明「store 視為新鮮」，不動；只有年齡的呈現隨型別變成帶符號。
- store 格式、daemon 寫入端、聯防訊息。
- 新增欄位：`heartbeat_age_ms` 沿用，只放寬值域（與 `emfor status --json` 同）。

## Success Criteria

- client 模式 `--explain` 對「heartbeat 領先 60 秒」的 store 輸出 `heartbeat 1m00s in the future` 與 `stale: the heartbeat is more than 3.0s ahead of the clock`；`--json` 的 `store.heartbeat_age_ms` 為約 -60000，`store.stale` 為 true。
- 過去的 heartbeat（含 3001 ms 老與已停止的 0）輸出與現在完全相同。
- `--dns A` 對未來 heartbeat 仍顯示 fresh（stale false），年齡為負。
- `tools/docker-test.sh` 的 `cargo test --workspace` 通過，新增的測試帶有 spec 情境註解；`spectra validate` 通過。

## Impact

- Affected specs: lookup-command
- Affected code:
  - Modified: clients/rust/src/select.rs, clients/rust/src/client.rs, crates/emfor-daemon/src/lookup.rs, docs/manual/12-operating.md, docs/manual.zh_TW/12-operating.md, CHANGELOG.md
  - New: none
  - Removed: none
