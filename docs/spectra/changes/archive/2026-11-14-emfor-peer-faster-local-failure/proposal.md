## Why

聯防群組裡，不是某台 backend 負責主機的成員，在兩台負責主機都回報 `up` 時照投票結果走，每 5 次排定的檢查才自己查一次（`crates/emfor-daemon/src/schedule.rs` 的 `OWN_CHECK_EVERY = 5`）；自己查到失敗後，再每個 `interval` 查一次，累積 `fall` 次才標成 `unhealthy`。所以只有這台主機看得到的故障（例如這台主機到 backend 的路由或防火牆壞了），這台主機上的應用程式要比沒有聯防群組時晚很久才停止拿到這台 backend。用預設值（`interval` 2 秒、`fall` 3）推算，最慢約 14 秒（第 10 秒才自己查到第一次失敗，再加 2 × 2 秒），沒有聯防群組時約 6 秒。0.3.0 的 CHANGELOG 把它列為已知限制。這些時間是依排程邏輯推算的，不是量測值；任務 3.1 的測試會以暫停的時鐘量出實際的檢查時間點。

## What Changes

- 不是負責主機的成員，每 3 次排定的檢查至少自己查一次，取代每 5 次（連續 2 次由投票決定後，第 3 次一定自己查）。
- 不是負責主機的成員，在至少兩台負責主機回報 `up`、而且沒有兩張 `down` 票時，自己的檢查得到 `down`，就立刻開始一輪快速重查：每 200 ms 查一次、最多 `fall` 次，和收到其他成員 `down` 通知時的快速重查相同，啟動條件也相同（backend 在本機是 `healthy`、沒有正在進行的快速重查、上一輪快速重查結束已超過 `fall × interval`）。開始時以 `info` 層級記錄。
- 推算的效果（預設值）：只有這台非負責主機看得到的故障，最慢約 6.4 秒標成 `unhealthy`（第 6 秒查到第一次失敗，再加 2 × 200 ms），原本約 14 秒。
- 代價：N 台成員時，每台 backend 每個 `interval` 約被檢查 3 + (N − 3) / 3 次，原本約 3 + (N − 3) / 5 次（例如 10 台成員，從約 4.4 次變成約 5.3 次）。這種情況下約 0.6 秒內連續 `fall` 次失敗就會標成 down，對短暫抖動比較敏感，與收到 `down` 通知時的快速重查相同。
- 手冊（第 1、3、8 章，英文與中文）、`docs/deploy/daemon.md`、`docs/spectra/LANGUAGE.md` 的「投票決定」與 `CHANGELOG.md` 改成新的次數與時間。

## Non-Goals

- 負責主機、沒有 `[peers]`、`peers = false` 的服務、少於三台存活成員時的行為不變。
- 不新增設定：自己檢查的頻率不開放調整。
- `rise`、`fall` 的計算、`local:` 與 `peers d/f:` 的 reason、`down` 通知的送出規則不變。本機自己查到的失敗仍照原規則送出 `down` 通知。
- 自己的檢查結果是 `error`（例如 plugin 結束）時不開始快速重查。
- 不考慮的做法：只把頻率改成每 3 次（最慢約 10 秒），或只加快速重查（最慢約 10.4 秒），都只解決一半。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `peer-group`: 「Split checks」改成連續 2 次投票決定後必須自己檢查，並新增「本機失敗時開始快速重查」的規則；對應的 scenario 與例子表一起更新。

## Impact

- 程式碼：crates/emfor-daemon/src/schedule.rs（`OWN_CHECK_EVERY`、`split`、backend 檢查迴圈中快速重查的啟動）、crates/emfor-daemon/src/quorum.rs（`Combined` 回報「區域為 up 時的本機失敗」）。
- 測試：crates/emfor-daemon/src/schedule.rs 的單元測試、crates/emfor-daemon/tests/peers.rs 的 `split_checks`。
- 文件：docs/manual/01-concepts.md、docs/manual.zh_TW/01-concepts.md、docs/manual/03-configuration.md、docs/manual.zh_TW/03-configuration.md、docs/manual/08-operations.md、docs/manual.zh_TW/08-operations.md、docs/deploy/daemon.md、docs/spectra/LANGUAGE.md、CHANGELOG.md。
- 不影響 store 格式、peer 訊息格式、client 與設定檔。
