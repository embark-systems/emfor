## Why

聯防群組第二階段（emfor-peers-quorum）讓每台後端由 3 台負責主機的結果做多數決，但群組裡每台主機仍然每個 `interval` 完整檢查每台後端。一個機房有 N 台 emfor daemon 時，每台後端每個 `interval` 被檢查 N 次；N 大時這些檢查本身就成了後端的負擔，而多數決已經只看那 3 台的結果。第三階段依 emfor-peers 的 design.md 規劃，讓非負責主機少做檢查、改用負責主機的結果，同時保留「全區 up 但本機連不上」的偵測。

## What Changes

- 分工檢查：一台後端有負責主機、而本機不是其中之一時（本文件稱「非負責主機」），本機在每次排定的檢查時間先看負責主機的新鮮結果。至少 2 票 down 時不檢查，直接以 `peers <d>/<f>: <原因>` 判 down；至少 2 票 up 時不檢查，直接判 up；其他情況（新鮮結果不足 2 票同向）照常檢查。
- 本機檢查仍會定期做：非負責主機連續 4 次只靠投票決定後，第 5 次一定自己檢查，也就是至少每 `interval × 5` 用服務原本的檢查方式檢查一次（使用者決定：不用 TCP 探測，避免 DNS、RADIUS、SIP 等 UDP 服務被誤判）。本機檢查結果不是 up 時，之後每次都自己檢查，直到本機結果回到 up；合成規則與第二階段相同，所以全區 up 但本機連不上時原因是 `local: <原因>`。
- 退回完整檢查：本機是負責主機、後端沒有負責主機（存活成員連同本機少於 3 台）、快節奏連查期間，每次都自己檢查，與第二階段相同。負責主機依每次檢查當下的存活成員重算，網路分割時兩邊各自在自己看得到的成員中重算。
- 只靠投票決定的那一次不量延遲、不送 `result`、不送 `down` 通知、不改變「全區判 down 但本機 up」的不一致狀態；它的結果照常進 rise / fall、store、state history、log 與檢查計數。
- `/metrics` 新增 `emfor_peer_checks_skipped_total`（counter）：因投票已決定而沒有做的本機檢查次數。
- 沒有 `[peers]`、或群組只有 2 台時行為完全不變；設定檔、store 格式、client、DNS 都不變。

## Non-Goals

- 本機 TCP 連線探測：使用者已選擇沿用服務原本的檢查方式，以 `interval × 5` 的間隔檢查。
- 可設定的分工比例或負責主機數：`× 5` 與 3 台都固定，不開放設定。
- `emfor status` 的新顯示：本階段不改 status 輸出；觀察分工效果用新指標。
- 跨 `site` 的聯防、master 選舉。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `peer-group`: 新增「分工檢查」requirement；故障通知 requirement 註明只靠投票決定的那一次不送 `down`。
- `health-checking`: 檢查排程 requirement 註明在聯防群組中，非負責主機可以只靠投票決定一次檢查。
- `metrics-endpoint`: daemon 指標加入 `emfor_peer_checks_skipped_total`。

## Impact

- 程式：crates/emfor-daemon/src/quorum.rs（只靠投票的判斷）、crates/emfor-daemon/src/schedule.rs（每次排定時間決定要不要自己檢查、本機失敗後的完整檢查）、crates/emfor-daemon/src/peers.rs（略過計數）、crates/emfor-daemon/src/metrics.rs。
- 測試：crates/emfor-daemon/tests/peers.rs（四個 daemon 的分工整合測試）、quorum.rs 與 schedule.rs 單元測試。
- 文件：docs/manual/01-concepts.md、docs/manual/03-configuration.md、docs/manual/08-operations.md、docs/deploy/daemon.md、CHANGELOG.md、docs/spectra/config.yaml、docs/spectra/LANGUAGE.md。
- 不新增相依套件；設定檔語法、LMDB 格式、client、DNS 都不變。
