## Why

聯防群組第三階段（emfor-peers-split-checks）讓非負責主機靠投票決定大部分檢查時間，但維運人員只能從 `/metrics` 的 `emfor_peer_checks_skipped_total` 看到總數，看不出哪些後端在這台主機上是每次檢查、哪些是靠投票，也看不出某台後端的負責主機是誰。排查「某台主機為什麼較晚發現故障」或「這台後端的判斷來自誰」時需要這些資訊。

## What Changes

- `emfor status` 文字輸出：本機不是某台後端的負責主機時（後端有負責主機，但本機不在其中），該後端那一行加上標記 `voted`。
- `peers:` 行：顯示的後端中有 `voted` 的後端時，行尾加 `, checks <n>/<m> backends here`（`n` 為本機每次都檢查的後端數，`m` 為顯示的後端數）；沒有時不加，現有輸出不變。
- `emfor status --json` 新增欄位 `assigned_checkers`：列出的每台後端的負責主機（依排名），沒有聯防狀態時為 `null`。
- 負責主機由 status 自己計算：讀 `meta/peers` 的本機 `listen` 與存活成員，用與 daemon 相同的函式（`quorum::assigned`）。daemon、store 格式（`meta/peers` 內容）、指標都不變。

## Non-Goals

- 不顯示某次檢查實際是否由投票決定（那取決於當下的票，status 讀不到）；`voted` 只表示「本機不是負責主機」。
- 不改 `meta/peers` 格式、不新增 store key、不改 daemon。
- 不改結束碼、`--watch` 的行為（照常重繪）。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `status-command`: 文字輸出加 `voted` 標記與 `peers:` 行的本機檢查數；JSON 加 `assigned_checkers`。

## Impact

- 程式：crates/emfor-daemon/src/status.rs（解析 `meta/peers` 的 `listen`、計算負責主機、文字與 JSON 輸出）。
- 測試：crates/emfor-daemon/src/status.rs 單元測試。
- 文件：docs/manual/08-operations.md（status 範例與說明）、CHANGELOG.md。
- 不新增相依套件；daemon、store、client、DNS 都不變。
