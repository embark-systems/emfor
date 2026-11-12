# database-checks Specification

## Purpose

Defines `emfor-check`, the plugin executable shipped with emfor, and its `redis`, `mysql` and `pgsql` plugins. It covers how each plugin is selected, which check options it accepts, what a check verifies (including the optional role check and TLS), how passwords are supplied, and how outcomes map to `up`, `down` and `error`.

## Requirements

### Requirement: Plugin selection by argument

`emfor-check` SHALL take exactly one argument naming the plugin: `redis`, `mysql` or `pgsql`. With a missing, extra or unknown argument it SHALL print a usage line to stderr and exit with status 2 without answering the handshake. With a valid argument it SHALL speak checker-plugins protocol 1 and answer `hello` with `name` equal to the plugin name and `version` equal to the `emfor-check` package version. Each check SHALL open a new connection to the backend and close it before answering. A check that has not finished within the request's `timeout_ms` SHALL be answered `down` with reason `timeout`.

#### Scenario: Unknown plugin argument

- **WHEN** a config declares `[plugins."kv"]` with `command = ["/usr/local/libexec/emfor-check", "memcached"]`
- **THEN** the plugin fails to start and the config is rejected with an error naming plugin `kv`

#### Scenario: Handshake names the plugin

- **WHEN** `emfor-check redis` receives `{"type":"hello","protocol":1}`
- **THEN** it answers `{"type":"hello","protocol":1,"name":"redis","version":"<package version>"}`

#### Scenario: Slow backend

- **WHEN** a check request has `timeout_ms` 900 and the backend accepts the TCP connection but never answers
- **THEN** the plugin answers that request within about 900 ms with `result` `down` and reason `timeout`


<!-- @trace
source: emfor-check-plugins
updated: 2026-11-12
code:
  - tests/plugins/run.sh
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/options.rs
  - tests/plugins/inside.sh
  - crates/emfor-plugin/src/lib.rs
  - docs/deploy/plugins.md
  - crates/emfor-check/src/main.rs
  - docs/plugins/protocol-v1.md
  - tests/plugins/Dockerfile.dockerignore
  - Cargo.toml
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/net.rs
  - crates/emfor-check/src/mysql.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-plugin/Cargo.toml
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/resp.rs
  - examples/emfor.toml
  - crates/emfor-check/src/redis.rs
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/data/test-ca.pem
  - crates/emfor-daemon/tests/support/test_plugin.rs
  - crates/emfor-daemon/tests/plugins.rs
  - crates/emfor-check/tests/cli.rs
-->

---
### Requirement: Check options validation

Each plugin SHALL validate `check_options` with the keys below and SHALL report every other key as `unknown key`. `user` SHALL be required for `mysql` and `pgsql`; `database` SHALL be required for `pgsql` and SHALL be an unknown key for `redis` and `mysql`. `role` SHALL accept `master` or `replica` for `redis` and `primary` for `mysql` and `pgsql`. `password_file` and `ca_file` SHALL be absolute paths of readable files; `password_file` SHALL be non-empty after removing one trailing `\n` or `\r\n`; `ca_file` SHALL contain at least one PEM certificate. `tls_verify`, `server_name` and `ca_file` SHALL be errors (`requires tls = true`) unless `tls = true`. For `redis`, `user` without `password_file` SHALL be an error on `user` (`requires password_file`).

#### Scenario: Valid options

- **WHEN** a service with `check = "pgsql"` sets `check_options = { user = "emfor", database = "app", role = "primary" }`
- **THEN** the plugin reports no errors and the config is accepted

##### Example: option validation

