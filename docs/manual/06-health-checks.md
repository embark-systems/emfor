# 6. Health checks: tcp, http and exec

[Contents](README.md) · Previous: [5. Configuring emfor](05-configuring.md) · Next: [7. Plugins](07-plugins.md)

A service's `check` says how each backend is tested, and its
`check_options` table configures that test. emfor has three built-in checks
and runs plugins for everything else.

| `check` | Tests | Good for |
| --- | --- | --- |
| `tcp` | A TCP connection; optionally TLS, and a send/expect exchange | Any TCP service; greeting protocols such as SMTP or IMAP |
| `http` | One `GET` request and its status code (and optionally its body) | Web services and health endpoints |
| `exec` | A program run once per check | Existing scripts and Nagios plugins, infrequent checks |
| a plugin name | A long-running plugin program | Databases and protocols ([chapter 7](07-plugins.md)) |

Every check ends in `up`, `down` or `error`
([chapter 3](03-concepts.md#check-results)). A check that has not finished
within the service's `timeout` is `down` with the reason `timeout`, except a
plugin check, which is `error` (`plugin did not answer within timeout`):
there the plugin is late, not the backend.

## tcp

Without options, a backend is `up` when it accepts a TCP connection; emfor
closes the connection at once.

```toml
[services."redis-raw.svc.emfor"]
check = "tcp"
backends = ["10.0.5.11:6379", "10.0.5.12:6379"]
```

A refused or unreachable connection is `down`, also when this host has no
route to the backend or its own firewall blocks it: the applications on this
host could not reach it either. Only a shortage on this host (open files,
buffers, memory, local ports) is an `error`.

| Option | Type | Default | Meaning |
| --- | --- | --- | --- |
| `send` | string | none | Bytes written once after connecting. TOML escapes such as `\r\n` are sent as those bytes. Needs `expect`. |
| `expect` | string | none | Bytes that must appear, case-sensitively, in what the backend sends (up to 16384 bytes are read). Not empty, at most 16384 bytes. |
| `tls` | boolean | `false` | Do a TLS handshake right after connecting. Never falls back to plaintext. |
| `tls_verify` | boolean | `true` | `false` accepts any certificate (the connection is still encrypted). Needs `tls = true`. |
| `server_name` | string | the backend's IP | The name (or IP) the certificate must match; sent as SNI when it is a name. Needs `tls = true`. |
| `ca_file` | absolute path | the system CAs | A PEM file of CA certificates to trust instead. It must hold at least one certificate usable as a CA when the config is loaded. Needs `tls = true`. |

Examples:

```toml
[services."smtp.svc.emfor"]
check = "tcp"
backends = ["10.0.6.11:25", "10.0.6.12:25"]
check_options = { expect = "220" }            # the greeting

[services."memcache.svc.emfor"]
check = "tcp"
backends = ["10.0.7.11:11211"]
check_options = { send = "stats\r\n", expect = "STAT pid" }

[services."imap.svc.emfor"]
check = "tcp"
backends = ["10.0.8.11:993"]
check_options = { tls = true, server_name = "imap.internal", expect = "* OK" }
```

Typical reasons:

| Reason | Meaning |
| --- | --- |
| `Connection refused (os error 111)` | Nothing listens on the port (the connect error, without a prefix). |
| `timeout` | No connection, or no matching answer, within `timeout`. |
| `expect: got "421 too busy\r\n"` | The answer did not contain `expect`; the first 64 bytes received, escaped. |
| `send: ...`, `expect: ...` | An I/O error while sending or reading. |
| `tls: no answer to the TLS handshake (does the port speak TLS?)` | The port does not speak TLS. |
| `tls: connection closed during the TLS handshake (does the server have a certificate?)` | The server closed instead of answering, as Active Directory does on port 636 without a certificate. |
| `tls: ...` | Another TLS failure, such as a certificate that does not match. |

## http

One `GET` request per check, to the backend's IP and port. Redirects are not
followed, and no proxy is used.

```toml
[services."api.svc.emfor"]
check = "http"
backends = ["10.0.0.11:8080", "10.0.0.12:8080"]

[services."api.svc.emfor".check_options]
path = "/healthz"
host_header = "api.internal"
expect_status = ["200-299"]
expect_body = '"status":"UP"'
```

| Option | Type | Default | Meaning |
| --- | --- | --- | --- |
| `scheme` | `"http"` or `"https"` | `"http"` | The protocol. |
| `path` | string starting with `/` | `"/"` | The request path, with a query string if you want one. `#`, `\` and control characters are refused; a space or other special character is sent percent-encoded. |
| `host_header` | string | none | The `Host` header: a host name or IP address with an optional `:port`, IPv6 in brackets (`api.internal`, `api.internal:8443`, `[2001:db8::1]`). The connection still goes to the backend; with `https` a host name is also the TLS server name. |
| `expect_status` | non-empty list | `["200-399"]` | Status codes that count as `up`: codes (`200` or `"200"`) or inclusive ranges (`"200-299"`), all within 100–599. |
| `expect_body` | string | none | Bytes that must appear, case-sensitively, in the body. Up to 65536 bytes of the body are read. Not empty, at most 16384 bytes. |
| `tls_verify` | boolean | `true` | With `https`: `false` accepts any certificate. |
| `ca_file` | absolute path | none | With `https`: a PEM file of the CA certificates to trust, for a server whose certificate a private CA signed. Only this file is trusted, not the system's CA certificates (like `curl --cacert`). Each check notices a replaced or rewritten file and reads it again, so a new file needs no reload; one that cannot be read or has no certificate fails validation, or makes the check `error` if it breaks later. Needs `scheme = "https"`; with `tls_verify = false` it has no effect. |

What the results look like:

| Reason | Meaning |
| --- | --- |
| `unexpected status 503` | The status is not in `expect_status`. The body is not read. |
| `expect_body: got "<first 64 bytes>"` | The status matched but the text was not in the body. |
| `body: ...` | The body could not be read. |
| `error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)` | The connection failed. The reason never contains the request URL, so a token in `path` does not leak into logs. |
| `timeout` | No complete answer within `timeout`. |
| `cannot build http client: ...; https checks need the system CA certificates, or a ca_file, unless tls_verify = false` (`error`) | An `https` check with verification on a host without CA certificates (`ca-certificates` on Debian and RHEL). Plain `http` needs no CA certificates since 0.7.0. Put the private CA in a `ca_file` instead of installing it system-wide. |

Use `expect_body` for health endpoints that answer 200 whatever their state and
put the state in the body. The latency that `balance = "fastest"` uses then
includes reading the body.

## exec

Runs a program once per check, without a shell, with `/` as its working
directory and no standard input. The backend is passed in the environment.

```toml
[services."report.svc.emfor"]
check = "exec"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
interval = "10s"
timeout = "5s"
check_options = { command = ["/usr/local/libexec/check_replication.sh", "--max-lag", "60"] }
```

A matching program:

```sh
#!/bin/sh
# /usr/local/libexec/check_replication.sh: emfor sets EMFOR_IP, EMFOR_PORT, EMFOR_SERVICE.
lag=$(psql -h "$EMFOR_IP" -p "$EMFOR_PORT" -U monitor -Atc \
      "select coalesce(extract(epoch from now() - pg_last_xact_replay_timestamp()), 0)::int" postgres) \
  || { echo "cannot query $EMFOR_IP"; exit 2; }
[ "$lag" -le "$2" ] || { echo "replication ${lag}s behind"; exit 1; }
exit 0
```

(The script is an example; it has not been run against a real PostgreSQL.)

| The program | Result |
| --- | --- |
| exits 0 | `up` |
| exits 1 or 2 | `down`; the reason is the first line of its standard output (cut to 200 characters), or `exit code N` when it printed nothing |
| exits with another code | `error`, with the reason `exit code N` or `exit code N: <first line>` |
| is killed by a signal | `error` (`killed by signal N`) |
| cannot be started | `error` (`cannot run <program>: ...`) |
| still runs at `timeout` | `down` (`timeout`); the program and every process it started are killed |

This is the Nagios convention, except that **1 (WARNING) counts as down** and
3 (UNKNOWN) as an error. Standard error is discarded.

| Option | Type | Meaning |
| --- | --- | --- |
| `command` | list of strings, required | The program and its arguments. The first element must be an absolute path to an executable file; no element may contain NUL. |
| `env_pass` | list of names | Variables of the daemon's environment that the program also gets ([below](#the-environment-of-check-programs)). |

Starting a process for every check of every backend is costly. For frequent
checks of many backends, write a [plugin](07-plugins.md#writing-your-own-plugin)
instead. To use an existing Nagios plugin, which wants `-H` and `-p` on its
command line, see the wrapper in
[chapter 14](14-integrations.md#using-a-nagios-plugin-as-a-check).

## The environment of check programs

Since 0.7.0 an `exec` program and a plugin **do not get the daemon's
environment**. They get only:

- `PATH`, `LANG`, `TZ` and every `LC_*` variable, with the values the daemon
  has;
- for `exec`: `EMFOR_IP`, `EMFOR_PORT` and `EMFOR_SERVICE`;
- the variables named in `env_pass`.

So a secret in the daemon's environment (one a unit file sets, say) does not
reach a check program. A program that needs another variable, such as `HOME`
or `https_proxy`, gets it when you list its name:

```toml
[services."report2.svc.emfor"]
check = "exec"
backends = ["10.0.3.1:5432"]
check_options = { command = ["/usr/local/libexec/check_replication.sh", "--max-lag", "60"], env_pass = ["HOME", "PGSSLROOTCERT"] }

[plugins."pgsql"]
command = ["/usr/bin/emfor-check", "pgsql"]
env_pass = ["https_proxy"]
```

- `env_pass` lists **names**, never values. A name the daemon does not have is
  skipped without an error. To give a program a value that is not in the
  daemon's environment, set it in the unit file (`Environment=`) and list its
  name.
- A name is letters, digits and `_`, not starting with a digit, and appears
  once. For `exec` it must not be `EMFOR_IP`, `EMFOR_PORT` or `EMFOR_SERVICE`
  (`env_pass: EMFOR_IP is set by emfor`).
- `emfor-check` reads `SSL_CERT_FILE` and `SSL_CERT_DIR` to find the system CAs
  for a TLS check without `ca_file`. If you rely on them, add them to the
  plugin's `env_pass`, or set `ca_file`.
- Changing a plugin's `env_pass` restarts the plugin on reload.
- In a peer group, `env_pass` is part of the check definition the members
  compare: hosts must list the same names for a service.
- `emfor check-config` does not warn about a variable a program needs but does
  not get. Before upgrading from 0.6, find the programs that read `HOME`, a
  proxy variable or `LD_LIBRARY_PATH` ([chapter 15](15-upgrading.md)).

This was checked in a container: with `env_pass = ["HOME"]` and a daemon
started with `HOME=/root MY_SECRET=hunter2`, the check program saw
`HOME=/root` and no `MY_SECRET`.

## Scheduling

Each backend is checked on its own schedule, and never by two checks at once:

- The first check after a start or a reload begins at a random moment within
  `min(interval, 200ms)`.
- While the backend is `unknown` and the checks return `up` or `down`, the next
  check follows `min(interval, 200ms)` after the previous one ends, for at
  most `rise + fall` checks. With the defaults every backend is known within
  about half a second. A backend whose results keep changing, so that neither
  count is reached, stays `unknown` and is then checked every `interval`.
- Otherwise the next check starts `interval` after the previous one **ended**,
  shifted at random by up to `jitter` (10% by default) either way. A slow
  backend is therefore not checked more often than every `interval`.
- In a peer group, a member that is not an assigned checker of a backend
  checks it less often ([chapter 11](11-peer-groups.md#how-the-votes-work)).

## Rise, fall and errors

- `up`: the success count goes up, the failure count resets; at `rise` the
  backend turns `healthy`.
- `down`: the failure count goes up, the success count resets; at `fall` the
  backend turns `unhealthy`.
- `error`: nothing changes. When a backend has had only errors for longer than
  `fall × interval`, it goes back to `unknown`.

Time to notice a failure is about `fall × interval` (6 s with the defaults);
time to notice a recovery about `rise × interval` (4 s). Every state change is
logged at `info` with the service, the backend, the old and new state and the
reason:

```
INFO emfor_daemon::schedule: backend 127.0.0.2:8080 of web.svc.emfor changed from healthy to unhealthy (error sending request: ...) service=web.svc.emfor backend=127.0.0.2:8080 from=healthy to=unhealthy reason="..."
```

## Reasons are made safe

Before a reason is stored, emfor escapes it: tab, CR and LF become `\t`, `\r`
and `\n`; other control characters become `\u{1b}` and the like; so do the
invisible format characters that can hide or reorder text (the bidirectional
controls such as U+202E, the zero-width characters, the soft hyphen, the tag
characters). A reason longer than 512 characters is cut and ends in `…`. A
backend therefore cannot make `emfor status` or a log line show something
else than what happened.

## Latency

For every backend, emfor measures how long each `up` check takes and keeps a
smoothed value (each new sample weighs 5%). `emfor status` and `/metrics` show
it, and `balance = "fastest"` uses it. If you rely on it, choose a check that
looks like real traffic.

## Choosing interval, timeout, rise and fall

| Goal | Settings |
| --- | --- |
| The defaults: notice a failure in about 6 s | `interval = "2s"`, `timeout = "1s"`, `rise = 2`, `fall = 3` |
| Notice faster, on a fast network | `interval = "500ms"`, `timeout = "300ms"`, `fall = 3` (about 1.5 s) |
| An expensive check (a script, a query) | `interval = "10s"` or more, `timeout` well below it |
| A backend that is slow now and then | Raise `timeout` (up to `interval`) and `fall` |
| A flapping backend ([chapter 16](16-troubleshooting.md#a-backend-flaps)) | Raise `rise` and `fall`; check `timeout` against the normal answer time |

Limits: `interval` from `100ms` to `24h`; `timeout` at least `10ms` and at most
`interval`; `rise` and `fall` at least 1; `jitter` from `0%` to `50%`.
