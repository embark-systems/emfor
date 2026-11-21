# 7. Plugins and emfor-check

[Contents](README.md) · Previous: [6. Health checks](06-health-checks.md) · Next: [8. How a backend is chosen](08-choosing-a-backend.md)

A **plugin** is a long-running program that the daemon starts once and sends
check requests to. One process serves every service that uses it, and can
keep state between checks. emfor ships one plugin program, `emfor-check`,
with thirteen checks; you can also [write your own](#writing-your-own-plugin).

## Using a plugin

Declare the plugin under `[plugins]`, then name it as a service's `check`:

```toml
[plugins."pgsql"]
command = ["/usr/bin/emfor-check", "pgsql"]

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
check_options = { user = "emfor", database = "postgres", role = "primary", password_file = "/etc/emfor/secrets/pg.pass" }
```

- The plugin's name is 1–63 characters of `a-z`, `0-9`, `_` and `-`, and not
  `tcp`, `http` or `exec`. It does not have to match the argument: a plugin
  named `pg-main` can run `emfor-check pgsql`.
- `command` is the program and its arguments, run without a shell. The first
  element is an absolute path to an executable. `emfor-check` is
  `/usr/bin/emfor-check` in the packages and `/usr/local/bin/emfor-check` in
  the FreeBSD port; after a build from source it is wherever you installed it
  ([chapter 4](04-installing.md#building-from-source)).
- `env_pass` lists variables the plugin gets besides `PATH`, `LANG`, `TZ` and
  `LC_*` ([chapter 6](06-health-checks.md#the-environment-of-check-programs)).
- Every declared plugin is started, used or not.
- The port always comes from the backend address; no plugin has a `port`
  option.

### What the daemon does with a plugin

| When | What happens |
| --- | --- |
| Start, reload, `emfor check-config` | The plugin is started (or kept, when its `command` and `env_pass` did not change) and asked to **validate** each service's `check_options`. Errors appear at `services."<name>".check_options.<key>`, so a wrong option is found before the config is applied. |
| Each check | The daemon sends a request with the backend and a timeout a little below the service's `timeout` (`timeout` minus 10%, at most 100 ms less). |
| The plugin exits | Its pending checks are `error` (`plugin exited`), and it is started again after 1 s, then 2 s, 4 s, 8 s, 16 s, at most 30 s. Until then its checks are `error` (`plugin <name> unavailable`). The delay starts over once it ran for 30 s. |
| It leaves 3 requests in a row unanswered | It is killed and restarted (`plugin <name> stopped answering: 3 requests in a row got no answer`). |
| It stops reading its input and 1024 requests are waiting | New checks are `error`: `plugin <name> busy: it is not reading its requests`. |
| It writes to standard error | The lines are logged at `info` (`plugin <name>: <line>`), at most 100 at once and 10 a second after that. |
| It stops or is stopped | Its whole process group is killed, so programs it started do not linger. |

A crashing plugin never marks backends down: its checks are `error`, which
does not count. But a backend whose checks give nothing but errors for longer
than `fall × interval` goes back to `unknown`, so a plugin that keeps crashing
leaves its backends `unknown` rather than `healthy`. Watch `emfor_plugin_up`
and `emfor_plugin_restarts_total` ([chapter 12](12-operating.md#metrics)).

## Options shared by the emfor-check checks

### TLS

Most checks take the same TLS keys:

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `tls` | boolean | `false` | Speak TLS from the first byte (LDAPS, SMTPS, rediss and so on). |
| `tls_verify` | boolean | `true` | `false` accepts any certificate. Needs TLS on. |
| `server_name` | string | the backend IP | The name the certificate must match; also sent as SNI. Needs TLS on. |
| `ca_file` | absolute path | the system CAs | A PEM file of CA certificates to trust. Checked when the config is loaded, re-read when it changes. Needs TLS on. |

Setting `tls_verify`, `server_name` or `ca_file` without TLS is an error
(`requires tls = true`; for ldap and smtp `requires tls = true or starttls = true`).

Typical TLS reasons: `tls: no answer to the TLS handshake (does the port speak
TLS?)`, `tls: connection closed during the TLS handshake (does the server have
a certificate?)`, and, as an `error`, `tls: no system CA certificates found;
set ca_file`.

### Passwords

Passwords are never in the config. `password_file` names a file that holds
one, and:

- must be an absolute path;
- is read again at **every** check, so you can change the password without a
  reload;
- loses one trailing line feed (`\n` or `\r\n`), and must then be non-empty
  and valid UTF-8;
- when it cannot be read at check time, the result is `error`
  (`password_file: entity not found` and the like), not `down`.

Keep the file readable only by its owner and the daemon's group:

```sh
# install -d -m 0750 -o root -g emfor /etc/emfor/secrets
# install -m 0640 -o root -g emfor /dev/null /etc/emfor/secrets/pg.pass
# printf '%s\n' 'the-password' > /etc/emfor/secrets/pg.pass
```

`emfor check-config` and the daemon warn about two things (warnings, never
errors):

- a `password_file` (or `secret_file`, `tls_key_file`) that every user can read
  or that others can change:
  `password_file /etc/emfor/redis.pass (mode 0644) can be read by every user on this host; used by cache.svc.emfor. Limit it to its owner and the daemon's group, for example chmod 0640`;
- a password sent without TLS, for `redis`, `ldap` (unless `starttls`),
  `mysql`, `pgsql` and `amqp`:
  `service cache.svc.emfor: check_options has a password_file but tls is not true: the password is sent in clear text, and anyone who can see the traffic can read it`.

### Roles and replication lag

The database checks can follow the primary, so that `balance = "failover"`
always gives the current primary:

| Check | `role` values | `max_lag` |
| --- | --- | --- |
| `redis` | `master`, `replica` | with `role = "replica"`: seconds the replica may be behind (`master_last_io_seconds_ago`) |
| `mysql` | `primary` (`@@global.read_only` is 0), `replica` (both replication threads run) | with `role = "replica"`: `Seconds_Behind_Source` |
| `pgsql` | `primary` (not in recovery), `replica` (in recovery with a WAL receiver) | with `role = "replica"`: replay lag |
| `mongodb` | `primary` (a primary, a standalone or a mongos), `secondary` | none |

`max_lag` is a whole number from 1 to 86400. A replica whose lag cannot be
read is `down` (`replication: lag unknown`).

## The checks

`emfor-check` takes the check as its argument:
`emfor-check redis|mysql|pgsql|ldap|dns|radius|sip|diameter|smtp|amqp|mongodb|grpc|quic`.
Every check opens a new connection and closes it again. Two exceptions:
QUIC shares one UDP socket per address family, and `diameter` sends its
Disconnect-Peer-Request after the result is decided, in the background, so
its connection can outlive the check by a fraction of a second. The tables below list every option; the TLS keys
are those [above](#tls).

### redis

`up` when `PING` answers `PONG`, after `AUTH` when a password is set, and the
role (and lag) match when `role` is set.

| Option | Default | Rule |
| --- | --- | --- |
| `user` | none | Needs `password_file`. Sent as the AUTH user name (Redis 6 ACLs). |
| `password_file` | none | Enables `AUTH`. |
| `role` | any | `master` or `replica`. A replica must have its link to the master up. |
| `max_lag` | none | 1–86400; needs `role = "replica"`. |
| `tls`, `tls_verify`, `server_name`, `ca_file` | | |

Reasons: `auth: ...` (the password is shown as `***` if the server echoes it),
`query: NOAUTH Authentication required.`, `role is replica, want master`,
`replication: link to master is sync`, `replication: 16s behind, max 15s`.

```toml
[plugins."redis"]
command = ["/usr/bin/emfor-check", "redis"]

[services."cache.svc.emfor"]
check = "redis"
balance = "failover"
backends = ["10.0.5.11:6379", "10.0.5.12:6379"]
check_options = { role = "master", tls = true, server_name = "cache.internal", password_file = "/etc/emfor/secrets/redis.pass" }
```

### mysql

`up` when the login succeeds and `SELECT 1` returns 1 (and the role matches).
Tested in emfor's suite with MariaDB and MySQL 8.0 and 8.4.

| Option | Default | Rule |
| --- | --- | --- |
| `user` | required | No NUL. |
| `password_file` | none (no password) | |
| `role` | any | `primary` or `replica`. |
| `max_lag` | none | 1–86400; needs `role = "replica"`. |
| `tls`, `tls_verify`, `server_name`, `ca_file` | | |

Reasons: `auth: ERROR 1045 (28000): Access denied ...`,
`tls: server does not support TLS`, `role is read-only, want primary`,
`replication: IO thread is Connecting: ...`, `replication: 31s behind, max 30s`.

### pgsql

`up` when the login succeeds and `SELECT 1` returns 1 (and the role matches).
The connection uses `application_name = emfor-check`.

| Option | Default | Rule |
| --- | --- | --- |
| `user` | required | No NUL. |
| `database` | required | No NUL. |
| `password_file` | none | No NUL in the password. |
| `role` | any | `primary` or `replica`. |
| `max_lag` | none | 1–86400; needs `role = "replica"`. |
| `tls`, `tls_verify`, `server_name`, `ca_file` | | With TLS the connection requires TLS; without it, TLS is off. |

Reasons: `auth: 28P01: password authentication failed for user "emfor"`,
`role is standby, want primary`, `role is primary, want replica`,
`replication: no WAL receiver`, `replication: 31s behind, max 30s`.

```toml
[plugins."pgsql"]
command = ["/usr/bin/emfor-check", "pgsql"]

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
on_all_down = "closed"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
check_options = { user = "emfor", database = "postgres", role = "primary", tls = true, ca_file = "/etc/emfor/ca.pem", server_name = "db.internal", password_file = "/etc/emfor/secrets/pg.pass" }

[services."db-ro.svc.emfor"]
check = "pgsql"
backends = ["10.0.3.1:5432", "10.0.3.2:5432", "10.0.3.3:5432"]
check_options = { user = "emfor", database = "postgres", role = "replica", max_lag = 30, password_file = "/etc/emfor/secrets/pg.pass" }
```

### ldap

`up` when an LDAPv3 simple bind succeeds (anonymous without `user`).

| Option | Default | Rule |
| --- | --- | --- |
| `user` | anonymous | The bind DN; needs `password_file`. |
| `password_file` | none | Needs `user`. |
| `tls` | `false` | LDAPS. |
| `starttls` | `false` | StartTLS on a plain connection; not together with `tls`. |
| `tls_verify`, `server_name`, `ca_file` | | Need `tls` or `starttls`. |

Reasons: `auth: 49 invalidCredentials`, `auth: 8 strongerAuthRequired`,
`tls: StartTLS refused: 2 protocolError`,
`connect: connection closed by the server`. Against Active Directory it was
tried only without a certificate (the TLS reason above).

### dns

`up` when one query gets a response code in `expect_rcode` (and, with
`expect_answer`, a record of the queried type).

| Option | Default | Rule |
| --- | --- | --- |
| `name` | required | A domain name, or `.`. |
| `type` | `A` | `A`, `AAAA`, `CNAME`, `MX`, `NS`, `PTR`, `SOA`, `SRV`, `TXT`. |
| `transport` | `udp` | `udp` or `tcp`. Over UDP the query is sent again once at half the timeout. |
| `recursion_desired` | `true` | The RD flag. |
| `expect_rcode` | `["NOERROR"]` | Any of `NOERROR`, `FORMERR`, `SERVFAIL`, `NXDOMAIN`, `NOTIMP`, `REFUSED`. |
| `expect_answer` | `false` | Require at least one record of `type`. |

Reasons: `rcode: SERVFAIL`, `answer: no A records`, with
`(truncated; set transport = "tcp")` when the answer was truncated.

```toml
[plugins."dns"]
command = ["/usr/bin/emfor-check", "dns"]

[services."resolver.svc.emfor"]
check = "dns"
backends = ["10.0.0.53:53", "10.0.1.53:53"]
check_options = { name = "example.com", type = "A", expect_answer = true }
```

### radius

`up` when a Status-Server (default) or an Access-Request gets the right
answer, signed with the shared secret.

| Option | Default | Rule |
| --- | --- | --- |
| `secret_file` | required | The shared secret, read at every check. |
| `request` | `status-server` | Or `access-request`. |
| `application` | `auth` | `auth` (Access-Accept is up) or `acct` (Accounting-Response is up; only with `status-server`). |
| `user` | none | 1–253 bytes; required with `access-request`, not allowed otherwise. |
| `password_file` | none | At most 128 bytes; required with `access-request`, not allowed otherwise. Sent as PAP. |
| `reject_is_up` | `false` | With `access-request`: an Access-Reject counts as up (the server answers, the test user is just refused). |
| `nas_identifier` | `emfor` | 1–253 bytes. |
| `require_message_authenticator` | `false` | A response without a Message-Authenticator is down. |

Reasons: `auth: Access-Reject: <Reply-Message>`,
`auth: response authenticator does not match (wrong secret?)`,
`query: unexpected response code 2`.

### sip

`up` when an OPTIONS request gets a final status in `expect_status`.

| Option | Default | Rule |
| --- | --- | --- |
| `transport` | `udp` | `udp` or `tcp`; must be `tcp` with `tls = true`. |
| `uri` | `sip:<ip>:<port>` (`sips:` with TLS) | A `sip:` or `sips:` URI, at most 256 bytes, no spaces, `<` or `>`. |
| `from` | `sip:emfor@emfor.invalid` | Same rule. |
| `expect_status` | `["200-499"]` | Strings `"NNN"` or `"lo-hi"`, codes 200–699 (numbers are not accepted). |
| `tls`, `tls_verify`, `server_name`, `ca_file` | | |

Reasons: `status: 503 Service Unavailable`,
`query: response does not match the request`.

### diameter

`up` when a Capabilities-Exchange (CER/CEA) and a Device-Watchdog (DWR/DWA)
both succeed with a 2xxx Result-Code. A Disconnect-Peer-Request follows.

| Option | Default | Rule |
| --- | --- | --- |
| `origin_host`, `origin_realm` | required | DiameterIdentity: 1–255 of `A-Za-z0-9.-`. |
| `auth_application_ids`, `acct_application_ids` | `[]` | Lists of integers; at least one application in total. |
| `vendor_specific_application_ids` | `[]` | Tables with `vendor_id` and one of `auth_application_id`, `acct_application_id`. |
| `supported_vendor_ids` | `[]` | List of integers. |
| `host_ip_address` | the local address | An IP address. |
| `tls`, `tls_verify`, `server_name`, `ca_file` | | |
| `tls_cert_file`, `tls_key_file` | none | A client certificate (PEM) and its key, together; need `tls = true`. Checked to match when the config is loaded. |

Reasons: `cea: 3010 DIAMETER_UNKNOWN_PEER`,
`cea: 5010 DIAMETER_NO_COMMON_APPLICATION`, `dwa: 3004 DIAMETER_TOO_BUSY`,
`cea: connection closed by peer`.

### smtp

`up` when the greeting is 220 and `EHLO` answers 250; with `starttls`, also
after STARTTLS and a second `EHLO`. No login and no mail is sent.

| Option | Default | Rule |
| --- | --- | --- |
| `helo` | the local address literal | A host name, 1–255 of `A-Za-z0-9.-`. |
| `tls` | `false` | Implicit TLS (port 465). |
| `starttls` | `false` | Not together with `tls`. |
| `tls_verify`, `server_name`, `ca_file` | | Need `tls` or `starttls`. |

Reasons: `greeting: 554 5.3.2 no service`, `ehlo: 550 5.7.1 go away`,
`tls: STARTTLS not offered`.

### amqp

`up` when an AMQP 0-9-1 login (SASL PLAIN) succeeds and the vhost opens.

| Option | Default | Rule |
| --- | --- | --- |
| `user` | required | 1–255 bytes, no NUL. |
| `password_file` | required | No NUL; the login must fit in AMQP's 4096-byte first frame. |
| `vhost` | `/` | 1–255 bytes. |
| `tls`, `tls_verify`, `server_name`, `ca_file` | | |

Reasons: `auth: 403 ACCESS_REFUSED - ...`, `vhost: 530 NOT_ALLOWED - ...`,
`auth: server does not offer PLAIN`.

### mongodb

`up` when `hello` answers, the SCRAM-SHA-256 login succeeds (when `user` is
set) and the role matches.

| Option | Default | Rule |
| --- | --- | --- |
| `user` | no login | 1–255 bytes; needs `password_file`. |
| `password_file` | none | Needs `user`; printable ASCII only. |
| `auth_source` | `admin` | Needs `user`. 1–64 of `A-Za-z0-9_-`. |
| `role` | any | `primary` or `secondary`. |
| `tls`, `tls_verify`, `server_name`, `ca_file` | | |

Reasons: `auth: AuthenticationFailed: Authentication failed.`,
`role is secondary, want primary`. No password warning: SCRAM does not send
the password.

### grpc

`up` when the standard health service (`grpc.health.v1.Health/Check`) answers
`SERVING`.

| Option | Default | Rule |
| --- | --- | --- |
| `service` | `""` (the whole server) | At most 256 bytes. |
| `tls`, `tls_verify`, `server_name`, `ca_file` | | With TLS, ALPN `h2` is required. |

Reasons: `status: NOT_SERVING`, `grpc: 12 UNIMPLEMENTED`,
`tls: server did not select h2 (ALPN)`.

### quic (and HTTP/3)

`up` when a QUIC handshake completes with a verified certificate and an ALPN
from `alpn`; with `http3 = true`, a `GET` must also get a status in
`expect_status`. QUIC always uses TLS, so there is no `tls` key.

| Option | Default | Rule |
| --- | --- | --- |
| `alpn` | `["h3"]` | Non-empty list of 1–255 byte strings. |
| `tls_verify`, `server_name`, `ca_file` | | As above, without needing `tls`. |
| `http3` | `false` | Send an HTTP/3 `GET`; needs `"h3"` in `alpn`. |
| `path` | `/` | Needs `http3 = true`. |
| `host_header` | none | Needs `http3 = true`. |
| `expect_status` | `["200-399"]` | Codes 100–599 or ranges; needs `http3 = true`. |
| `expect_body` | none | 1–16384 bytes; needs `http3 = true`. |

Reasons: `tls: no application protocol (ALPN)`, `quic: closed by peer: 2`,
`unexpected status 503`, `expect_body: got "..."`.

```toml
[plugins."quic"]
command = ["/usr/bin/emfor-check", "quic"]

[services."edge.svc.emfor"]
check = "quic"
backends = ["10.0.4.11:443", "10.0.4.12:443"]
check_options = { server_name = "edge.internal", ca_file = "/etc/emfor/ca.pem", http3 = true, path = "/healthz" }
```

### What has been tested

`emfor-check`'s checks are tested in the project's suite, partly against real
servers in containers. Not every combination was run: the `ldap` check against
Active Directory only without a certificate, and the warnings about passwords
sent without TLS were not run against real servers. [docs/deploy/plugins.md](../deploy/plugins.md)
has more detail on each check.

## Writing your own plugin

A plugin is any program that speaks **protocol version 1**: one JSON object
per line, requests on its standard input, answers on its standard output. The
full description is [docs/plugins/protocol-v1.md](../plugins/protocol-v1.md).

| Daemon sends | Plugin answers |
| --- | --- |
| `{"type":"hello","protocol":1}` | `{"type":"hello","protocol":1,"name":"...","version":"..."}` within 5 seconds |
| `{"type":"validate","id":7,"options":{...}}` | `{"type":"validate","id":7,"errors":[{"key":"...","message":"..."}]}` (an empty list when the options are fine) |
| `{"type":"check","id":8,"service":"x.svc.emfor","target":{"ip":"10.0.0.1","port":22},"options":{...},"timeout_ms":900}` | `{"type":"check","id":8,"result":"up"}`, or `"down"`/`"error"` with a `"reason"` |

Rules that matter:

- Answer every request, in any order, matched by `id`. Handle checks
  concurrently: a plugin that answers one at a time delays every other check.
- Flush after every line. A line may be at most 65536 bytes.
- Answer within `timeout_ms`; a later answer is ignored and the check is
  `error`.
- Ignore fields you do not know; later versions may add some.
- Write diagnostics to standard error; they reach the daemon's log.
- At end of input (the daemon stops you), finish what is running and exit.

A complete plugin in Python, which checks that a TCP connection succeeds:

```python
#!/usr/bin/env python3
"""emfor plugin (protocol 1): up when a TCP connection succeeds."""
import errno, json, socket, sys, threading

LOCAL = {errno.EMFILE, errno.ENFILE, errno.ENOBUFS, errno.ENOMEM, errno.EADDRNOTAVAIL}
lock = threading.Lock()

def send(msg):
    with lock:                                   # one whole line at a time
        sys.stdout.write(json.dumps(msg) + "\n")
        sys.stdout.flush()

def check(req):
    try:
        t = req["target"]
        socket.create_connection((t["ip"], t["port"]), timeout=req["timeout_ms"] / 1000).close()
        send({"type": "check", "id": req["id"], "result": "up"})
    except socket.timeout:
        send({"type": "check", "id": req["id"], "result": "down", "reason": "timeout"})
    except OSError as e:
        result = "error" if e.errno in LOCAL else "down"   # a problem of this host is an error
        send({"type": "check", "id": req["id"], "result": result, "reason": f"connect: {e.strerror}"})

for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    if msg["type"] == "hello":
        send({"type": "hello", "protocol": 1, "name": "tcpconnect", "version": "1.0"})
    elif msg["type"] == "validate":                # this plugin takes no options
        send({"type": "validate", "id": msg["id"],
              "errors": [{"key": k, "message": "unknown key"} for k in msg["options"]]})
    elif msg["type"] == "check":
        threading.Thread(target=check, args=(msg,), daemon=True).start()
```

```toml
[plugins."tcpconnect"]
command = ["/usr/bin/python3", "-u", "/usr/local/libexec/emfor-tcpconnect.py"]

[services."ssh.svc.emfor"]
check = "tcpconnect"
backends = ["10.0.0.11:22", "10.0.0.12:22"]
```

This plugin was run under emfor 0.7.0 in a Debian container, with one
listening and one closed port:

```console
# emfor status --service ssh.svc.emfor
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.9s ago, up 2.9s

ssh.svc.emfor  round-robin  on_all_down=open
  127.0.0.1:8080  healthy    2.7s ago  1.7ms
  127.0.0.1:8081  unhealthy  2.4s ago  -      connect: Connection refused
```

The log shows `plugin tcpconnect started (pid 47, tcpconnect 1.0)`, and
`check_options = { foo = 1 }` makes `emfor check-config` report
`services."ssh.svc.emfor".check_options.foo: unknown key`. A production plugin
should also survive a line it cannot parse.

### In Rust: the emfor-plugin crate

The `emfor-plugin` crate (in `crates/emfor-plugin`) implements the protocol:
you write `validate` and `check`, and `emfor_plugin::run` does the rest
(concurrency, timeouts, a panic in a check answered as
`error` `check panicked: ...`).

```rust
use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options, Plugin};
use tokio::net::TcpStream;

struct TcpConnect;

impl Plugin for TcpConnect {
    fn name(&self) -> &str { "tcpconnect" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn validate(&self, options: &Options) -> Vec<OptionError> {
        options.keys().map(|k| OptionError::new(k, "unknown key")).collect()
    }
    async fn check(&self, req: CheckRequest) -> CheckOutcome {
        match TcpStream::connect((req.ip, req.port)).await {
            Ok(_) => CheckOutcome::Up,
            Err(e) => CheckOutcome::Down(format!("connect: {e}")),
        }
    }
}

fn main() -> std::process::ExitCode {
    emfor_plugin::run(TcpConnect)
}
```

Before 1.0 the crate's API follows emfor's minor releases and may change
between them ([docs/compatibility.md](../compatibility.md)). This example was
not compiled for this manual; `emfor-check` itself is built on the crate.
