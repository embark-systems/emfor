## Why

`emfor-check` 的資料庫外掛只能確認「這台是不是 primary」，redis 也只能確認「這台是不是 replica」。複寫斷掉或落後好幾分鐘的 replica 照樣是 `up`，讀寫分離的應用程式拿到它就會讀到舊資料。emfor 要回答的是「這台現在能不能接流量」，所以 replica 也要檢查複寫本身。

## What Changes

- `mysql` 與 `pgsql` 的 `role` 新增 `replica`；redis 原本就有。
- `role = "replica"` 時，三個外掛都檢查複寫是否在進行，沒有就是 `down`：
  - redis：`ROLE` 回覆的連線狀態必須是 `connected`；
  - mysql：`SHOW REPLICA STATUS`（舊版伺服器退回 `SHOW SLAVE STATUS`）必須有資料，且 IO 與 SQL thread 都是 `Yes`；
  - pgsql：`pg_is_in_recovery()` 為 true，且有 WAL receiver。
- 新選項 `max_lag`（正整數秒，需 `role = "replica"`）：落後超過它就是 `down`：
  - redis：`INFO replication` 的 `master_last_io_seconds_ago`；
  - mysql：`Seconds_Behind_Master`／`Seconds_Behind_Source`；
  - pgsql：最後重播交易到現在的時間；已收到的 WAL 都重播完時算 0。
- 新的原因：`role is primary, want replica`、`role is not a replica, want replica`、`replication: ...`。
- 文件（手冊第 4 章中英文、docs/deploy/plugins.md）、examples/emfor.toml、CHANGELOG 與實機測試 tests/plugins/run.sh 一併更新。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `database-checks`：`role` 對 mysql、pgsql 接受 `replica`，新增 `max_lag`；Redis、MySQL、PostgreSQL check 要求 replica 的複寫在進行、且不超過 `max_lag`。

## Impact

- 程式：crates/emfor-check/src/options.rs、crates/emfor-check/src/redis.rs、crates/emfor-check/src/mysql.rs、crates/emfor-check/src/pgsql.rs。
- 測試：crates/emfor-check/src/redis.rs 的單元測試、crates/emfor-check/tests/live.rs、tests/plugins/run.sh、tests/plugins/inside.sh。
- 文件：docs/manual/04-health-checks.md、docs/manual.zh_TW/04-health-checks.md、docs/deploy/plugins.md、examples/emfor.toml、CHANGELOG.md。
- 不新增相依套件；外掛協定、store 格式不變。
- 權限：mysql 的 replica 檢查需要 `REPLICATION CLIENT`（MariaDB 10.5.9 起為 `REPLICA MONITOR`）。
