# network-checks Specification

## Purpose

Defines the network protocol plugins of `emfor-check` (DNS, and later RADIUS, SIP and Diameter) that check backends speaking protocols other than databases, often over UDP, as found in closed data centres such as telecom sites. It covers how they exchange datagrams, how their check options are validated, and how their outcomes map to `up`, `down` and `error`.

## Requirements

### Requirement: UDP request and response exchange

A network plugin that checks over UDP SHALL use a socket bound to the unspecified address of the backend's address family and connected to the backend address, so that only datagrams from the backend are received. It SHALL send the request once, then wait for a response that the plugin's protocol rules match to this request, discarding every other datagram. When no matching response has arrived by half of the request's `timeout_ms`, it SHALL send the identical request once more. It SHALL read at most 65535 bytes of each datagram. When no matching response has arrived within `timeout_ms`, the check SHALL be `down` with reason `timeout`. When the operating system reports that the backend port is unreachable (an ICMP port unreachable reported as a refused connection), the check SHALL be `down` with a reason starting `connect: `.

#### Scenario: Lost first datagram

- **WHEN** a check has `timeout_ms` 900 and the backend ignores the first request but answers the second
- **THEN** the plugin sends the second request about 450 ms after the first and the check uses the backend's answer

#### Scenario: Unrelated datagrams are ignored

- **WHEN** the backend first sends a datagram that does not match the request and then the matching response
- **THEN** the check result comes from the matching response

#### Scenario: Nothing listens on the UDP port

- **WHEN** the backend host answers the request with ICMP port unreachable
- **THEN** the check result is `down` with a reason starting `connect: `

#### Scenario: No answer

- **WHEN** a check has `timeout_ms` 900 and the backend never answers
- **THEN** the plugin sends exactly two requests and answers `down` with reason `timeout` within about 900 ms


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
### Requirement: Network check outcome classification

A network plugin SHALL answer `error` only for failures on the plugin's side: a file named in its `check_options` that cannot be read at check time, or a local resource limit (too many open files, out of memory, no socket of the address family available). Every failure caused by the backend or the network SHALL be `down`. A reason SHALL never contain a secret or password read from a file.

#### Scenario: Local socket failure

- **WHEN** the plugin cannot create a socket because the process has no free file descriptors
- **THEN** the check result is `error`, which does not count against the backend


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
### Requirement: DNS check options

The `dns` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `name` (required): a domain name of at most 253 characters without its trailing dot, made of labels of 1 to 63 characters of ASCII letters, digits, `-` and `_`, with an optional trailing dot; `.` alone is the root. Otherwise the error is `not a valid domain name`.
- `type`: one of `A`, `AAAA`, `CNAME`, `MX`, `NS`, `PTR`, `SOA`, `SRV`, `TXT`, compared without regard to case, default `A`. Otherwise the error is `must be one of A, AAAA, CNAME, MX, NS, PTR, SOA, SRV, TXT`.
- `transport`: `udp` or `tcp`, default `udp`. Otherwise the error is `must be udp or tcp`.
- `recursion_desired`: a boolean, default `true`.
- `expect_rcode`: a non-empty array of response code names, each one of `NOERROR`, `FORMERR`, `SERVFAIL`, `NXDOMAIN`, `NOTIMP`, `REFUSED`, compared without regard to case, default `["NOERROR"]`. An empty array or a value that is not an array is `must be a non-empty array`; an unknown name is `must be one of NOERROR, FORMERR, SERVFAIL, NXDOMAIN, NOTIMP, REFUSED`.
- `expect_answer`: a boolean, default `false`.

A missing `name` SHALL be `missing required key`; a non-string `name`, `type` or `transport` SHALL be `must be a string`; a non-boolean `recursion_desired` or `expect_answer` SHALL be `must be a boolean`.

#### Scenario: Minimal options