| Plugin | `check_options` | Errors |
| --- | --- | --- |
| mysql | `{}` | `user`: missing required key |
| pgsql | `{ user = "emfor" }` | `database`: missing required key |
| redis | `{ database = "0" }` | `database`: unknown key |
| redis | `{ role = "primary" }` | `role`: must be one of master, replica |
| mysql | `{ user = "emfor", role = "master" }` | `role`: must be primary |
| redis | `{ user = "emfor" }` | `user`: requires password_file |
| redis | `{ password_file = "redis.pass" }` | `password_file`: not an absolute path |
| redis | `{ password_file = "/etc/emfor/missing" }` | `password_file`: file not found |
| redis | `{ password_file = "/etc/emfor/empty" }` (file holds only `\n`) | `password_file`: file is empty |
| redis | `{ server_name = "redis.internal" }` | `server_name`: requires tls = true |
| redis | `{ tls = "yes" }` | `tls`: must be a boolean |
| redis | `{ tls = true, ca_file = "/etc/emfor/not-pem.txt" }` | `ca_file`: no certificate found |
| redis | `{ timeout = 1 }` | `timeout`: unknown key |

#### Scenario: Config check reports plugin errors

- **WHEN** `emfor --check-config` runs on a config whose `mysql` service has `check_options = {}`
- **THEN** it fails with `services."<name>".check_options.user: missing required key`


<!-- @trace
source: emfor-check-plugins
updated: 2026-11-12
code:
  - tests/plugins/run.sh
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/options.rs
  - tests/plugins/inside.sh
  - crates/emfor-plugin/src/lib.rs
  - docs/deploy/plugins.md
  - crates/emfor-check/src/main.rs
  - docs/plugins/protocol-v1.md
  - tests/plugins/Dockerfile.dockerignore
  - Cargo.toml
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/net.rs
  - crates/emfor-check/src/mysql.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-plugin/Cargo.toml
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/resp.rs
  - examples/emfor.toml
  - crates/emfor-check/src/redis.rs
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/data/test-ca.pem
  - crates/emfor-daemon/tests/support/test_plugin.rs
  - crates/emfor-daemon/tests/plugins.rs
  - crates/emfor-check/tests/cli.rs
-->

---
### Requirement: Redis check

The `redis` plugin SHALL connect to the backend, send `AUTH <user> <password>` when `user` is set or `AUTH <password>` when only `password_file` is set, then send `PING`. The result SHALL be `up` when `PING` returns `PONG` and, if `role` is set, the first element of the `ROLE` reply matches it, where `master` matches `master` and `replica` matches `slave`. A failed `AUTH` SHALL be `down` with a reason starting `auth: `.

#### Scenario: Healthy redis

- **WHEN** a redis backend without a password answers `PING` with `PONG`
- **THEN** the check result is `up`

#### Scenario: Wrong password

- **WHEN** `password_file` holds a password the redis backend rejects
- **THEN** the check result is `down` with a reason starting `auth: `, and the reason does not contain the password

##### Example: redis role

| `role` | `ROLE` first element | Result |
| --- | --- | --- |
| (not set) | `slave` | up |
| `master` | `master` | up |
| `master` | `slave` | down: `role is replica, want master` |
| `replica` | `slave` | up |
| `replica` | `master` | down: `role is master, want replica` |
| `master` | `sentinel` | down: `role is sentinel, want master` |


<!-- @trace
source: emfor-check-plugins
updated: 2026-11-12
code:
  - tests/plugins/run.sh
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/options.rs
  - tests/plugins/inside.sh
  - crates/emfor-plugin/src/lib.rs
  - docs/deploy/plugins.md
  - crates/emfor-check/src/main.rs
  - docs/plugins/protocol-v1.md
  - tests/plugins/Dockerfile.dockerignore
  - Cargo.toml
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/net.rs
  - crates/emfor-check/src/mysql.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-plugin/Cargo.toml
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/resp.rs
  - examples/emfor.toml
  - crates/emfor-check/src/redis.rs
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/data/test-ca.pem
  - crates/emfor-daemon/tests/support/test_plugin.rs
  - crates/emfor-daemon/tests/plugins.rs
  - crates/emfor-check/tests/cli.rs
-->

---
### Requirement: MySQL check

The `mysql` plugin SHALL log in as `user` with the password from `password_file` (or no password when it is not set) and run `SELECT 1`. When `role = "primary"` it SHALL also run `SELECT @@global.read_only` and the result SHALL be `down` with reason `role is read-only, want primary` when the value is not 0. A rejected login SHALL be `down` with a reason starting `auth: `.

#### Scenario: Primary MySQL

