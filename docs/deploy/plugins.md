# Checking redis, MySQL, PostgreSQL, LDAP, DNS, RADIUS, SIP, Diameter, SMTP, AMQP, MongoDB and gRPC with emfor-check

`emfor-check` is the plugin program that ships with emfor. One executable holds twelve plugins; its only argument picks which one runs:

| Argument | Checks | Up when |
| --- | --- | --- |
| `redis` | redis | `PING` answers `PONG` (after `AUTH` when a password is set) |
| `mysql` | MySQL, MariaDB | the login succeeds and `SELECT 1` returns 1 |
| `pgsql` | PostgreSQL | the login succeeds and `SELECT 1` returns 1 |
| `ldap` | OpenLDAP and other LDAPv3 servers | a simple bind succeeds (resultCode 0) |
| `dns` | DNS servers (unbound, BIND, PowerDNS, …) | the answer to one query has an expected response code, and records when asked for |
| `radius` | RADIUS servers (FreeRADIUS, …) | a Status-Server (or Access-Request) gets a correctly signed accept |
| `sip` | SIP proxies, registrars and SBCs (Kamailio, OpenSIPS, Asterisk, …) | an OPTIONS request gets a final response with an expected status code |
| `diameter` | Diameter nodes: HSS, PCRF, OCS, DRA (freeDiameter, …) | the capabilities exchange (CER / CEA) and a device watchdog (DWR / DWA) succeed |
| `smtp` | SMTP servers (Postfix, Exim, …) | the greeting is 220 and EHLO answers 250, over STARTTLS or TLS when asked for |
| `amqp` | AMQP 0-9-1 brokers (RabbitMQ, …) | the login succeeds and the vhost opens (Connection.Open-Ok) |
| `mongodb` | MongoDB 4.4 and later: replica set members, standalone servers, mongos | `hello` answers, the login succeeds when a user is set, and the role matches |
| `grpc` | gRPC services with the standard health service (`grpc.health.v1`) | `Health/Check` answers `SERVING` |

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

