## MODIFIED Requirements

### Requirement: Check options validation

The `redis`, `mysql`, `pgsql` and `ldap` plugins SHALL each validate `check_options` with the keys below and SHALL report every other key as `unknown key`. `user` SHALL be required for `mysql` and `pgsql`; `database` SHALL be required for `pgsql` and SHALL be an unknown key for `redis`, `mysql` and `ldap`. `role` SHALL accept `master` or `replica` for `redis` and `primary` or `replica` for `mysql` and `pgsql`, and SHALL be an unknown key for `ldap`. `max_lag` SHALL be an integer from 1 to 86400 (seconds), SHALL be an error (`requires role = "replica"`) unless `role = "replica"`, and SHALL be an unknown key for `ldap`. `starttls` SHALL be a boolean accepted by `ldap` only and SHALL be an unknown key for the other plugins. `password_file` and `ca_file` SHALL be absolute paths of readable files; `password_file` SHALL be non-empty after removing one trailing `\n` or `\r\n`; `ca_file` SHALL contain at least one PEM certificate. For `redis`, `mysql` and `pgsql`, `tls_verify`, `server_name` and `ca_file` SHALL be errors (`requires tls = true`) unless `tls = true`. For `ldap`, they SHALL be errors (`requires tls = true or starttls = true`) unless `tls = true` or `starttls = true`, and `tls = true` together with `starttls = true` SHALL be an error on `starttls` (`conflicts with tls`). For `redis`, `user` without `password_file` SHALL be an error on `user` (`requires password_file`). For `ldap`, `user` without `password_file` SHALL be an error on `user` (`requires password_file`), and `password_file` without `user` SHALL be an error on `password_file` (`requires user`).

#### Scenario: Valid options

- **WHEN** a service with `check = "pgsql"` sets `check_options = { user = "emfor", database = "app", role = "primary" }`
- **THEN** the plugin reports no errors and the config is accepted

#### Scenario: Valid replica options

- **WHEN** a service with `check = "mysql"` sets `check_options = { user = "emfor", role = "replica", max_lag = 30 }`
- **THEN** the plugin reports no errors and the config is accepted

##### Example: option validation

| Plugin | `check_options` | Errors |
| --- | --- | --- |
| mysql | `{}` | `user`: missing required key |
| pgsql | `{ user = "emfor" }` | `database`: missing required key |
| redis | `{ database = "0" }` | `database`: unknown key |
| redis | `{ role = "primary" }` | `role`: must be one of master, replica |
| mysql | `{ user = "emfor", role = "master" }` | `role`: must be one of primary, replica |
| pgsql | `{ user = "emfor", database = "app", role = "standby" }` | `role`: must be one of primary, replica |
| redis | `{ role = "replica", max_lag = 15 }` | none |
| pgsql | `{ user = "emfor", database = "app", role = "replica", max_lag = 86400 }` | none |
| mysql | `{ user = "emfor", max_lag = 30 }` | `max_lag`: requires role = "replica" |
| mysql | `{ user = "emfor", role = "primary", max_lag = 30 }` | `max_lag`: requires role = "replica" |
| redis | `{ role = "replica", max_lag = 0 }` | `max_lag`: must be an integer from 1 to 86400 |
| redis | `{ role = "replica", max_lag = 86401 }` | `max_lag`: must be an integer from 1 to 86400 |
| redis | `{ role = "replica", max_lag = "30s" }` | `max_lag`: must be an integer from 1 to 86400 |
| ldap | `{ max_lag = 30 }` | `max_lag`: unknown key |
| redis | `{ user = "emfor" }` | `user`: requires password_file |
| redis | `{ password_file = "redis.pass" }` | `password_file`: not an absolute path |
| redis | `{ password_file = "/etc/emfor/missing" }` | `password_file`: file not found |
| redis | `{ password_file = "/etc/emfor/empty" }` (file holds only `\n`) | `password_file`: file is empty |
| redis | `{ server_name = "redis.internal" }` | `server_name`: requires tls = true |
| redis | `{ tls = "yes" }` | `tls`: must be a boolean |
| redis | `{ tls = true, ca_file = "/etc/emfor/not-pem.txt" }` | `ca_file`: no certificate found |
| redis | `{ timeout = 1 }` | `timeout`: unknown key |
| redis | `{ starttls = true }` | `starttls`: unknown key |
| ldap | `{}` | none (anonymous bind) |
| ldap | `{ user = "cn=monitor,dc=example,dc=org" }` | `user`: requires password_file |
| ldap | `{ password_file = "/etc/emfor/ldap.pass" }` | `password_file`: requires user |
| ldap | `{ tls = true, starttls = true }` | `starttls`: conflicts with tls |
| ldap | `{ server_name = "ldap.internal" }` | `server_name`: requires tls = true or starttls = true |
| ldap | `{ starttls = "yes" }` | `starttls`: must be a boolean |
| ldap | `{ role = "primary" }` | `role`: unknown key |
| ldap | `{ database = "dc=example" }` | `database`: unknown key |

