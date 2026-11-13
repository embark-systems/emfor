# Checking redis, MySQL, PostgreSQL, LDAP, DNS, RADIUS and SIP with emfor-check

`emfor-check` is the plugin program that ships with emfor. One executable holds seven plugins; its only argument picks which one runs:

| Argument | Checks | Up when |
| --- | --- | --- |
| `redis` | redis | `PING` answers `PONG` (after `AUTH` when a password is set) |
| `mysql` | MySQL, MariaDB | the login succeeds and `SELECT 1` returns 1 |
| `pgsql` | PostgreSQL | the login succeeds and `SELECT 1` returns 1 |
| `ldap` | OpenLDAP and other LDAPv3 servers | a simple bind succeeds (resultCode 0) |
| `dns` | DNS servers (unbound, BIND, PowerDNS, …) | the answer to one query has an expected response code, and records when asked for |
| `radius` | RADIUS servers (FreeRADIUS, …) | a Status-Server (or Access-Request) gets a correctly signed accept |
| `sip` | SIP proxies, registrars and SBCs (Kamailio, OpenSIPS, Asterisk, …) | an OPTIONS request gets a final response with an expected status code |

Each plugin runs as one long-lived child process of the emfor daemon, shared by every service that uses it (see [the plugin protocol](../plugins/protocol-v1.md)). Each check opens a new connection to the backend (for `dns` over UDP, a new socket) and closes it before answering, so a server that refuses new connections (for example because `max_connections` is reached) is reported as down.

## Installing

Build it with the rest of emfor and copy it next to the daemon's other helpers:

```sh
cargo build --release --bin emfor-check
install -m 0755 target/release/emfor-check /usr/local/libexec/emfor-check
```

