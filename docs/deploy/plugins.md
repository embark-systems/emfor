# Checking redis, MySQL, PostgreSQL and LDAP with emfor-check

`emfor-check` is the plugin program that ships with emfor. One executable holds four plugins; its only argument picks which one runs:

| Argument | Checks | Up when |
| --- | --- | --- |
| `redis` | redis | `PING` answers `PONG` (after `AUTH` when a password is set) |
| `mysql` | MySQL, MariaDB | the login succeeds and `SELECT 1` returns 1 |
| `pgsql` | PostgreSQL | the login succeeds and `SELECT 1` returns 1 |
| `ldap` | OpenLDAP and other LDAPv3 servers | a simple bind succeeds (resultCode 0) |

Each plugin runs as one long-lived child process of the emfor daemon, shared by every service that uses it (see [the plugin protocol](../plugins/protocol-v1.md)). Each check opens a new connection to the backend and closes it before answering, so a server that refuses new connections (for example because `max_connections` is reached) is reported as down.

## Installing

Build it with the rest of emfor and copy it next to the daemon's other helpers:

```sh
cargo build --release --bin emfor-check
install -m 0755 target/release/emfor-check /usr/local/libexec/emfor-check
```

Or with `cargo install`, which puts it in `<root>/bin`: `cargo install --locked --path crates/emfor-check --root /usr/local` (see [daemon.md](daemon.md#building-with-an-install-prefix)).

The database drivers (and the LDAP encoder) are linked into `emfor-check` only; the `emfor` daemon and libemfor do not contain them.

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
| `user` | all; required for `mysql`, `pgsql` | string | none | Account to log in with; for `ldap` a DN, and without it the bind is anonymous |
| `password_file` | all | absolute path | none (no password) | File holding the password; see below |
| `database` | `pgsql` only, required | string | none | Database to connect to |
| `role` | all but `ldap` | string | none (any role) | `redis`: `master` or `replica`; `mysql`, `pgsql`: `primary` |
| `tls` | all | boolean | `false` | Connect with TLS; `true` never falls back to plaintext. For `ldap` this is LDAPS (usually port 636) |
| `starttls` | `ldap` only | boolean | `false` | Connect in plaintext, then upgrade with StartTLS before the bind; never binds without TLS. Conflicts with `tls` |
| `tls_verify` | all, needs `tls = true` (or `starttls = true`) | boolean | `true` | Verify the server certificate |
| `server_name` | all, needs `tls = true` (or `starttls = true`) | string | the backend IP | Name the certificate must match; also sent as SNI |
| `ca_file` | all, needs `tls = true` (or `starttls = true`) | absolute path | the system's CA certificates | PEM file of CA certificates to trust instead |

Any other key is an error, including `password`: passwords are never written in the config. For `redis`, `user` needs `password_file` (redis has no login without a password). For `ldap`, `user` and `password_file` go together: set both, or neither for an anonymous bind.

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

With `tls = true` (or `starttls = true` for `ldap`), the certificate is checked against `server_name`, or against the backend IP when `server_name` is not set. Backends are listed by IP, and database certificates usually name a host, so set `server_name` unless the certificate has the IP in its subject alternative names. Use `ca_file` for an internal CA. `tls_verify = false` keeps the connection encrypted but accepts any certificate.

### LDAP

The `ldap` plugin sends an LDAPv3 simple bind and is up when the server answers resultCode 0 (success); it then sends an unbind and closes the connection. It does not search or read any entry.

```toml
[plugins."ldap"]
command = ["/usr/local/libexec/emfor-check", "ldap"]

[services."directory.svc.emfor"]
check = "ldap"
backends = ["10.0.4.1:389", "10.0.4.2:389"]
check_options = { user = "cn=monitor,dc=example,dc=org", password_file = "/etc/emfor/ldap.pass", starttls = true, server_name = "ldap.example.org" }
```

- Without `user`, the bind is anonymous. Many servers accept an anonymous bind even when their database is broken, so a bind with a dedicated account says more: give it an entry with a password and no other rights.
- Use `starttls = true` on port 389 or `tls = true` (LDAPS) on port 636, so the password is never sent in plaintext. With `starttls`, a server that refuses StartTLS is down with `tls: StartTLS refused: <code> <name>`, and no bind is sent.
- A rejected bind is down with `auth: <code> <name>`, followed by the server's diagnostic message when it sends one, for example `auth: 49 invalidCredentials`, `auth: 53 unwillingToPerform: …` or `auth: 8 strongerAuthRequired` (the server wants TLS). The names are those of RFC 4511; an unknown code shows only the number.
- A response that is not valid LDAP, answers another message, is longer than 65536 bytes, or is a Notice of Disconnection is down with `query: `.
- Every check is a bind, which LDAP servers usually log. Lower the server's log level for this account, or use a longer `interval`.

## Reading results

| Reason starts with | Result | Meaning |
| --- | --- | --- |
| `connect: ` | down | Connection refused or dropped |
| `timeout` | down | No answer within the service `timeout` |
| `tls: ` | down | TLS handshake or certificate check failed, the port does not speak TLS, or the LDAP server refused StartTLS |
| `auth: ` | down | Login rejected (wrong password, unknown user or database, too many connections); for `ldap`, the resultCode and its name, see [LDAP](#ldap) |
| `query: ` | down | A query failed or answered something unexpected |
| `role is ` | down | The backend has another role |
| `password_file: `, `ca_file: ` | error | The file could not be read during the check |

## What is tested

`tests/plugins/run.sh` checks all four plugins against real servers in a Debian container: redis 7, MariaDB and PostgreSQL 15, each with a primary and a replica, read-only server or standby, and TLS; and OpenLDAP 2.5 with LDAPS, StartTLS and a server without TLS. The `ldap` plugin is tested against OpenLDAP only; Active Directory and other servers are untested. The `mysql` plugin is tested against MariaDB only; check it against your MySQL version before relying on it. Like the rest of emfor, the plugins have not been tested on FreeBSD yet.
