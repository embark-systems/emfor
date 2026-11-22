## Problem

讀取端把「比自己時鐘還晚的 heartbeat」當成新鮮。主機時鐘往回調超過 `max_staleness`（預設 3 秒）時，如果 daemon 剛好當掉或寫入卡住，heartbeat 會停在「未來」的某個值，client 與 `emfor status` 都會把這份死掉的資料當成新鮮。時間長度是回調幅度加上 `max_staleness`。

重現方式：daemon 把 heartbeat 寫成 T，讀取端時鐘退回 T − 60 秒，daemon 停止寫入。此時 `emfor lookup` 回報 store 新鮮、後端照 `state` 挑選，`emfor status` 的 daemon 行是 `daemon: running`，直到時鐘走回 T + 3 秒（約 63 秒後）才變 stale。

## Root Cause

`clients/rust/src/select.rs` 的 `is_stale` 以 `now_ms.saturating_sub(heartbeat) > max_ms` 判斷，未來的 heartbeat 相減後飽和成 0，永遠不會超過上限。`crates/emfor-daemon/src/status.rs` 的 `daemon_state` 用同樣的 `saturating_sub`。兩處是刻意這樣寫的：client-lookup spec 寫明「A heartbeat later than the reader's clock SHALL count as fresh」，並有範例列 `1700000015000 (in the future) | fresh`。

當初接受這個風險（2026-11-12-emfor-clients 的 design.md，Risks 一節）的理由是「daemon 每秒更新 heartbeat」。這只涵蓋 daemon 還活著的情況；daemon 死掉或卡住時，沒有下一次寫入來把 heartbeat 拉回現在，理由不成立。

## Proposed Solution

讓過期判斷對稱：heartbeat 比讀取端現在時間晚超過 `max_staleness`，一樣算 stale。

- 讀取端（`is_stale`，Rust、C、Go、Python、Perl、PHP 的 client 共用同一份實作）：`heartbeat` 非 0，且 `heartbeat − now > max_staleness` 時為 stale。
- `emfor status`：同樣的界線（3 秒）下 daemon 行為 stale，文字寫成 `daemon: stale, heartbeat <age> in the future`，`--json` 的 `heartbeat_age_ms` 為負數；退出碼同 stale（1）。
- 一致性測試資料加一個「heartbeat 在未來」的案例，所有語言 client 都會跑到。
- 手冊第 3 章與第 16 章（中英）說明時鐘往回調時的行為。

這是根因修正，不是緩解：它直接拿掉「未來的 heartbeat 一律新鮮」這個假設。代價記在 design.md：daemon 活著、時鐘往回調超過 `max_staleness` 時，client 會有約 1 秒（到 daemon 下一次寫 heartbeat 為止）看到 stale。

## Non-Goals

- 單調時鐘或 boot id：已評估並放棄，design.md 說明原因。
- 聯防訊息的 5 秒時鐘限制：只補手冊，另外處理。
- daemon 端偵測時鐘回調並記 warn：不在此變更。
- `emfor lookup --dns` 的說明輸出（它本來就說明「store 視為新鮮」）：不動。

## Success Criteria

- `select.rs` 的 `heartbeat_boundaries` 單元測試中，「未來 5 秒（max 3000 ms）」得 stale，「未來剛好 3000 ms」與「未來 1 秒」得 fresh。
- `status.rs` 的 `daemon_states` 測試中，未來 900 ms 得 running，未來 5000 ms 得 stale 且年齡為 −5000。
- `emfor status` 對未來 5 秒的 heartbeat 印出 `daemon: stale, heartbeat 5.0s in the future`，`--json` 的 `heartbeat_age_ms` 為 −5000，退出碼 1。
- 一致性資料（`tests/conformance/v1/cases.json`）含 `future` 案例，`tools/emfor-fixture/tests/conformance_data.rs` 與各語言 client 的一致性測試通過。
- `spectra validate` 通過；手冊中英兩版都說明時鐘往回調的行為。

## Impact

- Affected specs: client-lookup, status-command
- Affected code:
  - Modified: clients/rust/src/select.rs, clients/rust/src/client.rs, crates/emfor-daemon/src/status.rs, tools/emfor-fixture/src/lib.rs, tools/emfor-fixture/tests/conformance_data.rs, tests/conformance/v1/cases.json, tests/conformance/v1/README.md, docs/manual/03-concepts.md, docs/manual/16-troubleshooting.md, docs/manual.zh_TW/03-concepts.md, docs/manual.zh_TW/16-troubleshooting.md
  - New: none
  - Removed: none
