## Context

`emfor-check` 的 `redis`、`mysql`、`pgsql` 外掛每次檢查都開新連線，登入後做最小查詢（`PING` 或 `SELECT 1`）。設了 `role` 時再多查一次角色：

- redis：`ROLE` 回覆的第一個元素；
- mysql：`@@global.read_only`；
- pgsql：`pg_is_in_recovery()`。

`role` 目前只有 redis 能設 `replica`，而且只比對角色名稱。replica 連不上 master、複寫 thread 停了、或落後很久，仍然是 `up`。

實機測試（tests/plugins/inside.sh）裡，redis 6380 與 PostgreSQL 5433 是真的在複寫的 replica。MariaDB 3307 只是 `--read-only=1` 的獨立伺服器，並沒有在複寫。

## Goals / Non-Goals

**Goals:**

- `role = "replica"` 在三個外掛上的意思都是：這台是 replica，而且正在從上游接收複寫；
- `max_lag` 讓維運人員決定落後多少秒就不給流量；
- 判斷邏輯可以不靠真的伺服器就做表格測試，實機測試只確認查詢本身。

**Non-Goals:**

- MariaDB Galera（`wsrep_*`）與 MariaDB 多來源複寫（只看預設連線）；
- redis Sentinel 與 Cluster 模式；
- 用 primary 的位置計算精確落後量（例如比對 replica 與 master 的 offset）：外掛一次只連一台 backend；
- 依連線數或負載判斷 down。

## Decisions

### replica 角色要求複寫正在進行

只比對角色名稱，抓不到「是 replica 但複寫已斷」這種最需要排除的情況，所以 `role = "replica"` 一併要求複寫正在進行：

- redis：`ROLE` 回覆的第 4 個元素（連線狀態）必須是 `connected`，否則為 `down`，原因 `replication: link to master is <狀態>`；
- mysql：複寫狀態必須有一筆資料，否則為 `role is not a replica, want replica`。IO 與 SQL thread 都必須是 `Yes`，否則為 `replication: IO thread is <值>` 或 `replication: SQL thread is <值>`，`Last_IO_Error`／`Last_SQL_Error` 不是空字串時接在 `: ` 後面。IO 先檢查；
- pgsql：`pg_is_in_recovery()` 為 false 時是 `role is primary, want replica`；`pg_stat_wal_receiver` 沒有資料時是 `replication: no WAL receiver`。

替代方案：另設一個 `replication = true` 選項。否決，因為「是 replica 但不在複寫」幾乎不會是想要的狀態，多一個選項只會讓人忘了開。

### mysql 先用 SHOW REPLICA STATUS，語法錯誤時改用 SHOW SLAVE STATUS

`SHOW REPLICA STATUS` 從 MySQL 8.0.22 與 MariaDB 10.5.1 開始支援；更舊的伺服器只認得 `SHOW SLAVE STATUS`，而新版 MySQL 已移除後者。外掛先送 `SHOW REPLICA STATUS`，只有伺服器回錯誤碼 1064（語法錯誤）時才改送 `SHOW SLAVE STATUS`。欄位兩種名稱都接受：

- `Replica_IO_Running`／`Slave_IO_Running`；
- `Replica_SQL_Running`／`Slave_SQL_Running`；
- `Seconds_Behind_Source`／`Seconds_Behind_Master`。

MariaDB 即使用 `SHOW REPLICA STATUS` 也回 `Slave_*` 欄位名稱。

### max_lag 以整數秒表示，各外掛用自己的落後指標

`max_lag` 是 1 到 86400 的整數秒，只在 `role = "replica"` 時可以設。外掛選項目前沒有時間字串的先例（`timeout` 由 daemon 傳 `timeout_ms`），整數秒最不容易誤解。落後量的來源：

- redis：`INFO replication` 的 `master_last_io_seconds_ago`，是距上次收到 master 資料的秒數。master 預設每 10 秒 ping 一次 replica，所以文件建議 `max_lag` 至少 15；
- mysql：`Seconds_Behind_Source`／`Seconds_Behind_Master`。值為 NULL 時為 `replication: lag unknown`；
- pgsql：已收到的 WAL 都重播完（`pg_last_wal_receive_lsn() = pg_last_wal_replay_lsn()`）時為 0，否則為 `now() - pg_last_xact_replay_timestamp()` 的秒數（以 `float8` 取回）。結果為 NULL 時為 `replication: lag unknown`。

落後秒數取整數（無條件捨去）後大於 `max_lag` 時為 `down`，原因 `replication: <N>s behind, max <M>s`。redis 也用同一個寫法。

