# database-checks Specification

## Purpose

Defines `emfor-check`, the plugin executable shipped with emfor, and its `redis`, `mysql` and `pgsql` plugins. It covers how each plugin is selected, which check options it accepts, what a check verifies (including the optional role check and TLS), how passwords are supplied, and how outcomes map to `up`, `down` and `error`.

## Requirements

### Requirement: Plugin selection by argument

`emfor-check` SHALL take exactly one argument naming the plugin: `redis`, `mysql`, `pgsql`, `ldap`, `dns` or `radius`. With a missing, extra or unknown argument it SHALL print a usage line to stderr and exit with status 2 without answering the handshake. With a valid argument it SHALL speak checker-plugins protocol 1 and answer `hello` with `name` equal to the plugin name and `version` equal to the `emfor-check` package version. Each check of the `redis`, `mysql`, `pgsql` and `ldap` plugins SHALL open a new connection to the backend and close it before answering. A check that has not finished within the request's `timeout_ms` SHALL be answered `down` with reason `timeout`.

#### Scenario: Unknown plugin argument

- **WHEN** a config declares `[plugins."kv"]` with `command = ["/usr/local/libexec/emfor-check", "memcached"]`
- **THEN** the plugin fails to start and the config is rejected with an error naming plugin `kv`

#### Scenario: Handshake names the plugin

- **WHEN** `emfor-check redis` receives `{"type":"hello","protocol":1}`
- **THEN** it answers `{"type":"hello","protocol":1,"name":"redis","version":"<package version>"}`

#### Scenario: LDAP plugin handshake

- **WHEN** `emfor-check ldap` receives `{"type":"hello","protocol":1}`
- **THEN** it answers `{"type":"hello","protocol":1,"name":"ldap","version":"<package version>"}`

#### Scenario: DNS plugin handshake

- **WHEN** `emfor-check dns` receives `{"type":"hello","protocol":1}`
- **THEN** it answers `{"type":"hello","protocol":1,"name":"dns","version":"<package version>"}`

#### Scenario: RADIUS plugin handshake

- **WHEN** `emfor-check radius` receives `{"type":"hello","protocol":1}`
- **THEN** it answers `{"type":"hello","protocol":1,"name":"radius","version":"<package version>"}`

#### Scenario: Slow backend

- **WHEN** a check request has `timeout_ms` 900 and the backend accepts the TCP connection but never answers
- **THEN** the plugin answers that request within about 900 ms with `result` `down` and reason `timeout`


<!-- @trace
source: emfor-check-radius
updated: 2026-11-13
code:
  - CHANGELOG.md
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/options.rs
  - examples/emfor.toml
  - crates/emfor-check/src/main.rs
  - crates/emfor-check/src/radius.rs
  - docs/manual/04-health-checks.md
  - docs/deploy/plugins.md
  - docs/manual.zh_TW/04-health-checks.md
  - tests/plugins/Dockerfile
  - crates/emfor-check/Cargo.toml
  - tests/plugins/inside.sh
  - tests/plugins/run.sh
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/cli.rs
-->

---
### Requirement: Check options validation

The `redis`, `mysql`, `pgsql` and `ldap` plugins SHALL each validate `check_options` with the keys below and SHALL report every other key as `unknown key`. `user` SHALL be required for `mysql` and `pgsql`; `database` SHALL be required for `pgsql` and SHALL be an unknown key for `redis`, `mysql` and `ldap`. `role` SHALL accept `master` or `replica` for `redis` and `primary` for `mysql` and `pgsql`, and SHALL be an unknown key for `ldap`. `starttls` SHALL be a boolean accepted by `ldap` only and SHALL be an unknown key for the other plugins. `password_file` and `ca_file` SHALL be absolute paths of readable files; `password_file` SHALL be non-empty after removing one trailing `\n` or `\r\n`; `ca_file` SHALL contain at least one PEM certificate. For `redis`, `mysql` and `pgsql`, `tls_verify`, `server_name` and `ca_file` SHALL be errors (`requires tls = true`) unless `tls = true`. For `ldap`, they SHALL be errors (`requires tls = true or starttls = true`) unless `tls = true` or `starttls = true`, and `tls = true` together with `starttls = true` SHALL be an error on `starttls` (`conflicts with tls`). For `redis`, `user` without `password_file` SHALL be an error on `user` (`requires password_file`). For `ldap`, `user` without `password_file` SHALL be an error on `user` (`requires password_file`), and `password_file` without `user` SHALL be an error on `password_file` (`requires user`).

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