#### Scenario: Config check reports plugin errors

- **WHEN** `emfor check-config` runs on a config whose `mysql` service has `check_options = {}`
- **THEN** it fails with `services."<name>".check_options.user: missing required key`

### Requirement: Redis check

The `redis` plugin SHALL connect to the backend, send `AUTH <user> <password>` when `user` is set or `AUTH <password>` when only `password_file` is set, then send `PING`. The result SHALL be `up` when `PING` returns `PONG` and, if `role` is set, the first element of the `ROLE` reply matches it, where `master` matches `master` and `replica` matches `slave`. With `role = "replica"`, the fourth element of the `ROLE` reply (the link state) SHALL be `connected`; any other state SHALL be `down` with reason `replication: link to master is <state>`. With `max_lag` set, the plugin SHALL also send `INFO replication` and the result SHALL be `down` with reason `replication: <N>s behind, max <M>s` when `master_last_io_seconds_ago` is greater than `max_lag`. A failed `AUTH` SHALL be `down` with a reason starting `auth: `.

#### Scenario: Healthy redis

- **WHEN** a redis backend without a password answers `PING` with `PONG`
- **THEN** the check result is `up`

#### Scenario: Wrong password

- **WHEN** `password_file` holds a password the redis backend rejects
- **THEN** the check result is `down` with a reason starting `auth: `, and the reason does not contain the password

#### Scenario: Replica that lost its master

- **WHEN** `role = "replica"` and the backend answers `ROLE` with `slave` and link state `connect`
- **THEN** the check result is `down` with reason `replication: link to master is connect`

##### Example: redis role

| `role` | `max_lag` | `ROLE` reply | `master_last_io_seconds_ago` | Result |
| --- | --- | --- | --- | --- |
| (not set) | (not set) | `slave`, link `connect` | (not asked) | up |
| `master` | (not set) | `master` | (not asked) | up |
| `master` | (not set) | `slave`, link `connected` | (not asked) | down: `role is replica, want master` |
| `replica` | (not set) | `slave`, link `connected` | (not asked) | up |
| `replica` | (not set) | `master` | (not asked) | down: `role is master, want replica` |
| `master` | (not set) | `sentinel` | (not asked) | down: `role is sentinel, want master` |
| `replica` | (not set) | `slave`, link `sync` | (not asked) | down: `replication: link to master is sync` |
| `replica` | 15 | `slave`, link `connected` | 3 | up |
| `replica` | 15 | `slave`, link `connected` | 15 | up |
| `replica` | 15 | `slave`, link `connected` | 16 | down: `replication: 16s behind, max 15s` |

### Requirement: MySQL check

The `mysql` plugin SHALL log in as `user` with the password from `password_file` (or no password when it is not set) and run `SELECT 1`. When `role = "primary"` it SHALL also run `SELECT @@global.read_only` and the result SHALL be `down` with reason `role is read-only, want primary` when the value is not 0. When `role = "replica"` it SHALL run `SHOW REPLICA STATUS`, and `SHOW SLAVE STATUS` instead only when the server rejects the first with error 1064. No row SHALL be `down` with reason `role is not a replica, want replica`. The IO thread (`Replica_IO_Running` or `Slave_IO_Running`) not being `Yes` SHALL be `down` with reason `replication: IO thread is <value>`; otherwise the SQL thread (`Replica_SQL_Running` or `Slave_SQL_Running`) not being `Yes` SHALL be `down` with reason `replication: SQL thread is <value>`. Each of these reasons SHALL be followed by `: ` and `Last_IO_Error` or `Last_SQL_Error` respectively when that column is not empty. With `max_lag` set, `Seconds_Behind_Source` or `Seconds_Behind_Master` being NULL SHALL be `down` with reason `replication: lag unknown`, and being greater than `max_lag` SHALL be `down` with reason `replication: <N>s behind, max <M>s`. A rejected login SHALL be `down` with a reason starting `auth: `.