- **WHEN** a service with `check = "dns"` sets `check_options = { name = "ns-check.example.org" }`
- **THEN** the plugin reports no errors and checks with type `A` over UDP, recursion desired, expecting `NOERROR`

##### Example: DNS option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | `name`: missing required key |
| `{ name = "example.org." }` | none |
| `{ name = ".", type = "soa" }` | none |
| `{ name = "bad name.example" }` | `name`: not a valid domain name |
| `{ name = "example.org", type = "ANY" }` | `type`: must be one of A, AAAA, CNAME, MX, NS, PTR, SOA, SRV, TXT |
| `{ name = "example.org", transport = "tls" }` | `transport`: must be udp or tcp |
| `{ name = "example.org", expect_rcode = [] }` | `expect_rcode`: must be a non-empty array |
| `{ name = "example.org", expect_rcode = ["NOERROR", "BADVERS"] }` | `expect_rcode`: must be one of NOERROR, FORMERR, SERVFAIL, NXDOMAIN, NOTIMP, REFUSED |
| `{ name = "example.org", expect_answer = "yes" }` | `expect_answer`: must be a boolean |
| `{ name = "example.org", tls = true }` | `tls`: unknown key |


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
### Requirement: DNS check

The `dns` plugin SHALL send one DNS query per check to the backend: a random 16-bit ID, opcode QUERY, the RD flag set to `recursion_desired`, one question for `name`, `type` and class IN, and no additional records. With `transport = "udp"` it SHALL use the UDP request and response exchange, and a response SHALL match when its QR flag is set, its ID equals the query's, and it has exactly one question equal to the query's (the name compared without regard to ASCII case or a trailing dot). With `transport = "tcp"` it SHALL connect to the backend (a failure is `down` with a reason starting `connect: `), send the query with a 2-byte length prefix, read one length-prefixed response, and treat a response whose ID or question differs as `down` with reason `query: response does not match the query`.

It SHALL judge the matching response in this order:

1. A response that cannot be parsed SHALL be `down` with a reason starting `query: `.
2. A response code not in `expect_rcode` SHALL be `down` with reason `rcode: <name>`, using the standard mnemonic of the response code (`SERVFAIL`, `NXDOMAIN`, `REFUSED`, …), or its decimal value when it has none.
3. With `expect_answer = true`, an answer section without a record of `type` SHALL be `down` with reason `answer: no <TYPE> records`, followed by ` (truncated; set transport = "tcp")` when the response has the TC flag set.
4. Otherwise the check SHALL be `up`.

#### Scenario: Name resolves

- **WHEN** the backend answers an `A` query for `name` with `NOERROR` and one A record
- **THEN** the check result is `up`

#### Scenario: Server failure

- **WHEN** the backend answers with response code SERVFAIL and `expect_rcode` is the default
- **THEN** the check result is `down` with reason `rcode: SERVFAIL`

##### Example: DNS outcomes

| Options | Backend answer | Result |
| --- | --- | --- |
| defaults | NOERROR, one A record | `up` |
| defaults | NOERROR, no records | `up` |
| `expect_answer = true` | NOERROR, no records | `down`, `answer: no A records` |
| `expect_answer = true` | NOERROR, TC set, no records | `down`, `answer: no A records (truncated; set transport = "tcp")` |
| `expect_answer = true`, `type = "AAAA"` | NOERROR, only an A record | `down`, `answer: no AAAA records` |
| defaults | NXDOMAIN | `down`, `rcode: NXDOMAIN` |
| `expect_rcode = ["NOERROR", "NXDOMAIN"]` | NXDOMAIN | `up` |
| defaults | REFUSED | `down`, `rcode: REFUSED` |
| defaults | 12 bytes of garbage with the query's ID | `down`, reason starting `query: ` |

#### Scenario: Response for another question

- **WHEN** over UDP the backend first answers with the right ID but a different question, then with the matching response
- **THEN** the first datagram is ignored and the check result comes from the matching response

