## Why

聯防訊息的時間戳與收訊者的時鐘相差超過 5 秒（任一方向）就會被丟掉，記成 `stale`。目前維運只看得到結果：`emfor_peer_messages_rejected_total{reason="stale"}` 上升，以及每個來源每 60 秒一行的 warn log。這時成員的票已經消失，也看不出是哪個成員、差多少、是我的時鐘錯還是它的錯。手冊第 12 章「After a clock jump」只能叫維運「等 `stale` 計數器停止上升」。

2026-11-23 的 `tools/peer-wan-test.sh` 顯示一般的延遲與丟包不會讓成員離開；時鐘則不同，一旦偏差超過 5 秒，成員就完全不能參與投票，而且在到達 5 秒之前沒有任何徵兆。要在 5 秒之前看到偏差，需要在每則驗證過的訊息上量一次。

量測不必改協定：群組檢查訊息的順序是驗證密鑰、寄件者、`site`，最後才檢查時間差，所以到時間檢查時，時間戳已經可信，連之後被丟的 `stale` 訊息也能量到。

## What Changes

- 每則通過密鑰驗證、寄件者與 `site` 檢查的聯防訊息（包括之後因時間差被丟的 `stale` 訊息），記下「成員的時間戳減本機時鐘」的偏差，單位毫秒，帶正負號（正＝成員比本機快）。每個成員只保留最近一次的量測與量測時間；超過 10 秒沒有任何驗證過的訊息就丟掉這筆量測。
- 偏差的絕對值連續三則訊息都達到 2 秒時，這個成員進入「時鐘偏差」狀態；任何一則訊息的偏差絕對值低於 1.5 秒就離開；成員偏差量測過期時也離開。
- 顯示：
  - metric `emfor_peer_clock_offset_seconds{peer}`（gauge，帶正負號，單位秒）：每個目前有量測的成員一筆，無論是否超過門檻。
  - `emfor status` 的 peers 區段：處於時鐘偏差狀態的成員各一行 `peers: clock of <member> is <n.n> s behind|ahead of this host (messages more than 5 s apart are dropped)`；當量測中的成員至少兩個、而且全部都處於時鐘偏差狀態時，改寫成一行 `peers: clock of this host differs from every member by up to <n.n> s; check this host's clock (messages more than 5 s apart are dropped)`。
  - `emfor status --json` 的 `peers.clock_offsets`：`{"member","offset_ms"}` 的陣列，只列處於時鐘偏差狀態的成員，空的時候不寫出這個欄位。
  - log：成員進入時鐘偏差狀態時 `warn` 一次，離開時 `info` 一次；與既有的 stale 拒絕 warn（每來源每 60 秒一次）並存。
- 手冊更新：第 11 章（stale 說明與 Metrics 表）、第 12 章（After a clock jump、status、metrics）、第 16 章 stale 那一列（英文與 zh_TW）；CHANGELOG；LANGUAGE.md 新詞「時鐘偏差」。
- 驗證：偏差的符號、門檻與遲滯、stale 訊息也被量到、log 只記一次，都用單元測試；另用 docker 撥動一個容器的時鐘，確認 2 秒後出現預警、超過 5 秒後 `stale` 計數器上升（容器不能改時鐘時，只留單元測試並在手冊說明沒有實測）。

## Non-Goals

- 不改協定、不改 5 秒限制、不改 `clients/rust/src/select.rs` 的 `is_stale`、不改成單調時鐘或 boot id。
- 不校正或補償時鐘，不嘗試把偏差從投票或存活判斷中扣除。
- 不新增設定項（2 秒、1.5 秒、連續三則、10 秒都寫死）。
- 不量測單向網路延遲：偏差含單向延遲，機房內是毫秒級，相對於 2 秒門檻可忽略；跨機房的群組不是支援的做法（手冊第 11 章）。
- 不在 client 端或 `emfor lookup` 偵測時鐘；本機 heartbeat 落在未來的情況已有 `emfor status` 的 `heartbeat <age> in the future`。
- 不改變既有 `stale` 拒絕的計數與 warn。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `peer-group`: 新增成員時鐘偏差的量測、門檻、遲滯與 log。
- `status-command`: `emfor status` 文字輸出新增時鐘偏差的行（含全部成員都偏差時指向本機的寫法）。
- `state-store`: `meta/peers` 新增選用欄位 `clock_offsets`。
- `metrics-endpoint`: 新增 `emfor_peer_clock_offset_seconds`。

## Impact

- 程式碼：crates/emfor-daemon/src/peers/group.rs（驗證流程在 `stale` 時也回傳偏差、每個成員的量測與狀態、1 秒 tick 的判斷與 log）、crates/emfor-daemon/src/peers/task.rs（tick 呼叫與寫入 `meta/peers`）、crates/emfor-daemon/src/quorum.rs（`Summary` 的欄位）、crates/emfor-daemon/src/peers/stats.rs 與 crates/emfor-daemon/src/metrics.rs（metric）、crates/emfor-daemon/src/status.rs（文字行與 JSON 讀取）。
- 文件：docs/manual/11-peer-groups.md、12-operating.md、16-troubleshooting.md 與 docs/manual.zh_TW 對應檔、CHANGELOG.md、docs/spectra/LANGUAGE.md。
- 測試：group.rs 單元測試、status.rs 文字行測試、metrics.rs 輸出測試；docker 驗證腳本。
- 相容性：沒有協定變更；舊版 status 讀到新的 `clock_offsets` 會忽略；沒有偏差時 `meta/peers` 與現在完全相同。
