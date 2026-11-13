## Why

聯防群組第一階段（emfor-peers）只讓成員互相「通知後再各自重查」，每台主機的判斷仍完全來自自己的檢查。後端壞掉時若某台主機因網路路徑或時序沒看到（或看到一半），同一機房的主機會對同一台後端給出不同答案。第二階段依 emfor-peers 的 design.md 已定案的做法，讓每台後端由固定 3 台負責主機的檢查結果做多數決，多數判 down 時全區一致把它判為 down，並讓維運人員從 `emfor status` 看到全區與本機不一致的後端。

## What Changes

- 新增訊息種類 `result`：負責主機每次檢查（結果為 up 或 down）後，送出服務、摘要、後端、結果、原因與自己的 `interval` 給其他成員。第一階段的主機把它當成不認得的種類，只用來判斷存活。
- 負責主機：每台後端在「存活成員加上本機」中以 rendezvous hashing（MD5，同 sticky 的取法）選出 3 台。存活且連同本機少於 3 台時不做多數決。
- 全區判斷：負責主機的新鮮結果（3 個傳送者 `interval` 內收到、摘要相同、傳送者仍存活）中至少 2 票 down 時全區判 down，優先於本機檢查，原因寫成 `peers <down 票數>/<新鮮票數>: <負責主機的原因>`；否則依本機結果，本機 down 時原因寫成 `local: <原因>`。合成後的結果照常進 rise / fall、store、log 與指標。
- store 新增 `meta/peers`（JSON）：成員與存活、摘要不同的成員、全區判 down 但本機檢查為 up 的後端。client 與 DNS 不讀，`meta/schema_version` 維持 1。
- `emfor status` 顯示聯防群組一行（存活成員、摘要不同的服務與成員），後端「全區判 down、本機 up」時加上標記 `local-up`；`--json` 多 `peers` 欄位。結束碼不變。
- `/metrics` 新增 `emfor_peer_results_total{direction}` 與 `emfor_peer_disagreements`。
- emfor-daemon 新增相依 md-5 0.11（Cargo.lock 已有，emfor-check 使用中），不新增套件。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `peer-group`: 新增負責主機與檢查結果交換、全區判斷兩個 requirement；訊息認證 requirement 的範例改用仍不認得的種類。
- `health-checking`: rise / fall 狀態機在有聯防群組時吃合成後的結果。
- `state-store`: 新增 `meta/peers` key。
- `status-command`: 文字輸出加聯防群組一行與 `local-up` 標記，JSON 加 `peers`。
- `metrics-endpoint`: daemon 指標加入結果訊息計數與不一致後端數。

## Impact

- 程式：crates/emfor-daemon/src/quorum.rs（新檔：負責主機、投票表、全區判斷、`meta/peers` 摘要）、crates/emfor-daemon/src/peers.rs（`result` 訊息、收送與每秒發布 `meta/peers`）、crates/emfor-daemon/src/lib.rs、crates/emfor-daemon/src/schedule.rs（檢查後合成結果、送出結果）、crates/emfor-daemon/src/writer.rs（寫 `meta/peers`）、crates/emfor-daemon/src/status.rs、crates/emfor-daemon/src/metrics.rs、crates/emfor-daemon/src/supervisor.rs、crates/emfor-daemon/src/cli.rs、crates/emfor-format/src/lib.rs（`KEY_PEERS` 常數）、crates/emfor-daemon/Cargo.toml。
- 測試：crates/emfor-daemon/tests/peers.rs（三個 daemon 的多數決整合測試）、各模組單元測試。
- 文件：docs/manual/01-concepts.md、docs/manual/03-configuration.md、docs/manual/08-operations.md、docs/deploy/daemon.md、CHANGELOG.md、docs/spectra/config.yaml、docs/spectra/LANGUAGE.md。
- client、DNS、LMDB 服務記錄格式、設定檔語法都不變。
