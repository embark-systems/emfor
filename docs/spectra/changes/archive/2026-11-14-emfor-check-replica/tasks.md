## 1. 選項

- [x] 1.1 Check options validation：`role` 對 mysql、pgsql 接受 `replica`（錯誤訊息 `must be one of primary, replica`），新增 `max_lag`（1–86400 整數秒，需 `role = "replica"`，ldap 為 unknown key），依「max_lag 以整數秒表示，各外掛用自己的落後指標」寫進 crates/emfor-check/src/options.rs 的 `Settings`；以 options.rs 單元測試涵蓋 spec 範例表 option validation 的每一列，並確認拿掉新檢查時新增的列失敗

## 2. 外掛

- [x] 2.1 Redis check：依「replica 角色要求複寫正在進行」與「判斷與查詢分開以便表格測試」，讀 `ROLE` 第 4 個元素的連線狀態，設 `max_lag` 時送 `INFO replication` 取 `master_last_io_seconds_ago`，交給純函式 `replica_outcome` 判斷；以 redis.rs 的單元測試（fake_redis 與 spec 範例表 redis role 每一列）驗證 [after: 1.1]
- [x] 2.2 MySQL check：依「mysql 先用 SHOW REPLICA STATUS，語法錯誤時改用 SHOW SLAVE STATUS」查複寫狀態，兩種欄位名稱都接受，交給 `replica_outcome` 判斷 thread、錯誤訊息與 `max_lag`；以 mysql.rs 單元測試涵蓋 spec 範例表 mysql replica status 每一列 [after: 1.1]
- [x] 2.3 PostgreSQL check：依「pgsql 一次查詢取得全部欄位」以一個查詢取回 recovery、WAL receiver 與落後秒數（`float8`），交給 `replica_outcome` 判斷；以 pgsql.rs 單元測試涵蓋 spec 範例表 pgsql replica status 每一列 [after: 1.1]

## 3. 實機測試

- [x] 3.1 tests/plugins/inside.sh 讓 MariaDB 3307 真正複寫 3306（3306 開 binlog、兩台各自 server-id、3307 `CHANGE MASTER TO` 後 `START SLAVE`，仍保持 read-only），並給 `emfor` 帳號 `REPLICA MONITOR`；新增不在複寫的 replica：redis 6381（replicaof 不存在的 port）、MariaDB 3308（master 指向不存在的 port）、PostgreSQL 5434（primary_conninfo 指向不存在的 port）；以 tests/plugins/run.sh 啟動成功、既有實機測試仍全數通過驗證 [after: 2.1, 2.2, 2.3]
- [x] 3.2 crates/emfor-check/tests/live.rs 新增案例：6380、3307、5433 以 `role = "replica"` 及 `max_lag = 30` 為 up；6379、3306、5432 以 `role = "replica"` 為 down（`role is master, want replica`、`role is not a replica, want replica`、`role is primary, want replica`）；6381、3308、5434 為 down 並帶 `replication: ` 原因（5434 最多重試 5 秒）；確認一般帳號 `emfor` 看得到 `pg_stat_wal_receiver` 的資料列，看不到時依 design 的 Risks 改為要求 `pg_monitor` 並記錄於本任務；以 tests/plugins/run.sh 通過驗證 [after: 3.1]
  - 結果（2026-11-14）：PostgreSQL 15 的一般帳號 `emfor` 看得到 `pg_stat_wal_receiver` 的資料列（5433 以 `role = "replica"` 為 up、5434 為 `replication: no WAL receiver`），不需要 `pg_monitor`。

## 4. 文件與發行說明

- [x] 4.1 docs/deploy/plugins.md 的 Roles and failover 一節加入 replica 的檢查方式、`max_lag` 各外掛的落後來源、建議值（redis 至少 15）、所需權限（redis ACL 的 `ROLE` 與 `INFO`、mysql `REPLICATION CLIENT`／`REPLICA MONITOR`）與所有新原因；docs/manual/04-health-checks.md 與 docs/manual.zh_TW/04-health-checks.md 的 Main options 說明更新；examples/emfor.toml 加一個 replica 服務範例；以 `emfor check-config` 檢查範例設定通過、人工比對文件與 spec 一致驗證 [after: 3.2]
- [x] 4.2 CHANGELOG.md 的未發行段落記錄新功能，並標明 redis `role = "replica"` 現在要求連線狀態為 `connected`；以 macOS 與 Debian 映像的 fmt、clippy、test 全部通過，以及 tests/plugins/run.sh 通過作為整個 change 的最終驗證 [after: 4.1]