<!-- @trace
source: emfor-check-dns
updated: 2026-11-13
code:
  - examples/emfor.toml
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/options.rs
  - tests/plugins/run.sh
  - tests/plugins/inside.sh
  - CHANGELOG.md
  - crates/emfor-check/src/dns.rs
  - docs/deploy/plugins.md
  - crates/emfor-check/src/net.rs
  - docs/manual.zh_TW/04-health-checks.md
  - docs/manual/04-health-checks.md
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/main.rs
  - tests/plugins/Dockerfile
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
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

With `tls = true` a plugin SHALL connect with TLS and SHALL NOT fall back to plaintext. With `starttls = true` the `ldap` plugin SHALL connect in plaintext, send the StartTLS extended request, and on success perform the TLS handshake on the same connection before binding; when the server rejects StartTLS or answers it with anything other than success, the result SHALL be `down` with a reason starting `tls: ` and the plugin SHALL NOT bind in plaintext. In both cases the plugin SHALL verify the server certificate against the certificates in `ca_file`, or the system trust store when `ca_file` is not set, and against `server_name`, or the backend IP when `server_name` is not set; `server_name` SHALL also be sent as SNI. With `tls_verify = false` it SHALL accept any server certificate and still encrypt. A TLS handshake or certificate verification failure SHALL be `down` with a reason starting `tls: `.

#### Scenario: Name mismatch

- **WHEN** `tls = true`, `ca_file` holds the CA that signed the server certificate, and `server_name` does not match the certificate
- **THEN** the check result is `down` with a reason starting `tls: `

#### Scenario: Verification turned off

- **WHEN** `tls = true` and `tls_verify = false` and the server certificate names another host
- **THEN** the check result is `up` when the service itself is healthy

#### Scenario: Server without TLS

- **WHEN** `tls = true` and the backend port speaks plaintext
- **THEN** the check result is `down` with a reason starting `tls: `

#### Scenario: StartTLS upgrade

- **WHEN** an `ldap` service sets `starttls = true`, `server_name` and `ca_file` matching the server certificate, and the server supports StartTLS on its plaintext port
- **THEN** the bind happens over TLS and the check result is `up`

#### Scenario: StartTLS refused

- **WHEN** an `ldap` service sets `starttls = true` and the server answers the StartTLS request with a non-zero resultCode
- **THEN** the check result is `down` with a reason starting `tls: StartTLS refused: ` and no bind request is sent

##### Example: refused StartTLS

- **GIVEN** `check_options = { starttls = true, tls_verify = false }`
- **WHEN** the server answers the StartTLS extended request with resultCode 2 and an empty diagnosticMessage
- **THEN** the reason is `tls: StartTLS refused: 2 protocolError`


<!-- @trace
source: emfor-check-ldap
updated: 2026-11-12
code:
  - tests/plugins/inside.sh
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/ldap.rs
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/ber.rs
  - docs/deploy/plugins.md
  - examples/emfor.toml
  - tests/plugins/run.sh
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/options.rs
  - crates/emfor-check/src/main.rs
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/cli.rs
-->

---
### Requirement: Check outcome classification

The `redis`, `mysql`, `pgsql` and `ldap` plugins SHALL answer `up` only when every step succeeded and the role, if set, matches. It SHALL answer `down` when the backend refuses or drops the connection (reason starting `connect: `), times out (`timeout`), fails TLS (`tls: `), rejects the login (`auth: `), fails or answers a query unexpectedly (`query: `), or has another role (`role is <actual>, want <wanted>`). It SHALL answer `error` only for failures on the plugin's side: `password_file` or `ca_file` unreadable at check time, or a local resource limit (too many open files, out of memory).