替代方案：pgsql 只用 `now() - pg_last_xact_replay_timestamp()`。否決，因為 primary 沒有寫入時這個值會一直變大，閒置的系統會被誤判。

### 判斷與查詢分開以便表格測試

每個外掛把伺服器回來的資料整理成一個小結構（角色、連線或 thread 狀態、錯誤訊息、落後秒數），交給純函式 `replica_outcome(status, max_lag) -> CheckOutcome` 判斷。各種狀態組合都用單元測試的表格覆蓋。實機測試只驗證：查詢能在真的伺服器上取得這些欄位，以及至少一種 `down`。

### pgsql 一次查詢取得全部欄位

一個查詢同時取回：`pg_is_in_recovery()`、`EXISTS (SELECT 1 FROM pg_stat_wal_receiver)`、以及上述落後秒數。這樣只多一次往返，也確保三個值來自同一個時間點。

## Implementation Contract

**行為**

- `role = "replica"`：
  - redis：`ROLE` 第一個元素是 `slave`，且連線狀態是 `connected`；
  - mysql：複寫狀態有一筆資料，IO 與 SQL thread 都是 `Yes`；
  - pgsql：在 recovery 中，且有 WAL receiver。
  
  符合以上條件、設了 `max_lag` 時落後量也不超過它，才是 `up`。
- `role = "primary"`／`"master"` 與不設 `role` 的行為不變。

**選項**

- `role`：redis 接受 `master`、`replica`（錯誤訊息不變）；mysql、pgsql 接受 `primary`、`replica`，其他值的錯誤為 `must be one of primary, replica`；
- `max_lag`：1 到 86400 的整數，否則為 `must be an integer from 1 to 86400`；沒有 `role = "replica"` 時為 `requires role = "replica"`；ldap 為 `unknown key`。

**原因（全部是 `down`）**

- `role is primary, want replica`（pgsql）
- `role is master, want replica`（redis，維持原樣）
- `role is not a replica, want replica`（mysql）
- `replication: link to master is <狀態>`
- `replication: IO thread is <值>[: <錯誤>]`
- `replication: SQL thread is <值>[: <錯誤>]`
- `replication: no WAL receiver`
- `replication: lag unknown`
- `replication: <N>s behind, max <M>s`

查詢被拒（例如權限不足）沿用現有的 `query: ` 前綴。

**驗收**

- crates/emfor-check 的單元測試：選項驗證表、三個外掛的 `replica_outcome` 表；
- tests/plugins/run.sh 的實機測試：redis 6380、MariaDB 3307、PostgreSQL 5433 以 `role = "replica"` 與 `max_lag = 30` 為 `up`；primary 以 `role = "replica"` 為 `down`；各有一台不在複寫的 replica 為 `down`，並帶對應的 `replication: ` 原因；
- macOS 與 Debian 映像的 fmt、clippy、test 全部通過。

**範圍**

- 範圍內：上述三個外掛、選項驗證、文件、範例、CHANGELOG、實機測試環境（MariaDB 真正的複寫，以及不在複寫的 replica）；
- 範圍外：Non-Goals 所列各項、daemon 與外掛協定的變更。

## Risks / Trade-offs

- [非 superuser 看不到 `pg_stat_wal_receiver` 的資料列] → 實機測試以一般帳號 `emfor` 驗證。若看不到，改為文件要求 `pg_monitor` 角色，測試帳號也給這個角色，並在 tasks 記錄結果。
- [redis 的 `master_last_io_seconds_ago` 只代表「多久沒收到 master 的資料」，不是資料落後量] → 文件寫明它的意思，並建議值至少 15 秒。
- [mysql 的 replica 檢查需要額外權限] → 文件列出 `REPLICATION CLIENT`（MariaDB 10.5.9 起為 `REPLICA MONITOR`）；缺權限時原因是伺服器的錯誤，前綴為 `query: `。
- [replica 斷線時所有主機會同時把它排除] → 這正是想要的行為；`failover` 與其他 balance 會改用其他後端，全部不健康時依 `on_all_down` 處理。

## Migration Plan

新增選項與新的 `role` 值都向後相容：現有設定的行為不變。唯一的差別是 redis 的 `role = "replica"`：連線狀態不是 `connected` 的 replica 從 `up` 變成 `down`。CHANGELOG 標明這一點。

## Open Questions

- 非 superuser 能否看到 `pg_stat_wal_receiver` 的資料列：在實機測試中確認（見 Risks）。