#### Scenario: Primary MySQL

- **WHEN** `role = "primary"` and the backend logs in, returns 1 for `SELECT 1` and 0 for `@@global.read_only`
- **THEN** the check result is `up`

#### Scenario: Read-only replica

- **WHEN** `role = "primary"` and the backend returns 1 for `@@global.read_only`
- **THEN** the check result is `down` with reason `role is read-only, want primary`

#### Scenario: Replicating replica

- **WHEN** `role = "replica"` and `max_lag = 30`, and the backend's replica status has both threads `Yes` and 2 seconds behind
- **THEN** the check result is `up`

##### Example: mysql replica status

| `max_lag` | Replica status | Result |
| --- | --- | --- |
| (not set) | no row | down: `role is not a replica, want replica` |
| (not set) | IO `Yes`, SQL `Yes`, behind NULL | up |
| (not set) | IO `Connecting`, SQL `Yes`, `Last_IO_Error` = `error connecting to master` | down: `replication: IO thread is Connecting: error connecting to master` |
| (not set) | IO `No`, SQL `No`, both errors empty | down: `replication: IO thread is No` |
| (not set) | IO `Yes`, SQL `No`, `Last_SQL_Error` = `Duplicate entry` | down: `replication: SQL thread is No: Duplicate entry` |
| 30 | IO `Yes`, SQL `Yes`, behind 30 | up |
| 30 | IO `Yes`, SQL `Yes`, behind 31 | down: `replication: 31s behind, max 30s` |
| 30 | IO `Yes`, SQL `Yes`, behind NULL | down: `replication: lag unknown` |

### Requirement: PostgreSQL check

The `pgsql` plugin SHALL log in as `user` to `database` with the password from `password_file` (or no password when it is not set) and run `SELECT 1`. When `role = "primary"` it SHALL also run `SELECT pg_is_in_recovery()` and the result SHALL be `down` with reason `role is standby, want primary` when it returns true. When `role = "replica"` it SHALL run one query returning `pg_is_in_recovery()`, whether `pg_stat_wal_receiver` has a row, and the replay lag in seconds, where the lag SHALL be 0 when `pg_last_wal_receive_lsn()` equals `pg_last_wal_replay_lsn()` and otherwise the time from `pg_last_xact_replay_timestamp()` to now. Not being in recovery SHALL be `down` with reason `role is primary, want replica`; no WAL receiver row SHALL be `down` with reason `replication: no WAL receiver`. With `max_lag` set, a NULL lag SHALL be `down` with reason `replication: lag unknown`, and a lag whose whole seconds N (rounded down) are greater than `max_lag` SHALL be `down` with reason `replication: <N>s behind, max <M>s`. A rejected login SHALL be `down` with a reason starting `auth: `.

#### Scenario: Standby server

- **WHEN** `role = "primary"` and the backend returns true for `pg_is_in_recovery()`
- **THEN** the check result is `down` with reason `role is standby, want primary`

#### Scenario: Missing database

- **WHEN** `database = "nope"` names a database that does not exist on the backend
- **THEN** the check result is `down` with a reason starting `auth: ` or `query: `

#### Scenario: Streaming standby

- **WHEN** `role = "replica"` and the backend is in recovery with a WAL receiver and has replayed all received WAL
- **THEN** the check result is `up`

##### Example: pgsql replica status

| `max_lag` | In recovery | WAL receiver | Lag | Result |
| --- | --- | --- | --- | --- |
| (not set) | false | no | NULL | down: `role is primary, want replica` |
| (not set) | true | no | 0 | down: `replication: no WAL receiver` |
| (not set) | true | yes | 500.0 | up |
| 30 | true | yes | 0 | up |
| 30 | true | yes | 30.9 | up |
| 30 | true | yes | 31.0 | down: `replication: 31s behind, max 30s` |
| 30 | true | yes | NULL | down: `replication: lag unknown` |