#### Scenario: Backend stopped

- **WHEN** nothing listens on the backend port
- **THEN** the check result is `down` with a reason starting `connect: `

#### Scenario: Failover follows the primary

- **WHEN** a service has `balance = "failover"`, `check = "pgsql"`, `role = "primary"`, its first backend is a standby and its second is the primary
- **THEN** after the checks reach `rise`, a lookup of the service returns the second backend


<!-- @trace
source: emfor-check-dns
updated: 2026-11-13
code:
  - examples/emfor.toml
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/options.rs
  - tests/plugins/run.sh
  - tests/plugins/inside.sh
  - CHANGELOG.md
  - crates/emfor-check/src/dns.rs
  - docs/deploy/plugins.md
  - crates/emfor-check/src/net.rs
  - docs/manual.zh_TW/04-health-checks.md
  - docs/manual/04-health-checks.md
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/main.rs
  - tests/plugins/Dockerfile
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: LDAP check

The `ldap` plugin SHALL perform an LDAPv3 simple bind and SHALL answer `up` when the BindResponse resultCode is 0. Without `user` it SHALL bind anonymously (empty name, empty password); with `user` it SHALL bind with `user` as the DN and the content of `password_file` as the password. After a successful bind it SHALL send an UnbindRequest before closing the connection. A BindResponse with a non-zero resultCode SHALL be `down` with reason `auth: <code> <name>`, followed by `: <diagnosticMessage>` when the server sent a non-empty diagnosticMessage; `<name>` SHALL be the resultCode name defined in RFC 4511 section 4.1.9 (for example `invalidCredentials` for 49), and SHALL be left out (keeping only the code) for codes RFC 4511 does not define. A response that is not a valid LDAPMessage, has another messageID, carries another protocol operation, or a Notice of Disconnection, SHALL be `down` with a reason starting `query: `. The plugin SHALL read at most 65536 bytes for one response and SHALL answer `down` with a reason starting `query: ` when a response is longer.

#### Scenario: Anonymous bind

- **WHEN** an `ldap` service sets no `user` and the server allows anonymous bind
- **THEN** the plugin sends a BindRequest with version 3, an empty name and an empty simple password, and the result is `up`

##### Example: anonymous bind bytes

- **GIVEN** `check_options = {}`
- **WHEN** the plugin sends its first message
- **THEN** the bytes are `30 0c 02 01 01 60 07 02 01 03 04 00 80 00` (LDAPMessage, messageID 1, BindRequest version 3, empty name, empty simple password)

#### Scenario: Bind with a password

- **WHEN** an `ldap` service sets `user` and `password_file` matching an entry on the server
- **THEN** the result is `up`

#### Scenario: Wrong LDAP password

- **WHEN** the password in `password_file` does not match the entry named by `user`
- **THEN** the result is `down` with reason starting `auth: 49 invalidCredentials` and the reason does not contain the password

##### Example: result codes in reasons

| resultCode | diagnosticMessage | Reason |
| --- | --- | --- |
| 49 | empty | `auth: 49 invalidCredentials` |
| 53 | `unauthenticated bind (DN with no password) disallowed` | `auth: 53 unwillingToPerform: unauthenticated bind (DN with no password) disallowed` |
| 8 | empty | `auth: 8 strongerAuthRequired` |
| 4096 | empty | `auth: 4096` |

#### Scenario: Malformed response

- **WHEN** the server answers the BindRequest with bytes that are not a BindResponse for messageID 1
- **THEN** the result is `down` with a reason starting `query: `

<!-- @trace
source: emfor-check-ldap
updated: 2026-11-12
code:
  - tests/plugins/inside.sh
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/ldap.rs
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/ber.rs
  - docs/deploy/plugins.md
  - examples/emfor.toml
  - tests/plugins/run.sh
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/options.rs
  - crates/emfor-check/src/main.rs
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/cli.rs
-->