These are the options of the database plugins `redis`, `mysql`, `pgsql` and `ldap`. The `dns`, `radius`, `sip`, `diameter`, `smtp`, `amqp`, `mongodb` and `grpc` plugins have their own, see [DNS](#dns), [RADIUS](#radius), [SIP](#sip), [Diameter](#diameter), [SMTP](#smtp), [AMQP](#amqp), [MongoDB](#mongodb) and [gRPC](#grpc).

| Key | Plugins | Type | Default | Meaning |
| --- | --- | --- | --- | --- |
| `user` | all; required for `mysql`, `pgsql` | string | none | Account to log in with; for `ldap` a DN, and without it the bind is anonymous |
| `password_file` | all | absolute path | none (no password) | File holding the password; see below |
| `database` | `pgsql` only, required | string | none | Database to connect to |
| `role` | all but `ldap` | string | none (any role) | `redis`: `master` or `replica`; `mysql`, `pgsql`: `primary` or `replica` |
| `max_lag` | all but `ldap`, needs `role = "replica"` | integer, 1 to 86400 | none (any lag) | Seconds a replica may be behind; see [Replicas](#replicas) |
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
| `redis`, `mysql`, `pgsql` | `replica` | see [Replicas](#replicas) |
| `mongodb` | `primary`, `secondary` | the `hello` reply, see [MongoDB](#mongodb) |

For `mysql`, an answer to `SELECT 1` or `@@global.read_only` that is NULL or not an integer is down, for example `query: @@global.read_only answered NULL, want an integer`. A redis reply longer than 2 MiB is down with `query: invalid reply: reply too long`.

### Replicas

`role = "replica"` accepts only a replica that is replicating, so that applications reading from replicas are not sent to one that has stopped receiving changes:

| Plugin | Up when | Down reasons |
| --- | --- | --- |
| `redis` | `ROLE` says `slave` and its link state is `connected` | `role is master, want replica`, `replication: link to master is connect` (or `connecting`, `sync`, …) |
| `mysql` | `SHOW REPLICA STATUS` (`SHOW SLAVE STATUS` on servers that do not know it) has a row, and both the IO and the SQL thread say `Yes` | `role is not a replica, want replica`, `replication: IO thread is Connecting: <Last_IO_Error>`, `replication: SQL thread is No: <Last_SQL_Error>` |
| `pgsql` | `pg_is_in_recovery()` is true and `pg_stat_wal_receiver` has a row | `role is primary, want replica`, `replication: no WAL receiver` |

With `max_lag` the replica is also down, with `replication: <N>s behind, max <M>s`, when it is more than `max_lag` whole seconds behind, and with `replication: lag unknown` when the server cannot say:

| Plugin | Lag is |
| --- | --- |
| `redis` | `master_last_io_seconds_ago` from `INFO replication`: seconds since the replica last heard from its master. The master pings its replicas every 10 seconds by default (`repl-ping-replica-period`), so use a `max_lag` of 15 or more |
| `mysql` | `Seconds_Behind_Source` (`Seconds_Behind_Master`); NULL is "lag unknown" |
| `pgsql` | 0 when all received WAL has been replayed, otherwise the time since the last replayed transaction (`pg_last_xact_replay_timestamp()`); an idle primary therefore does not make the lag grow |

The checks need these rights besides logging in:

- `redis`: the `ROLE` command, and `INFO` with `max_lag`;
- `mysql`: `REPLICATION CLIENT` (MySQL), or `REPLICA MONITOR` (MariaDB 10.5.9 and later), for the replica status; without it the reason is the server's `query: ` error;
- `pgsql`: none.

```toml
[services."reports-db.svc.emfor"]
check = "pgsql"
backends = ["10.0.3.2:5432", "10.0.3.3:5432"]
check_options = { user = "emfor", database = "postgres", role = "replica", max_lag = 30, password_file = "/etc/emfor/pg.pass" }
```

MariaDB Galera, multi-source replication (only the default connection is read), and redis Sentinel and Cluster are not covered.

### Passwords

`password_file` names a file whose content is the password; one trailing newline (`\n` or `\r\n`) is removed. The plugin reads the file on every check, so you can change the password on the server and in the file without reloading emfor.

- Make the file readable only by the account emfor runs as: `install -m 0600 -o emfor /dev/stdin /etc/emfor/pg.pass`.
- Give emfor its own account with the least rights the check needs: login, `SELECT 1`, and for `role` the role query.
- The password never appears in check reasons, in emfor's log, or in the plugin's stderr.

If the file cannot be read during a check, the check is `error`, which does not count against the backend.

A `user` (and for `pgsql` a `database`) with a NUL character is refused by `emfor check-config`: the protocols end those strings at NUL. For `amqp` the password must not contain NUL either, since the PLAIN login separates the fields with it.

### TLS

With `tls = true` (or `starttls = true` for `ldap`), the certificate is checked against `server_name`, or against the backend IP when `server_name` is not set. Backends are listed by IP, and database certificates usually name a host, so set `server_name` unless the certificate has the IP in its subject alternative names. Use `ca_file` for an internal CA; certificates in it that cannot serve as a CA are skipped. `emfor check-config` refuses a file with none left (`no usable certificate`), and one that changes into such a file later makes the check `error` with `ca_file: no usable certificate`. `tls_verify = false` keeps the connection encrypted but accepts any certificate. A server that closes the connection during the handshake is down with `tls: connection closed during the TLS handshake (does the server have a certificate?)`; Active Directory does this on 636, and refuses StartTLS with `52 unavailable`, until a certificate for LDAPS is installed on the domain controller.

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
- A rejected bind is down with `auth: <code> <name>`, followed by the server's diagnostic message when it sends one, for example `auth: 49 invalidCredentials`, `auth: 53 unwillingToPerform: …` or `auth: 8 strongerAuthRequired` (the server wants TLS). Active Directory adds its own code to the message, for example `auth: 49 invalidCredentials: 80090308: LdapErr: DSID-0C090434, comment: AcceptSecurityContext error, data 52e, v4f7c` (`data 52e` is a wrong user name or password). The names are those of RFC 4511; an unknown code shows only the number.
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
| `uri` | string | `sip:<backend IP>:<port>` (`sips:` with TLS) | Request-URI and To; a `sip:` or `sips:` URI of at most 256 bytes, without `<` or `>` |
| `from` | string | `sip:emfor@emfor.invalid` | From; without `<` or `>` |
| `expect_status` | array of strings | `["200-499"]` | Final status codes that count as up: codes from 200 to 699 or ranges `lo-hi` |

Any other key is an error. SCTP, WebSocket, REGISTER and authentication are not supported.

- **Why 4xx is up by default:** many proxies answer an OPTIONS addressed to themselves with 404, 405 or 483, or ask for authentication with 401 or 407, while working normally. A server that is overloaded or in maintenance says so with 503, and 5xx and 6xx are down. This is also what Kamailio's dispatcher and similar tools do.
- **To accept only 200**, set `expect_status = ["200"]`, and allow the emfor hosts on the server (an IP allow list) so that it answers 200 instead of 403 or 401.
- A final response outside `expect_status` is down with `status: <code> <reason phrase>`, for example `status: 503 Service Unavailable`.
- A response that is not SIP, lacks Via, Call-ID or CSeq, or is longer than 65536 bytes is down with `query: `. Over UDP an answer to another request (another Via branch, Call-ID or CSeq) is ignored; over TCP it is down with `query: response does not match the request`.
- The request has a Via with `rport`, so the server answers where the request came from even behind NAT.
- SIP servers often log every OPTIONS request; use a longer `interval`, or lower the log level for OPTIONS on the server.

### Diameter

The `diameter` plugin opens a new TCP connection (or TLS) to the node on every check, exchanges capabilities (CER / CEA, RFC 6733) and sends a Device-Watchdog-Request (DWR / DWA). Both answers must carry a Result-Code from 2000 to 2999. After a successful capabilities exchange, whatever the result, it sends a Disconnect-Peer-Request with Disconnect-Cause `DO_NOT_WANT_TO_TALK_TO_YOU`, waits at most 200 ms for the answer, and closes the connection; this happens after the result is reported, so a node that does not answer it does not slow the check down.

```toml
[plugins."diameter"]
command = ["/usr/local/libexec/emfor-check", "diameter"]

[services."pcrf.svc.emfor"]
check = "diameter"
backends = ["10.0.11.1:3868", "10.0.11.2:3868"]
interval = "10s"

[services."pcrf.svc.emfor".check_options]
origin_host = "emfor-app01.example.org"
origin_realm = "example.org"
# Gx (3GPP TS 29.212): application 16777238 of vendor 10415 (3GPP)
vendor_specific_application_ids = [{ vendor_id = 10415, auth_application_id = 16777238 }]
supported_vendor_ids = [10415]
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `origin_host` | string | required | Origin-Host: a DiameterIdentity of 1 to 255 ASCII letters, digits, `-` and `.` |
| `origin_realm` | string | required | Origin-Realm, the same rules |
| `auth_application_ids` | array of integers | `[]` | Auth-Application-Id values, 0 to 4294967295 |
| `acct_application_ids` | array of integers | `[]` | Acct-Application-Id values |
| `vendor_specific_application_ids` | array of tables | `[]` | Each `{ vendor_id, auth_application_id }` or `{ vendor_id, acct_application_id }`, for 3GPP and other vendor applications |
| `supported_vendor_ids` | array of integers | `[]` | Supported-Vendor-Id values |
| `host_ip_address` | string | the local address of the connection | Host-IP-Address, IPv4 or IPv6 |
| `tls` | boolean | `false` | TLS from the start of the connection (usually port 5658) |
| `tls_verify`, `server_name`, `ca_file` | | | As for the other plugins, see [TLS](#tls); need `tls = true` |
| `tls_cert_file`, `tls_key_file` | absolute paths | | A PEM client certificate chain and its PEM private key, presented when the node asks for one; set both, with `tls = true` |

At least one of `auth_application_ids`, `acct_application_ids` and `vendor_specific_application_ids` is required: the node accepts a peer only when they share an application, and emfor does not guess which one yours has. Any other key is an error. SCTP, DTLS, application requests (S6a AIR, Gx CCR, …) and a long-lived peer connection are not supported.

- **Allow emfor as a peer.** Diameter nodes accept only the peers they know: add every emfor host's `origin_host` to the node's peer list or ACL. Many implementations allow a wildcard or a whole realm. Give each emfor host its own `origin_host` so the node's logs tell them apart.
- **Client certificates.** RFC 6733 TLS authenticates both sides, and nodes such as freeDiameter require a client certificate whose name matches the peer's Origin-Host. Without one the TLS handshake completes on emfor's side and the node then closes the connection, so the check is down with `cea: connection closed by peer` (or `tls: ` when the node sends a TLS alert first). `tls_cert_file` and `tls_key_file` are checked for changes on every check (one `stat` each) and read again when they changed, so a renewed certificate takes effect at the next check without a reload; a file that cannot be read, or holds no certificate or no private key, is `error` with `tls_cert_file: ` or `tls_key_file: `, and nothing is sent. A key that does not belong to the certificate is reported as `tls_key_file: ` too, and `emfor check-config` already reports it on `tls_key_file`.
- **Interval.** Every check is a peer connection, a capabilities exchange and a disconnect, which the node usually logs. Use an `interval` of 10 seconds or more.
- A failed capabilities exchange or watchdog is down with `cea: <code> <name>` or `dwa: <code> <name>`; codes RFC 6733 does not name are shown as the number alone. Common ones:

| Reason | Meaning |
| --- | --- |
| `cea: 3010 DIAMETER_UNKNOWN_PEER` | The node does not know this `origin_host` (or realm): add it to the node's peer list |
| `cea: 5010 DIAMETER_NO_COMMON_APPLICATION` | The node supports none of the applications in `check_options` |
| `cea: 3004 DIAMETER_TOO_BUSY`, `dwa: 3004 DIAMETER_TOO_BUSY` | The node is overloaded |
| `cea: connection closed by peer` | The node closed the connection before answering: an unknown peer on some implementations, or a TLS node that wants a client certificate |
| `dwa: connection closed by peer` | The node accepted the peer but closed the connection before answering the watchdog |
| `query: ` | A message that is not Diameter version 1, has inconsistent lengths, is longer than 65536 bytes, or an answer without Result-Code |

Requests the node sends on its own (for example its own DWR) are ignored and not answered.

### SMTP

The `smtp` plugin opens a new TCP connection (or TLS) on every check, waits for the server's greeting and sends `EHLO`. It is up when the greeting is `220` and `EHLO` answers `250`; it then sends `QUIT` and closes the connection without waiting for the answer. It does not log in or send mail (no `MAIL FROM`, no `AUTH`).

```toml
[plugins."smtp"]
command = ["/usr/local/libexec/emfor-check", "smtp"]

[services."mx.svc.emfor"]
check = "smtp"
backends = ["10.0.12.1:25", "10.0.12.2:25"]
check_options = { starttls = true, server_name = "mx.example.org" }
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `helo` | string | the local address of the connection, as `[192.0.2.5]` or `[IPv6:2001:db8::5]` | The `EHLO` argument: a host name of 1 to 255 ASCII letters, digits, `-` and `.` |
| `starttls` | boolean | `false` | Upgrade with `STARTTLS` (ports 25 and 587), then send `EHLO` again over TLS |
| `tls` | boolean | `false` | TLS from the start of the connection (port 465); conflicts with `starttls` |
| `tls_verify`, `server_name`, `ca_file` | | | As for the other plugins, see [TLS](#tls); need `tls = true` or `starttls = true` |

Any other key is an error.

- With `starttls`, a server whose `EHLO` answer does not list `STARTTLS` is down with `tls: STARTTLS not offered`, and one that answers `STARTTLS` with anything but `220` is down with `tls: STARTTLS refused: <code> <text>`. Nothing is sent after either. The server's second `EHLO` answer, over TLS, must be `250` too.
- A greeting other than `220` (for example `421` or `554` from an overloaded or blocking server) is down with `greeting: <code> <text>`; an `EHLO` answer other than `250` is down with `ehlo: <code> <text>`. The text is the first line of the reply.
- A reply that is not SMTP, has a line longer than 2048 bytes, or is longer than 65536 bytes in all is down with `query: `; so is a server that closes the connection before answering.
- Mail servers log every connection, and some count one without mail against the client (Postfix's `anvil`, `postscreen`, fail2ban rules). Allow the emfor hosts there, and use an `interval` of 10 seconds or more.

### AMQP

The `amqp` plugin opens a new TCP connection (or TLS) on every check and goes through the AMQP 0-9-1 connection handshake: protocol header, Connection.Start, a `PLAIN` login in Start-Ok, Tune, and Open for `vhost`. It is up when the broker answers Connection.Open-Ok. It then sends Connection.Close (reply code 200), waits at most 200 ms for Close-Ok and closes the connection; this happens after the result is reported. It opens no channel and declares, reads or publishes nothing.

```toml
[plugins."amqp"]
command = ["/usr/local/libexec/emfor-check", "amqp"]

[services."mq.svc.emfor"]
check = "amqp"
backends = ["10.0.13.1:5672", "10.0.13.2:5672"]
check_options = { user = "emfor", password_file = "/etc/emfor/amqp.pass", vhost = "orders" }
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `user` | string | required | The user, 1 to 255 bytes |
| `password_file` | absolute path | required | As for the database plugins, see [Passwords](#passwords) |
| `vhost` | string | `/` | The virtual host to open, 1 to 255 bytes |
| `tls` | boolean | `false` | TLS from the start of the connection (usually port 5671) |
| `tls_verify`, `server_name`, `ca_file` | | | As for the other plugins, see [TLS](#tls); need `tls = true` |

Any other key is an error. Only the `PLAIN` mechanism is supported (not `EXTERNAL` with a client certificate, and not `AMQPLAIN`); AMQP 1.0 brokers (ActiveMQ Artemis, Azure Service Bus, …) are not.

- Give the check its own user with no rights except on the vhost it opens: with RabbitMQ, `rabbitmqctl add_user emfor <password>` and `rabbitmqctl set_permissions -p <vhost> emfor '^$' '^$' '^$'`. Opening a vhost needs permissions on it, even empty ones.
- The client properties announce `authentication_failure_close`, so a broker that rejects the login says why: `auth: 403 ACCESS_REFUSED - Login was refused using authentication mechanism PLAIN …`. A broker that closes the connection instead is down with `auth: connection closed after login`. A broker that does not offer `PLAIN` is down with `auth: server does not offer PLAIN`.
- A vhost that does not exist, or that the user has no permissions on, is down with `vhost: 530 NOT_ALLOWED - …`.
- Any other Connection.Close from the broker is down with `query: <code> <text>`. A broker that answers with another protocol version is down with `query: server speaks AMQP <a>-<b>-<c>`; a frame larger than 131072 bytes or not ending in `0xCE` is down with `query: frame too large` or `query: malformed frame`, and a method out of order with `query: unexpected method <class>.<method>`.
- A RabbitMQ memory or disk alarm blocks publishers, but the broker still accepts connections, so the check stays up during an alarm.
- RabbitMQ logs every connection and its close. Use a longer `interval` if that is too much.

### MongoDB

The `mongodb` plugin opens a new TCP connection (or TLS) on every check and sends `hello` (an OP_MSG command, which needs no login). With `user` set it then logs in with SCRAM-SHA-256 against `auth_source` and checks the server's signature. With `role` set it compares the role in the `hello` reply. It is up when every step succeeds. It reads no data and runs no other command.

```toml
[plugins."mongodb"]
command = ["/usr/local/libexec/emfor-check", "mongodb"]

[services."orders-db.svc.emfor"]
check = "mongodb"
balance = "failover"
backends = ["10.0.14.1:27017", "10.0.14.2:27017", "10.0.14.3:27017"]
check_options = { role = "primary", user = "emfor", password_file = "/etc/emfor/mongo.pass" }
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `user` | string | none: no login | The user, 1 to 255 bytes; needs `password_file` |
| `password_file` | absolute path | | As for the database plugins, see [Passwords](#passwords); needs `user`. Only printable ASCII passwords are supported |
| `auth_source` | string | `admin` | The database the user is defined in: 1 to 64 ASCII letters, digits, `_` and `-`; needs `user` |
| `role` | `primary` or `secondary` | any | The role the backend must have |
| `tls` | boolean | `false` | TLS from the start of the connection |
| `tls_verify`, `server_name`, `ca_file` | | | As for the other plugins, see [TLS](#tls); need `tls = true` |

Any other key is an error. The role comes from the `hello` reply, in this order:

| `hello` says | Role | Accepted by |
| --- | --- | --- |
| `arbiterOnly: true` | `arbiter` | none |
| `msg: "isdbgrid"` | `mongos` | `role = "primary"` |
| `isWritablePrimary: true` with `setName` | `primary` | `role = "primary"` |
| `isWritablePrimary: true` without `setName` | `standalone` | `role = "primary"` |
| `secondary: true` | `secondary` | `role = "secondary"` |
| anything else (`RECOVERING`, `STARTUP2`, …) | `unknown` | none |

Another role is down with `role is <role>, want <role>`, for example `role is secondary, want primary`. `role = "primary"` with `balance = "failover"` follows the replica set's primary.

- Without `user`, the check needs no account: `hello` works before a login, so it suits servers without access control and checks that only want the role. A login says more, because it needs the server to read its user data. Give the check a user with no roles: `db.getSiblingDB("admin").createUser({user: "emfor", pwd: "…", roles: []})`.
- A rejected login is down with `auth: <codeName>: <errmsg>`, for example `auth: AuthenticationFailed: Authentication failed.`; any other failed command is down with `query: <codeName>: <errmsg>`. A server whose signature does not match the password is down with `auth: server signature does not match`; a SCRAM message with fewer than 4096 iterations, a foreign nonce or a missing field is down with `auth: malformed SCRAM message`, and one asking for more than 1000000 iterations (MongoDB's default is 15000) is down with `auth: server asks for <n> SCRAM iterations, more than 1000000`. The key derivation runs on a separate thread, so the check's timeout still ends a slow login.
- A reply longer than 65536 bytes is down with `query: reply too large`; one that is not an OP_MSG reply to the command, whose BSON cannot be read, or that nests documents more than 100 levels deep, with `query: malformed reply`.
- Only printable ASCII passwords are supported: other passwords would need SASLprep. Validation (`emfor check-config`, a reload) refuses a `password_file` holding another password with `only printable ASCII passwords are supported`; if the file changes to one later, the check is `error` with `password_file: only printable ASCII passwords are supported`, and nothing is sent.
- Only SCRAM-SHA-256 is supported: not SCRAM-SHA-1, x.509, LDAP or Kerberos. MongoDB before 4.4 (4.2.10) has no `hello` and is down with `query: CommandNotFound: …`. Replication lag (`max_lag`), connection strings (`mongodb://`) and SRV records are not supported.
- MongoDB logs every connection and authentication. Use a longer `interval` if that is too much.

### gRPC

The `grpc` plugin opens a new HTTP/2 connection on every check and calls the standard health service: `POST /grpc.health.v1.Health/Check` with `HealthCheckRequest { service }`. It is up when the answer is `SERVING`. Without TLS it speaks HTTP/2 directly (prior knowledge, as gRPC servers expect); with `tls = true` it offers ALPN `h2`. `:authority` is `server_name` (an IPv6 address in brackets), or the backend's `ip:port`. The request carries `content-type: application/grpc`, `te: trailers` and `user-agent: emfor-check/<version>`, and nothing else: no token or other metadata.

```toml
[plugins."grpc"]
command = ["/usr/local/libexec/emfor-check", "grpc"]

[services."orders-api.svc.emfor"]
check = "grpc"
backends = ["10.0.15.1:50051", "10.0.15.2:50051"]
check_options = { service = "orders.v1.Orders" }
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `service` | string | `""`: the whole server | The service name to ask about, at most 256 bytes |
| `tls` | boolean | `false` | TLS with ALPN `h2` |
| `tls_verify`, `server_name`, `ca_file` | | | As for the other plugins, see [TLS](#tls); need `tls = true` |

Any other key is an error. The server must register the health service: in Go `google.golang.org/grpc/health`, in Java `grpc-services`' `HealthStatusManager`, in Python `grpcio-health-checking`. The answer is judged in this order; the first match is the reason:

| Answer | Result |
| --- | --- |
| An HTTP/2 error, or a server that does not speak HTTP/2 (an HTTP/1.1 server, plaintext to a TLS port) | down: `query: HTTP/2: <error>` |
| A status other than 200 | down: `query: HTTP status <n>` |
| A `content-type` that does not start with `application/grpc` | down: `query: not a gRPC response (content-type <value>)` |
| A `grpc-status` other than 0 | down: `grpc: <code> <NAME>`, with `: <grpc-message>` when the server sends one, for example `grpc: 12 UNIMPLEMENTED` (no health service) or `grpc: 5 NOT_FOUND: unknown service` (`service` is not registered) |
| No `grpc-status` | down: `query: no grpc-status` |
| A compressed message, a message longer than 65536 bytes, none, or one that cannot be decoded | down: `query: compressed message`, `query: reply too large`, `query: no message`, `query: malformed message` |
| `SERVING` | up |
| `NOT_SERVING`, `UNKNOWN`, `SERVICE_UNKNOWN`, or another value | down: `status: NOT_SERVING`, `status: UNKNOWN`, `status: SERVICE_UNKNOWN`, `status: <n>` |

- With `tls = true`, a server that completes the handshake without selecting `h2` is down with `tls: server did not select h2 (ALPN)`; one that refuses `h2` with a TLS alert is down with `tls: ` and the alert.
- Client certificates (mTLS), tokens and other metadata, compressed messages, `Watch` and HTTP/1.1 upgrade (`h2c` with `Upgrade:`) are not supported. A service that needs a token for the health service cannot be checked.

### QUIC

The `quic` plugin opens a new QUIC version 1 connection over UDP on every check, without 0-RTT, offering the ALPN values of `alpn` and verifying the certificate with TLS 1.3. With `http3 = false` (the default) it is up when the handshake completes, and it then closes the connection with application error code 0. With `http3 = true` it also sends one HTTP/3 `GET` for `path`, judged like the built-in [`http` check](../manual/04-health-checks.md#http): `:authority` is `host_header`, else `server_name` (an IPv6 address in brackets), else the backend's `ip:port`; redirects are not followed; `user-agent` is `emfor-check/<version>`. The connection then ends with `H3_NO_ERROR`. All checks of one plugin process share one local UDP socket per address family.

```toml
[plugins."quic"]
command = ["/usr/local/libexec/emfor-check", "quic"]

[services."web.svc.emfor"]
check = "quic"
backends = ["10.0.9.11:443", "10.0.9.12:443"]
check_options = { http3 = true, path = "/healthz", server_name = "web.internal" }
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `alpn` | array of strings | `["h3"]` | ALPN values offered, in order, each 1 to 255 bytes |
| `tls_verify`, `server_name`, `ca_file` | | | As for the other plugins, see [TLS](#tls); QUIC is always encrypted, so there is no `tls` key and these never need one |
| `http3` | boolean | `false` | Send an HTTP/3 `GET` after the handshake; needs `"h3"` in `alpn` |
| `path` | string | `/` | The request path and optional query, starting with `/`, without spaces, control characters or `#`; needs `http3 = true` |
| `host_header` | string | | `:authority`: a host with an optional port such as `api.internal:8443`, 1 to 256 bytes, no path or `user@`; needs `http3 = true` |
| `expect_status` | array | `["200-399"]` | Status codes (integers or strings) and `"lo-hi"` ranges from 100 to 599; needs `http3 = true` |
| `expect_body` | string | | 1 to 16384 bytes the body must contain (the first 65536 bytes are read, case-sensitive); needs `http3 = true` |

Any other key is an error, `tls` included. The result is decided in this order; the first match is the reason:

| Answer | Result |
| --- | --- |
| No answer within the service `timeout` | down: `timeout` |
| The local UDP socket cannot be opened, `ca_file` cannot be read, or the host has no CA certificates | error |
| The server supports no QUIC version 1 | down: `quic: version negotiation failed` |
| The TLS handshake fails (certificate, server name, no shared ALPN) | down: `tls: ` and the cause, for example `tls: no application protocol (ALPN)` |
| The server closes the connection, also during the HTTP/3 request or the body | down: `quic: closed by peer: <code>`, with `: <reason>` when it sends one, for example `quic: closed by peer: 2` for a refused connection; `quic: stateless reset`; another connection error is `quic: ` and the error |
| The server selected an ALPN that is not in `alpn` (a server that does not follow RFC 9001) | down: `tls: server selected ALPN <value> not in alpn` |
| `http3 = false` | up |
| Another HTTP/3 error, such as a reset request stream or response headers over 65536 bytes | down: `query: HTTP/3: <error>` |
| A status outside `expect_status` | down: `unexpected status <n>` |
| A failure reading the body, or `expect_body` not found | down: `body: …`, or `expect_body: got "<first 64 bytes>"` |
| Otherwise | up |

- UDP has no "connection refused": a closed port, a firewall that drops UDP and a server that is down all end as `timeout` after the full service `timeout`. The daemon host must be able to send UDP to the backend's port and receive the answers.
- The service `timeout` covers the whole check. A handshake takes at least one round trip, plus one more for `http3 = true`.
- 0-RTT, QUIC version 2, client certificates, methods other than `GET`, request bodies and headers other than `:authority` and `user-agent` are not supported. Each check is a new connection; none is kept between checks.

## Reading results

| Reason starts with | Result | Meaning |
| --- | --- | --- |
| `connect: ` | down | Connection refused or dropped |
| `timeout` | down | No answer within the service `timeout` (for `radius`, possibly a wrong secret, see [RADIUS](#radius)) |
| `tls: ` | down | TLS handshake or certificate check failed, the port does not speak TLS, or the LDAP or SMTP server refused or did not offer StartTLS |
| `auth: ` | down | Login rejected (wrong password, unknown user or database, too many connections); for `ldap`, the resultCode and its name, see [LDAP](#ldap); for `radius`, a reject or a response not signed with the secret, see [RADIUS](#radius) |
| `query: ` | down | A query failed or answered something unexpected (for `dns`, a response that cannot be parsed or, over TCP, answers another query; for `radius`, a malformed response or an unexpected response code) |
| `role is ` | down | The backend has another role |
| `replication: ` | down | A replica that is not replicating or is more than `max_lag` behind, see [Replicas](#replicas) |
| `rcode: `, `answer: ` | down | `dns`: an unexpected response code, or no records of the asked type, see [DNS](#dns) |
| `status: ` | down | `sip`: a final status code outside `expect_status`, see [SIP](#sip); `grpc`: a health status other than `SERVING`, see [gRPC](#grpc) |
| `cea: `, `dwa: ` | down | `diameter`: a capabilities exchange or watchdog answer with a failed Result-Code, or the node closed the connection, see [Diameter](#diameter) |
| `greeting: `, `ehlo: ` | down | `smtp`: a greeting other than 220 or an `EHLO` answer other than 250, see [SMTP](#smtp) |
| `vhost: ` | down | `amqp`: the broker refused to open the vhost, see [AMQP](#amqp) |
| `grpc: ` | down | `grpc`: a `grpc-status` other than 0, for example no health service (`12 UNIMPLEMENTED`) or an unknown `service` (`5 NOT_FOUND`), see [gRPC](#grpc) |
| `quic: ` | down | `quic`: the server closed the connection, sent a stateless reset, or another QUIC transport error, see [QUIC](#quic) |
| `unexpected status `, `expect_body: `, `body: ` | down | `quic` with `http3 = true`: a status outside `expect_status`, or a body without `expect_body`, see [QUIC](#quic) |
| `password_file: `, `secret_file: `, `ca_file: `, `tls_cert_file: `, `tls_key_file: ` | error | The file could not be read during the check |

## What is tested

`tests/plugins/run.sh` checks all thirteen plugins against real servers in a Debian container: redis 7, MariaDB and PostgreSQL 15, each with a primary, a replica that replicates from it, a replica whose primary is unreachable, and TLS; OpenLDAP 2.5 with LDAPS, StartTLS and a server without TLS; unbound 1.17 with a local zone, over UDP and TCP; FreeRADIUS 3.2.1 with Status-Server on the authentication and accounting ports and a test account; Kamailio 5.6.3 over UDP, TCP and TLS; freeDiameter 1.2.1 over TCP and over TLS with a client certificate, with an ACL that knows one peer; Postfix 3.7 with STARTTLS, with TLS on port 465 and without TLS; RabbitMQ 3.10 over plaintext and TLS, with a vhost the user may open and one it may not; MongoDB 8.0 with a two-member replica set and a server with access control over plaintext and TLS; a gRPC server written with Python grpcio 1.51 over plaintext and TLS, with and without a health service; and Caddy 2.6 with HTTP/3. The `ldap` plugin is tested against OpenLDAP. Against Active Directory (a Windows Server domain controller without an LDAPS certificate) it was checked by hand only: an anonymous bind is up, a wrong password is `auth: 49 invalidCredentials: …`, StartTLS is refused and LDAPS on 636 is `tls: connection closed during the TLS handshake …`; a successful bind with an account, StartTLS and LDAPS with a certificate are untested there, as are other servers. The `dns` plugin is tested against unbound only, the `radius` plugin against FreeRADIUS only, the `sip` plugin against Kamailio only, the `diameter` plugin against freeDiameter only, the `smtp` plugin against Postfix only, the `amqp` plugin against RabbitMQ only, the `mongodb` plugin against MongoDB 8.0 only, the `grpc` plugin against grpcio (Python) only, and the `quic` plugin against Caddy 2.6 only. The `mysql` plugin is tested against MariaDB and, with `tests/plugins/mysql.sh`, against MySQL 8.0 and 8.4 (the `mysql` images from Docker Hub) with the same cases: a primary with TLS, a replica that replicates from it and a replica whose primary is unreachable, users with MySQL's default `caching_sha2_password`, and the replica status read with MySQL's `Replica_*` and `Seconds_Behind_Source` column names. Other MySQL versions are untested. On FreeBSD (15.1-RELEASE, arm64) only the unit tests of `emfor-check` have run; the plugins have not been checked against real servers there.