- **WHEN** `role = "primary"` and the backend logs in, returns 1 for `SELECT 1` and 0 for `@@global.read_only`
- **THEN** the check result is `up`

#### Scenario: Read-only replica

- **WHEN** `role = "primary"` and the backend returns 1 for `@@global.read_only`
- **THEN** the check result is `down` with reason `role is read-only, want primary`


<!-- @trace
source: emfor-check-plugins
updated: 2026-11-12
code:
  - tests/plugins/run.sh
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/options.rs
  - tests/plugins/inside.sh
  - crates/emfor-plugin/src/lib.rs
  - docs/deploy/plugins.md
  - crates/emfor-check/src/main.rs
  - docs/plugins/protocol-v1.md
  - tests/plugins/Dockerfile.dockerignore
  - Cargo.toml
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/net.rs
  - crates/emfor-check/src/mysql.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-plugin/Cargo.toml
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/resp.rs
  - examples/emfor.toml
  - crates/emfor-check/src/redis.rs
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/data/test-ca.pem
  - crates/emfor-daemon/tests/support/test_plugin.rs
  - crates/emfor-daemon/tests/plugins.rs
  - crates/emfor-check/tests/cli.rs
-->

---
### Requirement: PostgreSQL check

The `pgsql` plugin SHALL log in as `user` to `database` with the password from `password_file` (or no password when it is not set) and run `SELECT 1`. When `role = "primary"` it SHALL also run `SELECT pg_is_in_recovery()` and the result SHALL be `down` with reason `role is standby, want primary` when it returns true. A rejected login SHALL be `down` with a reason starting `auth: `.

#### Scenario: Standby server

- **WHEN** `role = "primary"` and the backend returns true for `pg_is_in_recovery()`
- **THEN** the check result is `down` with reason `role is standby, want primary`

#### Scenario: Missing database

- **WHEN** `database = "nope"` names a database that does not exist on the backend
- **THEN** the check result is `down` with a reason starting `auth: ` or `query: `


<!-- @trace
source: emfor-check-plugins
updated: 2026-11-12
code:
  - tests/plugins/run.sh
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/options.rs
  - tests/plugins/inside.sh
  - crates/emfor-plugin/src/lib.rs
  - docs/deploy/plugins.md
  - crates/emfor-check/src/main.rs
  - docs/plugins/protocol-v1.md
  - tests/plugins/Dockerfile.dockerignore
  - Cargo.toml
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/net.rs
  - crates/emfor-check/src/mysql.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-plugin/Cargo.toml
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/resp.rs
  - examples/emfor.toml
  - crates/emfor-check/src/redis.rs
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/data/test-ca.pem
  - crates/emfor-daemon/tests/support/test_plugin.rs
  - crates/emfor-daemon/tests/plugins.rs
  - crates/emfor-check/tests/cli.rs
-->

---
### Requirement: Passwords from password_file only

A plugin SHALL accept a password only through `password_file`; a `password` key SHALL be reported as `unknown key`. The plugin SHALL read the file on every check, removing one trailing `\n` or `\r\n`, so that a changed password takes effect without a reload. When the file cannot be read during a check, the result SHALL be `error` with a reason starting `password_file: ` that names the I/O error kind. No check reason and no line the plugin writes to stderr SHALL contain the password.

#### Scenario: Password rotated

- **WHEN** the password in `password_file` and on the backend are both changed while emfor runs
- **THEN** the next check uses the new password and is `up` without a reload

#### Scenario: Password file removed after validation

- **WHEN** `password_file` is deleted after the config was applied
- **THEN** checks of that service are `error` with a reason starting `password_file: `, and the backends' health state does not count them as failures


<!-- @trace
source: emfor-check-plugins
updated: 2026-11-12
code:
  - tests/plugins/run.sh
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/options.rs
  - tests/plugins/inside.sh
  - crates/emfor-plugin/src/lib.rs
  - docs/deploy/plugins.md
  - crates/emfor-check/src/main.rs
  - docs/plugins/protocol-v1.md
  - tests/plugins/Dockerfile.dockerignore
  - Cargo.toml
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/net.rs
  - crates/emfor-check/src/mysql.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-plugin/Cargo.toml
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/resp.rs
  - examples/emfor.toml
  - crates/emfor-check/src/redis.rs
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/data/test-ca.pem
  - crates/emfor-daemon/tests/support/test_plugin.rs
  - crates/emfor-daemon/tests/plugins.rs
  - crates/emfor-check/tests/cli.rs