Or with `cargo install`, which puts it in `<root>/bin`: `cargo install --locked --path crates/emfor-check --root /usr/local` (see [daemon.md](daemon.md#building-with-an-install-prefix)).

The database drivers (and the LDAP and DNS encoders) are linked into `emfor-check` only; the `emfor` daemon and libemfor do not contain them.

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

The plugin name in `[plugins."<name>"]` is your choice; the argument after the path decides the protocol. `emfor check-config` starts the plugins and has them validate every service's `check_options`, so mistakes are found before a reload.

### Options

These are the options of the database plugins `redis`, `mysql`, `pgsql` and `ldap`. The `dns`, `radius` and `sip` plugins have their own, see [DNS](#dns), [RADIUS](#radius) and [SIP](#sip).

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

### DNS

The `dns` plugin sends one DNS query to the backend and judges the answer. It is up when the response code is one of `expect_rcode` and, with `expect_answer = true`, the answer has at least one record of the asked type.

```toml
[plugins."dns"]
command = ["/usr/local/libexec/emfor-check", "dns"]

[services."resolver.svc.emfor"]
check = "dns"
backends = ["10.0.5.1:53", "10.0.5.2:53"]
check_options = { name = "ns-check.example.org", type = "A", expect_answer = true }
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `name` | string | required | The name to ask for; a trailing dot is optional, `.` is the root |
| `type` | string | `A` | `A`, `AAAA`, `CNAME`, `MX`, `NS`, `PTR`, `SOA`, `SRV` or `TXT` |
| `transport` | string | `udp` | `udp` or `tcp` |
| `recursion_desired` | boolean | `true` | Set the RD flag; `false` for an authoritative-only server |
| `expect_rcode` | array of strings | `["NOERROR"]` | Response codes that count as up: `NOERROR`, `FORMERR`, `SERVFAIL`, `NXDOMAIN`, `NOTIMP`, `REFUSED` |
| `expect_answer` | boolean | `false` | Also require a record of `type` in the answer |

Any other key is an error, including `tls`: DNS over TLS and DNS over HTTPS are not supported, and neither is DNSSEC validation.

- Ask for a name the server answers itself: a `local-zone` or local data on a resolver, or a name in a zone the server is authoritative for. A name the resolver has to look up elsewhere fails with `SERVFAIL` whenever the upstream servers or the network are down, which in a closed site may be the normal state. If you check a name that does not exist on purpose, set `expect_rcode = ["NOERROR", "NXDOMAIN"]`.
- `expect_answer = true` catches a server that answers `NOERROR` with no records (a name that exists without records of this type).
- Over UDP the plugin sends the query again once when no answer has come by half of the service `timeout`. It ignores datagrams that are not the answer to its query (another ID or question), so a late answer to an earlier check never counts.
- When an answer is too big for UDP the server sets the TC flag and may leave out records; with `expect_answer`, the reason then ends with `(truncated; set transport = "tcp")`. Use `transport = "tcp"` for large answers such as TXT records.
- A response code outside `expect_rcode` is down with `rcode: <name>`, for example `rcode: SERVFAIL`, `rcode: REFUSED` (the server does not serve this client or zone) or `rcode: NXDOMAIN`. Codes without a name show their number.
- With nothing listening on a UDP port, the reason is `connect: ` when the host sends ICMP port unreachable, and `timeout` when a firewall drops the packets.

### RADIUS

The `radius` plugin sends one RADIUS request over UDP and is up when the server answers with a response signed with the shared secret: Access-Accept on the authentication port, Accounting-Response on the accounting port.

```toml
[plugins."radius"]
command = ["/usr/local/libexec/emfor-check", "radius"]

[services."aaa.svc.emfor"]
check = "radius"
backends = ["10.0.6.1:1812", "10.0.6.2:1812"]
check_options = { secret_file = "/etc/emfor/radius.secret" }
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `secret_file` | absolute path | required | File holding the shared secret; one trailing newline is removed, and it is read on every check |
| `request` | string | `status-server` | `status-server` or `access-request` |
| `application` | string | `auth` | `auth` (port 1812) or `acct` (port 1813, Status-Server only) |
| `user` | string | none | `access-request` only, and required there: the test account |
| `password_file` | absolute path | none | `access-request` only, and required there: the test account's password |
| `reject_is_up` | boolean | `false` | `access-request` only: an Access-Reject counts as up |
| `nas_identifier` | string | `emfor` | NAS-Identifier sent with every request, 1 to 253 bytes |
| `require_message_authenticator` | boolean | `false` | A response without Message-Authenticator is down |

Any other key is an error, including `secret` and `password`. RadSec (RADIUS over TLS), CHAP, EAP and accounting records are not supported.

- **Status-Server** (RFC 5997, the default) is meant for monitoring: it needs no test account, writes no accounting record and raises no failed-login alarm. FreeRADIUS answers it only with `status_server = yes` in the `security` section of `radiusd.conf`; a server that does not answer it shows as `timeout`.
- **Access-Request** sends User-Name and a PAP User-Password, so it also checks what is behind the server (an LDAP or SQL user store). Every check is a login the server logs; use a longer `interval`. Set `reject_is_up = true` to check only that the server processes logins, without keeping a valid account. Access-Challenge is always down.
- Each emfor host must be a client of the RADIUS server, with the same secret. In FreeRADIUS, add it to `clients.conf`; a client can cover a whole network (`ipaddr = 10.0.0.0/24`).
- Every request carries Message-Authenticator first, as advised after BlastRADIUS (2024), so servers with `require_message_authenticator = yes` accept it. Servers that check it **drop a request made with the wrong secret without answering**: with FreeRADIUS a wrong secret shows as `timeout`, and the server log says `invalid Message-Authenticator! (Shared secret is incorrect.)`. Servers that do not check it answer, and the plugin reports `auth: response authenticator does not match (wrong secret?)`.
- Set `require_message_authenticator = true` only for servers that sign their responses with Message-Authenticator (FreeRADIUS 3.2.5 and later); FreeRADIUS 3.2.1, for example, does not, and every check would be down with `auth: response has no Message-Authenticator`.
- An `auth: ` reason for Access-Reject or Access-Challenge ends with the server's Reply-Message, when it sends one: `auth: Access-Reject: Unknown user`.
- The secret and the password never appear in check reasons, in emfor's log, or in the plugin's stderr. A `secret_file` or `password_file` that cannot be read during a check is `error`, and no request is sent.

### SIP

The `sip` plugin sends one SIP OPTIONS request (RFC 3261) and judges the status code of the final response; provisional responses (1xx) are skipped.

```toml
[plugins."sip"]
command = ["/usr/local/libexec/emfor-check", "sip"]

[services."sbc.svc.emfor"]
check = "sip"
backends = ["10.0.9.11:5060", "10.0.9.12:5060"]
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `transport` | string | `udp` | `udp` or `tcp` |
| `tls` | boolean | `false` | SIP over TLS (usually port 5061); needs `transport = "tcp"` |
| `tls_verify`, `server_name`, `ca_file` | | | As for the other plugins, see [TLS](#tls); need `tls = true` |
| `uri` | string | `sip:<backend IP>:<port>` (`sips:` with TLS) | Request-URI and To; a `sip:` or `sips:` URI of at most 256 bytes |
| `from` | string | `sip:emfor@emfor.invalid` | From |
| `expect_status` | array of strings | `["200-499"]` | Final status codes that count as up: codes from 200 to 699 or ranges `lo-hi` |

Any other key is an error. SCTP, WebSocket, REGISTER and authentication are not supported.

- **Why 4xx is up by default:** many proxies answer an OPTIONS addressed to themselves with 404, 405 or 483, or ask for authentication with 401 or 407, while working normally. A server that is overloaded or in maintenance says so with 503, and 5xx and 6xx are down. This is also what Kamailio's dispatcher and similar tools do.
- **To accept only 200**, set `expect_status = ["200"]`, and allow the emfor hosts on the server (an IP allow list) so that it answers 200 instead of 403 or 401.
- A final response outside `expect_status` is down with `status: <code> <reason phrase>`, for example `status: 503 Service Unavailable`.
- A response that is not SIP, lacks Via, Call-ID or CSeq, or is longer than 65536 bytes is down with `query: `. Over UDP an answer to another request (another Via branch, Call-ID or CSeq) is ignored; over TCP it is down with `query: response does not match the request`.
- The request has a Via with `rport`, so the server answers where the request came from even behind NAT.
- SIP servers often log every OPTIONS request; use a longer `interval`, or lower the log level for OPTIONS on the server.

## Reading results

| Reason starts with | Result | Meaning |
| --- | --- | --- |
| `connect: ` | down | Connection refused or dropped |
| `timeout` | down | No answer within the service `timeout` (for `radius`, possibly a wrong secret, see [RADIUS](#radius)) |
| `tls: ` | down | TLS handshake or certificate check failed, the port does not speak TLS, or the LDAP server refused StartTLS |
| `auth: ` | down | Login rejected (wrong password, unknown user or database, too many connections); for `ldap`, the resultCode and its name, see [LDAP](#ldap); for `radius`, a reject or a response not signed with the secret, see [RADIUS](#radius) |
| `query: ` | down | A query failed or answered something unexpected (for `dns`, a response that cannot be parsed or, over TCP, answers another query; for `radius`, a malformed response or an unexpected response code) |
| `role is ` | down | The backend has another role |
| `rcode: `, `answer: ` | down | `dns`: an unexpected response code, or no records of the asked type, see [DNS](#dns) |
| `status: ` | down | `sip`: a final status code outside `expect_status`, see [SIP](#sip) |
| `password_file: `, `secret_file: `, `ca_file: ` | error | The file could not be read during the check |

## What is tested

`tests/plugins/run.sh` checks all seven plugins against real servers in a Debian container: redis 7, MariaDB and PostgreSQL 15, each with a primary and a replica, read-only server or standby, and TLS; OpenLDAP 2.5 with LDAPS, StartTLS and a server without TLS; unbound 1.17 with a local zone, over UDP and TCP; FreeRADIUS 3.2.1 with Status-Server on the authentication and accounting ports and a test account; and Kamailio 5.6.3 over UDP, TCP and TLS. The `ldap` plugin is tested against OpenLDAP only; Active Directory and other servers are untested. The `dns` plugin is tested against unbound only, the `radius` plugin against FreeRADIUS only, and the `sip` plugin against Kamailio only. The `mysql` plugin is tested against MariaDB only; check it against your MySQL version before relying on it. Like the rest of emfor, the plugins have not been tested on FreeBSD yet.