#### Scenario: DNS over TCP

- **WHEN** a check has `transport = "tcp"` and the backend answers the length-prefixed query with NOERROR
- **THEN** the check result is `up`

#### Scenario: TCP port closed

- **WHEN** a check has `transport = "tcp"` and nothing listens on the backend port
- **THEN** the check result is `down` with a reason starting `connect: `

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
### Requirement: RADIUS check options

The `radius` plugin SHALL accept these `check_options` keys and SHALL report every other key, including `secret` and `password`, as `unknown key`:

- `secret_file` (required): an absolute path of a readable file whose content, after removing one trailing `\n` or `\r\n`, is the non-empty shared secret.
- `request`: `status-server` or `access-request`, default `status-server`. Otherwise the error is `must be status-server or access-request`.
- `application`: `auth` or `acct`, default `auth`. Otherwise the error is `must be auth or acct`. `acct` together with `request = "access-request"` SHALL be an error on `application`: `requires request = "status-server"`.
- `user`: a string of 1 to 253 bytes, otherwise `must be 1 to 253 bytes`; `password_file`: an absolute path of a readable file, non-empty after removing one trailing `\n` or `\r\n`, holding at most 128 bytes (the longest password User-Password can carry), otherwise `password longer than 128 bytes`. With `request = "access-request"` both SHALL be required (`missing required key`); with `request = "status-server"` each of them SHALL be an error: `requires request = "access-request"`.
- `reject_is_up`: a boolean, default `false`; set with `request = "status-server"` it SHALL be an error: `requires request = "access-request"`.
- `nas_identifier`: a string of 1 to 253 bytes, default `emfor`. Otherwise the error is `must be 1 to 253 bytes`.
- `require_message_authenticator`: a boolean, default `false`.

Path errors SHALL use the messages `not an absolute path`, `file not found` and `file is empty`; type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Status-Server with a secret

- **WHEN** a service with `check = "radius"` sets `check_options = { secret_file = "/etc/emfor/radius.secret" }` and the file holds `testing123\n`
- **THEN** the plugin reports no errors and checks with Status-Server on the authentication port

##### Example: RADIUS option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | `secret_file`: missing required key |
| `{ secret_file = "radius.secret" }` | `secret_file`: not an absolute path |
| `{ secret_file = "/etc/emfor/empty" }` (file holds only `\n`) | `secret_file`: file is empty |
| `{ secret_file = "/etc/emfor/radius.secret", secret = "testing123" }` | `secret`: unknown key |
| `{ secret_file = "/etc/emfor/radius.secret", request = "access-request" }` | `user`: missing required key; `password_file`: missing required key |
| `{ secret_file = "/etc/emfor/radius.secret", user = "probe" }` | `user`: requires request = "access-request" |
| `{ secret_file = "/etc/emfor/radius.secret", request = "access-request", application = "acct", user = "probe", password_file = "/etc/emfor/probe.pass" }` | `application`: requires request = "status-server" |
| `{ secret_file = "/etc/emfor/radius.secret", reject_is_up = true }` | `reject_is_up`: requires request = "access-request" |
| `{ secret_file = "/etc/emfor/radius.secret", request = "coa" }` | `request`: must be status-server or access-request |
| `{ secret_file = "/etc/emfor/radius.secret", nas_identifier = "" }` | `nas_identifier`: must be 1 to 253 bytes |


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
### Requirement: RADIUS check

