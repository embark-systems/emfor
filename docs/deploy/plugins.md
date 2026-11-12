# Checking redis, MySQL and PostgreSQL with emfor-check

`emfor-check` is the plugin program that ships with emfor. One executable holds three plugins; its only argument picks which one runs:

| Argument | Checks | Up when |
| --- | --- | --- |
| `redis` | redis | `PING` answers `PONG` (after `AUTH` when a password is set) |
| `mysql` | MySQL, MariaDB | the login succeeds and `SELECT 1` returns 1 |
| `pgsql` | PostgreSQL | the login succeeds and `SELECT 1` returns 1 |

Each plugin runs as one long-lived child process of the emfor daemon, shared by every service that uses it (see [the plugin protocol](../plugins/protocol-v1.md)). Each check opens a new connection to the backend and closes it before answering, so a server that refuses new connections (for example because `max_connections` is reached) is reported as down.

## Installing

Build it with the rest of emfor and copy it next to the daemon's other helpers:

```sh
cargo build --release --bin emfor-check
install -m 0755 target/release/emfor-check /usr/local/libexec/emfor-check
```

The database drivers are linked into `emfor-check` only; the `emfor` daemon and libemfor do not contain them.

## Configuring

Declare each plugin you use once, then point services at it with `check`:

```toml
[plugins."redis"]
command = ["/usr/local/libexec/emfor-check", "redis"]

[plugins."pgsql"]
command = ["/usr/local/libexec/emfor-check", "pgsql"]

[services."sessions.svc.emfor"]
check = "redis"
backends = ["10.0.2.1:6379", "10.0.2.2:6379"]
check_options = { user = "emfor", password_file = "/etc/emfor/redis.pass" }

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
check_options = { user = "emfor", database = "postgres", role = "primary", password_file = "/etc/emfor/pg.pass" }
```

The plugin name in `[plugins."<name>"]` is your choice; the argument after the path decides the protocol. `emfor --check-config` starts the plugins and has them validate every service's `check_options`, so mistakes are found before a reload.

### Options

| Key | Plugins | Type | Default | Meaning |
| --- | --- | --- | --- | --- |
| `user` | all; required for `mysql`, `pgsql` | string | none | Account to log in with |
| `password_file` | all | absolute path | none (no password) | File holding the password; see below |
| `database` | `pgsql` only, required | string | none | Database to connect to |
| `role` | all | string | none (any role) | `redis`: `master` or `replica`; `mysql`, `pgsql`: `primary` |
| `tls` | all | boolean | `false` | Connect with TLS; `true` never falls back to plaintext |
| `tls_verify` | all, needs `tls = true` | boolean | `true` | Verify the server certificate |
| `server_name` | all, needs `tls = true` | string | the backend IP | Name the certificate must match; also sent as SNI |
| `ca_file` | all, needs `tls = true` | absolute path | the system's CA certificates | PEM file of CA certificates to trust instead |

Any other key is an error, including `password`: passwords are never written in the config. For `redis`, `user` needs `password_file` (redis has no login without a password).

### Roles and failover

With `role` set, a backend in another role is down, with a reason such as `role is standby, want primary`. Combined with `balance = "failover"`, clients and DNS get the current primary, and follow it after a switchover once the checks reach `rise`:

| Plugin | `role` | Checked with |
| --- | --- | --- |
| `redis` | `master` | `ROLE` says `master` |
| `redis` | `replica` | `ROLE` says `slave` |
| `mysql` | `primary` | `SELECT @@global.read_only` is 0 |
| `pgsql` | `primary` | `SELECT pg_is_in_recovery()` is false |

### Passwords

`password_file` names a file whose content is the password; one trailing newline (`\n` or `\r\n`) is removed. The plugin reads the file on every check, so you can change the password on the server and in the file without reloading emfor.

- Make the file readable only by the account emfor runs as: `install -m 0600 -o emfor /dev/stdin /etc/emfor/pg.pass`.
- Give emfor its own account with the least rights the check needs: login, `SELECT 1`, and for `role` the role query.
- The password never appears in check reasons, in emfor's log, or in the plugin's stderr.

If the file cannot be read during a check, the check is `error`, which does not count against the backend.

### TLS

With `tls = true`, the certificate is checked against `server_name`, or against the backend IP when `server_name` is not set. Backends are listed by IP, and database certificates usually name a host, so set `server_name` unless the certificate has the IP in its subject alternative names. Use `ca_file` for an internal CA. `tls_verify = false` keeps the connection encrypted but accepts any certificate.

## Reading results

| Reason starts with | Result | Meaning |
| --- | --- | --- |
| `connect: ` | down | Connection refused or dropped |
| `timeout` | down | No answer within the service `timeout` |
| `tls: ` | down | TLS handshake or certificate check failed, or the port does not speak TLS |
| `auth: ` | down | Login rejected (wrong password, unknown user or database, too many connections) |
| `query: ` | down | A query failed or answered something unexpected |
| `role is ` | down | The backend has another role |
| `password_file: `, `ca_file: ` | error | The file could not be read during the check |

## What is tested

`tests/plugins/run.sh` checks all three plugins against real servers in a Debian container: redis 7, MariaDB and PostgreSQL 15, each with a primary and a replica, read-only server or standby, and TLS. The `mysql` plugin is tested against MariaDB only; check it against your MySQL version before relying on it. Like the rest of emfor, the plugins have not been tested on FreeBSD yet.
