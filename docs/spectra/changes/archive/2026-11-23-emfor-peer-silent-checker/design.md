## Context

- 收票的成員 A 在 `quorum.rs` 的 `Quorum` 裡保存每個成員、每個 (service, backend) 最新的 `Kept` 結果；`kept_vote` 以 `FRESH_INTERVALS`（3）倍的送出者 `interval` 判斷新鮮度，digest 不同也回 None。`combine` 在每次本機檢查時用 `assigned` 算出 assigned checker，但算完就丟掉，沒有保存。
- 成員只有在自己是 assigned checker、且自己的檢查結果是 `up` 或 `down` 時才送 `result`（`error` 不送），所以「assigned checker 沒送結果」就是缺票的症狀。`candidates` 已經排除通告 off 標籤的成員。
- 成員存活由 `peers/group.rs` 決定（3 秒窗口），Quorum 沒有「變存活的時間」，也沒有 daemon 啟動時間。
- 每秒一次的 peer tick（`peers/task.rs`）已經在呼叫 `quorum.summary`、並把 `disagreements`、`skipped` 複製到 `PeerStats` 給 metrics 讀；`summary` 的 JSON 直接成為 `meta/peers`，`status --json` 原樣輸出。
- 2026-11-23 的 `tools/peer-mismatch-test.sh` 已經用三個 docker 容器重現缺票的情境，可以直接加斷言。

## Goals / Non-Goals

**Goals:**

- 在缺票的那一台（收票的 A）看得到哪個成員、哪個服務沒有結果，不論原因。
- 不改協定、heartbeat、排名與投票結果。
- 短暫的網路抖動、啟動、成員剛變存活時不誤報。

**Non-Goals:**

- 不診斷原因，也不在缺東西的那一台記數。
- 不新增設定項；不改變 assigned checker 的選法。
- 不把「回 `error` 的檢查」與「沒有這個 backend」區分開（兩者都不送結果）。

## Decisions

### 在本機檢查時記下 assigned checker，在 peer tick 判斷沉默

每次本機檢查之後，`Quorum::watch`（由 `schedule.rs` 在 `Quorum::combine` 之後呼叫，避免改動 `combine` 的簽章）把 (service, backend) 的 watch 記進 `State`：digest、本機的服務 `interval`、assigned checker 中本機以外的成員、記錄時間。每秒一次的 `update_silent(now)` 走過所有 watch。理由：`assigned` 需要的輸入（live、off 標籤）在 `Quorum` 裡就有，不必改別處；`combine` 只在本機檢查時跑，所以判斷放在 tick。替代方案：在 `summary` 裡算，因為會讓純讀取的函式帶副作用（log）而不採用。

### 沉默的定義與門檻

成員 M 對 (service, backend) 沉默，當：M 在最近一次記錄的 assigned checker 中；M 目前存活，且沒有通告離開這個服務；這個服務對 M 沒有 digest 不符；M 對這個 backend 沒有新鮮的結果（同 `kept_vote` 的新鮮度，且 digest 相同）；而且以上從第一次被掃描到的時間起，連續達到 `max(10 秒, 本機 interval × 6)`。watch 的記錄時間若已超過這個窗口（本機不再檢查它，例如服務被移除）就丟掉。

計時從「第一次掃描時 M 是 assigned 而且沒有新鮮結果」開始，不是從啟動或成員變存活開始，所以啟動與成員剛變存活自然有一個窗口的寬限期，不需要另存時間。任何一次新鮮結果、成員不存活、離開服務、不再是 assigned、watch 過期，都清掉計時。

10 秒與 6 倍是估計值，沒有量過：用 `tools/peer-mismatch-test.sh`（`interval = "1s"`）確認，情境 2 的訊號要在切斷後 15 秒內出現，情境 1 在 30 秒內不出現。

### 回 `error` 的檢查不特別處理

`error` 不送結果，收票那一側無法分辨「沒有這個 backend」和「檢查一直 `error`」。兩者對投票的影響相同（少一票），所以顯示成同一種訊號，手冊寫明。

### 輸出

- `Summary` 新增 `silent_checkers`：`{"service","backend","member"}` 的陣列，依 service、backend、member 排序；空的時候不寫出這個欄位（`skip_serializing_if`），所以沒有沉默時 `meta/peers` 與現在完全相同，既有 scenario 不變。
- `emfor status` 文字：對每個顯示中、有沉默成員的服務與成員，在 digest 不符的行之後寫 `peers: no results from <member> for <service> (<n> backends)`，依服務名稱再依成員排序。
- metric `emfor_peer_silent_checkers`（gauge）：目前沉默的 (service, backend, member) 組數；沒有 `[peers]` 時不輸出，和其他 peer 系列相同。
- log：某個 (service, member) 第一次有沉默的 backend 時 `warn` 一次：`member <member> sends no results for service <service> (backend <backend>)`；該 (service, member) 沒有任何沉默的 backend 時 `info` 一次：`member <member> sends results for service <service> again`。其他 backend 的進出不再記。理由：投票變少是維運要處理的事，所以進入用 `warn`；每個 backend 一行在大服務會太吵，所以以 (service, member) 為單位。

## Implementation Contract

**行為：**

- 缺少某個 backend（或服務）的成員，在收票成員上：窗口後 `emfor status` 出現 `peers: no results from <member> for <service> (1 backends)`，`status --json` 的 `peers.silent_checkers` 有一筆，`emfor_peer_silent_checkers` 為 1，log 有一次 `warn`。成員開始送結果後，下一秒內清掉，log 有一次 `info`。
- 三台都正常時沒有這些輸出，也沒有 `silent_checkers` 欄位。
- 已經在 `digest_mismatch` 的成員、通告離開服務的成員、不存活的成員，不算沉默。
- 排名、投票結果、協定、heartbeat 都不變。

**資料形狀：** `meta/peers.silent_checkers`: `[{"service": <name>, "backend": <id>, "member": <ip:port>}]`；欄位選用，缺少當空。status 讀取端對這個欄位 `#[serde(default)]`。

**錯誤與靜默：** 掃描不會失敗；一切時間都由 `now: Instant` 傳入，單元測試可以控制時間。

**驗收：**

- 單元測試（`quorum.rs`）：窗口前不出現、窗口後出現、新鮮結果清掉、digest 不符者排除、離開服務者排除、窗口隨 interval 調整、log 只記一次進入與一次離開。
- `status.rs` 文字行測試與 `meta/peers` JSON 測試（欄位選用）。
- `metrics.rs` 輸出測試。
- `tools/peer-mismatch-test.sh`：情境 2、4 出現訊號，情境 1 沒有，設 `peers = false` 後消失。
- `tools/docker-test.sh` 完整檢查通過。

**範圍：** 範圍內：quorum、status、metrics、stats、peers 的 tick、手冊（英文與 zh_TW）、CHANGELOG、LANGUAGE.md、peer-mismatch 測試腳本。範圍外：協定、heartbeat、排名、設定項、診斷原因、缺東西那一側的計數。

## Risks / Trade-offs

- [門檻 10 秒與 6 倍是估計] -> 用 tools/peer-mismatch-test.sh 量；過短會在短暫掉封包時誤報，過長會延遲通知；視測試結果調整常數。
- [`error` 與缺 backend 無法分辨] -> 手冊寫明；兩者同樣少一票。
- [大服務、多 backend 時 JSON 變長] -> 只列沉默的組合，正常時不出現；文字與 log 以 (service, member) 彙整。
- [啟動後第一個窗口沒有訊號] -> 設計上的寬限期；手冊寫明「約 10 秒或 6 個 interval 後」。

## Open Questions

None
