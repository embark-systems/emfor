## Why

聯防群組裡，成員 C 沒有某個 service 或 backend 時，C 仍會排進其他成員的 assigned checker 前 3 名，卻永遠不檢查它、不送結果。收票的成員 A 因此少一票，沒有任何警告：`emfor status` 只看得到 digest 不符，其他情況完全沉默。2026-11-23 的三容器驗證（`tools/peer-mismatch-test.sh`）證實了這一點，手冊第 11 章「When members differ」與第 16 章目前只能寫「Nothing warns about it」。

缺 service、缺 backend、單向掉封包的共同症狀是：「排名裡的 assigned checker 存活、沒有宣告離開這個服務，卻一直沒送這個 backend 的新鮮結果」。這個症狀在收票的成員 A 就能看出來，不需要改協定。

## What Changes

- daemon 在每次自己檢查 backend 時記下它的 assigned checker；每秒的 peer tick 檢查這些成員（本機以外）的最新結果，連續一段時間沒有新鮮結果就視為「沉默的 checker」。
- 判斷門檻：沒有新鮮結果的時間達到 `max(10 秒, 本機 interval × 6)` 才算。成員剛排進 assigned checker 時才開始計時，所以 daemon 啟動和成員剛變存活自然有寬限期。
- 不算：本機沒有 assigned checker 的 backend（少於 3 台候選）、已經有 digest 不符的成員（那個服務已由 digest 不符顯示）。
- 顯示：
  - `emfor status` 文字多一行 `peers: no results from <member> for <service> (<n> backends)`，放在 digest 不符的行之後。
  - `emfor status --json` 的 `peers.silent_checkers`：`{"service","backend","member"}` 的陣列，空的時候不寫出這個欄位。
  - metric `emfor_peer_silent_checkers`（gauge）：目前沉默的（backend、成員）組數。
  - log：某個（服務、成員）第一次出現沉默時 `warn` 一次，全部恢復時 `info` 一次，不重複。
- 手冊更新：第 11 章「When members differ」與第 16 章那一列不再寫「沒有警告」；第 12 章的 status 與 metric 說明；CHANGELOG；LANGUAGE.md 若有新詞。
- `tools/peer-mismatch-test.sh` 加斷言：情境 2 與 4 出現訊號，情境 1 沒有，設 `peers = false` 後消失。

## Non-Goals

- 不改協定、不改 heartbeat、不改排名（assigned checker 的選法不變）。
- 不診斷原因：缺 service、缺 backend、單向掉封包、檢查一直回 `error`（不送結果）都顯示成同一種「沒有結果」。
- 不新增設定項（門檻寫死）；真實環境需要調整時再議。
- 不在被忽略訊息的那一側（缺東西的 C）記數；那一側不是需要處理的人。
- 不因此自動改變投票結果：沉默的 checker 仍然佔一個名額，只是被看見。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `peer-group`: 新增「沉默的 assigned checker」的偵測規則、門檻、例外與 log。
- `status-command`: `emfor status` 文字輸出新增一行 peers 沉默 checker。
- `state-store`: `meta/peers` 新增選用欄位 `silent_checkers`。
- `metrics-endpoint`: 新增 `emfor_peer_silent_checkers`。

## Impact

- 程式碼：crates/emfor-daemon/src/quorum.rs（偵測與 summary）、crates/emfor-daemon/src/peers/task.rs（tick 呼叫掃描、寫入統計）、crates/emfor-daemon/src/peers/stats.rs 與 crates/emfor-daemon/src/metrics.rs（metric）、crates/emfor-daemon/src/status.rs（文字行與 JSON 讀取）。
- 文件：docs/manual/11-peer-groups.md、docs/manual/12-operating.md、docs/manual/16-troubleshooting.md 與 docs/manual.zh_TW 對應檔、CHANGELOG.md、docs/spectra/LANGUAGE.md。
- 測試：quorum.rs 單元測試、status.rs 文字行測試、tools/peer-mismatch-test.sh。
- 相容性：沒有協定變更；舊版 status 讀到新的 `silent_checkers` 會忽略；沒有新欄位時行為與現在相同。