On every check the `radius` plugin SHALL read `secret_file`, and `password_file` with `request = "access-request"`; when a file cannot be read the result SHALL be `error` with a reason starting `secret_file: ` or `password_file: ` that names the I/O error kind. It SHALL send one request through the UDP request and response exchange: a random Identifier, a random 16-byte Request Authenticator, and the attributes Message-Authenticator first (HMAC-MD5 keyed with the shared secret over the packet with this attribute's value set to 16 zero bytes), then, with `request = "access-request"`, User-Name set to `user` and User-Password set to the password hidden as in RFC 2865 section 5.2, then NAS-Identifier set to `nas_identifier`. The Code SHALL be 12 (Status-Server) or 1 (Access-Request). A datagram of at least 20 bytes with the request's Identifier SHALL match.

The plugin SHALL judge the matching response in this order:

1. A Length field that differs from the datagram length, or an attribute whose length is below 2 or runs past the end, SHALL be `down` with reason `query: malformed response`.
2. A Response Authenticator other than MD5 of the Code, Identifier, Length, the Request Authenticator, the attributes and the shared secret SHALL be `down` with reason `auth: response authenticator does not match (wrong secret?)`.
3. A Message-Authenticator attribute whose value differs from HMAC-MD5 over the response with the Request Authenticator in place of the Response Authenticator and this attribute's value zeroed SHALL be `down` with reason `auth: Message-Authenticator does not match`. A response without Message-Authenticator SHALL be `down` with reason `auth: response has no Message-Authenticator` when `require_message_authenticator` is `true`.
4. With `application = "auth"`: Access-Accept SHALL be `up`; Access-Reject SHALL be `up` when `reject_is_up` is `true` and otherwise `down` with reason `auth: Access-Reject`; Access-Challenge SHALL be `down` with reason `auth: Access-Challenge`. With `application = "acct"`: Accounting-Response SHALL be `up`. Any other Code SHALL be `down` with reason `query: unexpected response code <N>`.

An `auth: ` reason from step 4 SHALL be followed by `: ` and the Reply-Message attributes of the response joined by single spaces when it has any, with bytes that are not valid UTF-8 replaced by U+FFFD. No reason and no stderr line SHALL contain the shared secret or the password.

#### Scenario: Server answers Status-Server

- **WHEN** a check uses the defaults and the backend answers with an Access-Accept signed with the same secret
- **THEN** the check result is `up`

#### Scenario: Wrong shared secret

- **WHEN** the backend signs its Access-Accept with a different secret than the one in `secret_file`
- **THEN** the check result is `down` with reason `auth: response authenticator does not match (wrong secret?)`

##### Example: RADIUS outcomes

| Options | Backend answer (correctly signed unless noted) | Result |
| --- | --- | --- |
| defaults | Access-Accept | `up` |
| `application = "acct"` | Accounting-Response | `up` |
| `application = "acct"` | Access-Accept | `down`, `query: unexpected response code 2` |
| `request = "access-request"` | Access-Accept | `up` |
| `request = "access-request"` | Access-Reject with Reply-Message `Unknown user` | `down`, `auth: Access-Reject: Unknown user` |
| `request = "access-request"`, `reject_is_up = true` | Access-Reject | `up` |
| `request = "access-request"` | Access-Challenge | `down`, `auth: Access-Challenge` |
| defaults | Access-Accept with a wrong Message-Authenticator | `down`, `auth: Message-Authenticator does not match` |
| `require_message_authenticator = true` | Access-Accept without Message-Authenticator | `down`, `auth: response has no Message-Authenticator` |
| defaults | Access-Accept without Message-Authenticator | `up` |
| defaults | Length field 40 in a 20-byte datagram | `down`, `query: malformed response` |

#### Scenario: Password hidden as in RFC 2865

- **WHEN** an Access-Request is built with the shared secret, Request Authenticator and password of the example packet in RFC 2865 section 7.1
- **THEN** its User-Password attribute value equals the one in that example

#### Scenario: Secret file removed

- **WHEN** `secret_file` was readable when the config was loaded and is deleted before a check
- **THEN** the check result is `error` with a reason starting `secret_file: `, and the backend is not sent a request

#### Scenario: Server without Status-Server support

- **WHEN** the backend silently drops Status-Server requests and the check has `timeout_ms` 900
- **THEN** the check result is `down` with reason `timeout`

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