# 4. Health checks

[Manual](README.md) · Previous: [3. Configuration](03-configuration.md) · Next: [5. Choosing a backend](05-backend-selection.md)

A service's `check` decides how each backend is tested, and its
`check_options` table configures that checker. Every check ends in `up`,
`down` or `error` ([chapter 1](01-concepts.md#check-results-and-health-states)).

| `check` | Tests | Good for |
| --- | --- | --- |
| `tcp` | A TCP connection, optionally TLS and a send/expect exchange | Any TCP service; greeting-based protocols such as SMTP |
| `http` | One `GET` request and its status code | Web services and health endpoints |
| `exec` | A program run once per check | Existing Nagios-style scripts, infrequent checks |
| a plugin name | A long-running plugin program | redis, MySQL, PostgreSQL, LDAP, DNS, RADIUS, SIP (`emfor-check`), or your own |

A check that has not finished within the service's `timeout` is `down`
with reason `timeout`. The one exception is a plugin: a plugin that does not
answer in time gives `error`, because the plugin, not the backend, is late.

## tcp

Without options, a backend is `up` when it accepts a connection; emfor
closes the connection at once. A refused or unreachable connection is
`down`.

| Option | Type | Default | Meaning |
| --- | --- | --- | --- |
| `send` | string | none | Bytes written once after connecting. TOML escapes such as `\r\n` are sent as those bytes. Needs `expect`. |
| `expect` | string | none | Bytes that must appear, case-sensitively, in what the backend sends (read up to 16384 bytes). At most 16384 bytes. |
| `tls` | boolean | `false` | Do a TLS handshake right after connecting; never falls back to plaintext. |
| `tls_verify` | boolean | `true` | `false` accepts any certificate (still encrypted). Needs `tls = true`. |
| `server_name` | string | the backend IP | Name (or IP) the certificate must match; sent as SNI when it is a name. Needs `tls = true`. |
| `ca_file` | absolute path | system CAs | PEM file of CA certificates to trust instead. Must exist when the config is loaded. Needs `tls = true`. |

```toml
[services."smtp.svc.emfor"]
check = "tcp"
backends = ["10.0.6.11:25", "10.0.6.12:25"]
check_options = { expect = "220" }

[services."memcache.svc.emfor"]
check = "tcp"
backends = ["10.0.7.11:11211"]
check_options = { send = "stats\r\n", expect = "STAT pid" }

[services."imap.svc.emfor"]
check = "tcp"
backends = ["10.0.8.11:993"]
check_options = { tls = true, server_name = "imap.internal", expect = "* OK" }
```

Typical reasons: `expect: got "421 too busy\r\n"` (the first 64 bytes
received, escaped), `send: ...`, `tls: ...`, and
`tls: no answer to the TLS handshake (does the port speak TLS?)`.

## http

One `GET` per check to the backend's IP and port. Redirects are not
followed.

| Option | Type | Default | Meaning |
| --- | --- | --- | --- |
| `scheme` | `"http"` or `"https"` | `"http"` | Protocol. |
| `path` | string starting with `/` | `"/"` | Request path. |
| `host_header` | string | none | `Host` header to send. |
| `expect_status` | non-empty list | `["200-399"]` | Status codes that count as `up`: codes (`200` or `"200"`) or inclusive ranges (`"200-299"`), all within 100–599. |
| `tls_verify` | boolean | `true` | For `https`: `false` accepts any certificate. |

```toml
[services."api.svc.emfor".check_options]
path = "/healthz"
host_header = "api.internal"
expect_status = ["200-299"]
```

A response with another status is `down` with the status code in the
reason.

## exec

Runs a program once per check, without a shell. The backend is passed in
the environment variables `EMFOR_IP`, `EMFOR_PORT` and `EMFOR_SERVICE`.

```toml
[services."report.svc.emfor"]
check = "exec"
backends = ["10.0.3.1:5432"]
interval = "10s"
timeout = "5s"
check_options = { command = ["/usr/local/libexec/check_pg_replication", "--max-lag", "60"] }
```

| Exit | Result |
| --- | --- |
| 0 | `up` |
| 1 or 2 | `down`; the reason is the first line of stdout, or `exit code N` |
| any other code, killed by a signal, or failed to start | `error` |
| still running at `timeout` | `down` (`timeout`); the program and all its children are killed |

`command` is the only option: a non-empty list whose first element is an
absolute path to an executable. Starting a process per check is costly; for
frequent checks of many backends, use a plugin.

## Plugins and emfor-check

A plugin is a long-running program that the daemon starts once and sends
check requests to, as JSON lines on its stdin and stdout. One process
serves every service that uses it, and it can keep connections and state
between checks.

emfor ships one plugin program, `emfor-check`. Its argument selects the
check:

```toml
[plugins."pgsql"]
command = ["/usr/local/libexec/emfor-check", "pgsql"]

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
check_options = { user = "emfor", database = "postgres", role = "primary", password_file = "/etc/emfor/pg.pass" }
```

| Argument | Up when |
| --- | --- |
| `redis` | `PING` answers `PONG` (after `AUTH` when a password is set) |
| `mysql` | The login succeeds and `SELECT 1` returns 1 (tested with MariaDB) |
| `pgsql` | The login succeeds and `SELECT 1` returns 1 |
| `ldap` | A simple bind succeeds |
| `dns` | One query gets an expected response code (and records, with `expect_answer`) |
| `radius` | A Status-Server (or Access-Request) gets an accept signed with the shared secret |
| `sip` | An OPTIONS request gets a final status code in `expect_status` (default 200 to 499) |

Main options: `user`, `password_file` (never the password itself; read on
every check), `database` (`pgsql`), `role` (`master`/`replica` for redis,
`primary` for MySQL and PostgreSQL, so that `failover` follows the primary),
and `tls`, `starttls` (`ldap`), `tls_verify`, `server_name`, `ca_file`.
The `dns` plugin takes `name`, `type`, `transport`, `recursion_desired`,
`expect_rcode` and `expect_answer` instead, and the `radius` plugin
`secret_file`, `request`, `application`, `user`, `password_file`,
`reject_is_up`, `nas_identifier` and `require_message_authenticator`, and
the `sip` plugin `transport`, `tls` (with the TLS keys), `uri`, `from` and
`expect_status`.
[docs/deploy/plugins.md](../deploy/plugins.md) lists every option and
reason.

`emfor check-config` and every reload start the plugins and have them
validate each service's `check_options`, so mistakes show up before the
config is applied.

When a plugin exits, its pending checks become `error` (`plugin exited`),
and the daemon restarts it after 1 s, then 2 s, 4 s … up to 30 s; until
then its checks are `error` (`plugin <name> unavailable`). Errors do not
change backend states, so a crashing plugin does not take backends down.
Lines a plugin writes to stderr are logged at `info`.

To write your own plugin, see
[docs/plugins/protocol-v1.md](../plugins/protocol-v1.md); the `emfor-plugin`
crate implements the protocol for Rust.

## Scheduling

Each backend is checked on its own schedule, and never by two checks at
once:

- The first check after a start or reload begins at a random moment within
  `min(interval, 200ms)`.
- While the backend is `unknown` and checks return `up` or `down`, the next
  check follows `min(interval, 200ms)` after the previous one ends. With
  the defaults (`rise = 2`), backends are known within about half a second.
- Otherwise the next check starts `interval` after the previous one ends,
  shifted at random by up to `jitter` (10% by default) either way.

Because the interval is counted from the end of a check, a slow backend is
not checked more often than every `interval`.

## Rise, fall and errors

- `up`: the success counter goes up and the failure counter resets; at
  `rise` the backend turns `healthy`.
- `down`: the failure counter goes up and the success counter resets; at
  `fall` the backend turns `unhealthy`.
- `error`: no counter and no state changes. When a backend has had only
  errors for longer than `fall × interval`, it goes back to `unknown`.

Time to detect a failure is about `fall × interval` (6 s with the
defaults); time to recover is about `rise × interval` (4 s). Every state
change is logged at `info` with the service, backend, old and new state,
and the reason.

## Reasons

A `down` or `error` result carries a reason, shown by `emfor status`, in
logs and in the state history. Before it is stored, the reason is made
safe: tab, CR and LF become `\t`, `\r` and `\n`; other control characters
become `\u{1b}` and the like; and a reason longer than 512 characters is
cut and ends in `…`.

## Latency

For every backend the daemon measures how long each `up` check takes and
keeps a smoothed value (each new sample weighs 5%). `emfor status` and
`/metrics` show it, and `balance = "fastest"` uses it. Choose a check that
resembles real traffic when you rely on it.