-->

---
### Requirement: TLS connections

With `tls = true` a plugin SHALL connect with TLS and SHALL NOT fall back to plaintext. It SHALL verify the server certificate against the certificates in `ca_file`, or the system trust store when `ca_file` is not set, and against `server_name`, or the backend IP when `server_name` is not set; `server_name` SHALL also be sent as SNI. With `tls_verify = false` it SHALL accept any server certificate and still encrypt. A TLS handshake or certificate verification failure SHALL be `down` with a reason starting `tls: `.

#### Scenario: Name mismatch

- **WHEN** `tls = true`, `ca_file` holds the CA that signed the server certificate, and `server_name` does not match the certificate
- **THEN** the check result is `down` with a reason starting `tls: `

#### Scenario: Verification turned off

- **WHEN** `tls = true` and `tls_verify = false` and the server certificate names another host
- **THEN** the check result is `up` when the service itself is healthy

#### Scenario: Server without TLS

- **WHEN** `tls = true` and the backend port speaks plaintext
- **THEN** the check result is `down` with a reason starting `tls: `


<!-- @trace
source: emfor-check-plugins
updated: 2026-11-12
code:
  - tests/plugins/run.sh
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/options.rs
  - tests/plugins/inside.sh
  - crates/emfor-plugin/src/lib.rs
  - docs/deploy/plugins.md
  - crates/emfor-check/src/main.rs
  - docs/plugins/protocol-v1.md
  - tests/plugins/Dockerfile.dockerignore
  - Cargo.toml
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/net.rs
  - crates/emfor-check/src/mysql.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-plugin/Cargo.toml
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/resp.rs
  - examples/emfor.toml
  - crates/emfor-check/src/redis.rs
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/data/test-ca.pem
  - crates/emfor-daemon/tests/support/test_plugin.rs
  - crates/emfor-daemon/tests/plugins.rs
  - crates/emfor-check/tests/cli.rs
-->

---
### Requirement: Check outcome classification

A plugin SHALL answer `up` only when every step succeeded and the role, if set, matches. It SHALL answer `down` when the backend refuses or drops the connection (reason starting `connect: `), times out (`timeout`), fails TLS (`tls: `), rejects the login (`auth: `), fails or answers a query unexpectedly (`query: `), or has another role (`role is <actual>, want <wanted>`). It SHALL answer `error` only for failures on the plugin's side: `password_file` or `ca_file` unreadable at check time, or a local resource limit (too many open files, out of memory).

#### Scenario: Backend stopped

- **WHEN** nothing listens on the backend port
- **THEN** the check result is `down` with a reason starting `connect: `

#### Scenario: Failover follows the primary

- **WHEN** a service has `balance = "failover"`, `check = "pgsql"`, `role = "primary"`, its first backend is a standby and its second is the primary
- **THEN** after the checks reach `rise`, a lookup of the service returns the second backend

<!-- @trace
source: emfor-check-plugins
updated: 2026-11-12
code:
  - tests/plugins/run.sh
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/options.rs
  - tests/plugins/inside.sh
  - crates/emfor-plugin/src/lib.rs
  - docs/deploy/plugins.md
  - crates/emfor-check/src/main.rs
  - docs/plugins/protocol-v1.md
  - tests/plugins/Dockerfile.dockerignore
  - Cargo.toml
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/net.rs
  - crates/emfor-check/src/mysql.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-plugin/Cargo.toml
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/resp.rs
  - examples/emfor.toml
  - crates/emfor-check/src/redis.rs
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/data/test-ca.pem
  - crates/emfor-daemon/tests/support/test_plugin.rs
  - crates/emfor-daemon/tests/plugins.rs
  - crates/emfor-check/tests/cli.rs
-->