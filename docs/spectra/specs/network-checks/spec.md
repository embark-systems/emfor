# network-checks Specification

## Purpose

Defines the network protocol plugins of `emfor-check` (DNS, RADIUS, SIP and Diameter) that check backends speaking protocols other than databases, often over UDP, as found in closed data centres such as telecom sites. It covers how they exchange datagrams, how their check options are validated, and how their outcomes map to `up`, `down` and `error`.

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

---
### Requirement: SIP check options

The `sip` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `transport`: `udp` or `tcp`, default `udp`. Otherwise the error is `must be udp or tcp`. With `tls = true` a `transport` other than `tcp` SHALL be an error on `transport`: `must be tcp when tls = true`.
- `tls`: a boolean, default `false`. `tls_verify`, `server_name` and `ca_file` SHALL follow the rules they have for the other `emfor-check` plugins, including the error `requires tls = true` when `tls` is not `true`.
- `uri` and `from`: strings of at most 256 bytes that start with `sip:` or `sips:` and contain no whitespace or control characters. Otherwise the error is `must be a sip: or sips: URI`. `uri` defaults to `sip:<backend>` (`sips:<backend>` with `tls = true`), where `<backend>` is the backend IP, in brackets for IPv6, followed by `:` and the port; `from` defaults to `sip:emfor@emfor.invalid`.
- `expect_status`: a non-empty array of strings, each a status code from 200 to 699 or an inclusive range `lo-hi` of such codes with `lo` not greater than `hi`, default `["200-499"]`. An empty array or a value that is not an array is `must be a non-empty array`; an invalid entry is `must be a status code from 200 to 699 or a range lo-hi`.

Type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Defaults

- **WHEN** a service with `check = "sip"` and backend `10.0.9.11:5060` sets no `check_options`
- **THEN** the plugin reports no errors and sends OPTIONS for `sip:10.0.9.11:5060` over UDP, accepting final responses 200 to 499

##### Example: SIP option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | none |
| `{ transport = "sctp" }` | `transport`: must be udp or tcp |
| `{ tls = true }` | `transport`: must be tcp when tls = true |
| `{ transport = "tcp", tls = true, server_name = "sbc.example.org" }` | none |
| `{ transport = "tcp", server_name = "sbc.example.org" }` | `server_name`: requires tls = true |
| `{ uri = "tel:+1234" }` | `uri`: must be a sip: or sips: URI |
| `{ from = "sip:probe@example.org" }` | none |
| `{ expect_status = [] }` | `expect_status`: must be a non-empty array |
| `{ expect_status = ["200", "180"] }` | `expect_status`: must be a status code from 200 to 699 or a range lo-hi |
| `{ expect_status = ["499-200"] }` | `expect_status`: must be a status code from 200 to 699 or a range lo-hi |
| `{ method = "REGISTER" }` | `method`: unknown key |


<!-- @trace
source: emfor-check-sip
updated: 2026-11-13
code:
  - docs/deploy/plugins.md
  - examples/emfor.toml
  - crates/emfor-check/src/radius.rs
  - crates/emfor-check/src/sip.rs
  - crates/emfor-check/Cargo.toml
  - tests/plugins/Dockerfile
  - docs/manual/04-health-checks.md
  - crates/emfor-check/src/options.rs
  - crates/emfor-check/src/net.rs
  - CHANGELOG.md
  - tests/plugins/inside.sh
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/main.rs
  - docs/manual.zh_TW/04-health-checks.md
  - crates/emfor-check/src/dns.rs
  - tests/plugins/run.sh
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: SIP check

The `sip` plugin SHALL send one OPTIONS request per check with Request-URI and To set to `uri`, From set to `from` with a random tag, a random Call-ID, `CSeq: 1 OPTIONS`, `Max-Forwards: 70`, `Content-Length: 0`, and one Via header naming the transport (`UDP`, `TCP` or `TLS`), the local address and port, a random `branch` parameter starting with `z9hG4bK`, and the `rport` parameter; lines SHALL end in CRLF. With `transport = "udp"` it SHALL use the UDP request and response exchange. With `transport = "tcp"` it SHALL connect to the backend (a failure is `down` with a reason starting `connect: `), perform the TLS handshake when `tls = true` (a failure is `down` with a reason starting `tls: `), write the request, and read responses one after another, each up to the end of its headers and then its Content-Length bytes of body; CRLFs before a status line SHALL be skipped (RFC 3261 section 7.5).

A response SHALL match the request when its first Via header has the request's `branch`, its Call-ID equals the request's, and its CSeq is `1 OPTIONS`; header names SHALL be compared without regard to case, with the compact forms `v`, `i` and `l` accepted for Via, Call-ID and Content-Length. Over UDP a datagram that does not match SHALL be ignored; over TCP a response that does not match SHALL be `down` with reason `query: response does not match the request`. A response whose status code is from 100 to 199 SHALL be ignored, and the plugin SHALL keep waiting for a final response.

A response that is not a SIP/2.0 status line followed by headers, lacks Via, Call-ID or CSeq, or is longer than 65536 bytes SHALL be `down` with a reason starting `query: `. A final response whose status code matches an entry of `expect_status` SHALL be `up`; any other final response SHALL be `down` with reason `status: <code> <reason phrase>`. The reason phrase MAY be empty (RFC 3261 section 25.1); the reason is then `status: <code>`.

#### Scenario: Server answers OPTIONS

- **WHEN** the backend answers the OPTIONS request over UDP with `SIP/2.0 200 OK` and matching Via branch, Call-ID and CSeq
- **THEN** the check result is `up`

#### Scenario: Server overloaded

- **WHEN** the backend answers with `SIP/2.0 503 Service Unavailable` and `expect_status` is the default
- **THEN** the check result is `down` with reason `status: 503 Service Unavailable`

##### Example: SIP outcomes

| Options | Backend answers | Result |
| --- | --- | --- |
| defaults | `200 OK` | `up` |
| defaults | `404 Not Found` | `up` |
| defaults | `407 Proxy Authentication Required` | `up` |
| defaults | `100 Trying`, then `200 OK` | `up` |
| defaults | `503 Service Unavailable` | `down`, `status: 503 Service Unavailable` |
| defaults | `603 Decline` | `down`, `status: 603 Decline` |
| `expect_status = ["200"]` | `404 Not Found` | `down`, `status: 404 Not Found` |
| defaults | `200 OK` with another Call-ID, then `200 OK` matching (UDP) | `up` |
| `transport = "tcp"` | `200 OK` with another Call-ID | `down`, `query: response does not match the request` |
| defaults | `HTTP/1.1 200 OK` | `down`, reason starting `query: ` |
| defaults | `200 OK` with compact headers `v:`, `i:`, `l: 0` | `up` |

#### Scenario: Request format

- **WHEN** a check over UDP from local address `10.0.0.5:40000` to backend `10.0.9.11:5060` uses branch `z9hG4bKabc`, tag `t1` and Call-ID `c1@emfor`
- **THEN** the datagram starts with `OPTIONS sip:10.0.9.11:5060 SIP/2.0\r\nVia: SIP/2.0/UDP 10.0.0.5:40000;branch=z9hG4bKabc;rport\r\n` and contains `From: <sip:emfor@emfor.invalid>;tag=t1`, `To: <sip:10.0.9.11:5060>`, `Call-ID: c1@emfor` and `CSeq: 1 OPTIONS`

#### Scenario: SIP over TLS with a wrong certificate name

- **WHEN** a check has `transport = "tcp"`, `tls = true`, `server_name = "sbc.example.org"` and the backend presents a certificate for another name
- **THEN** the check result is `down` with a reason starting `tls: `

<!-- @trace
source: emfor-check-sip
updated: 2026-11-13
code:
  - docs/deploy/plugins.md
  - examples/emfor.toml
  - crates/emfor-check/src/radius.rs
  - crates/emfor-check/src/sip.rs
  - crates/emfor-check/Cargo.toml
  - tests/plugins/Dockerfile
  - docs/manual/04-health-checks.md
  - crates/emfor-check/src/options.rs
  - crates/emfor-check/src/net.rs
  - CHANGELOG.md
  - tests/plugins/inside.sh
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/main.rs
  - docs/manual.zh_TW/04-health-checks.md
  - crates/emfor-check/src/dns.rs
  - tests/plugins/run.sh
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: Diameter check options

The `diameter` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `origin_host` and `origin_realm` (both required): DiameterIdentity strings of 1 to 255 characters made of ASCII letters, digits, `-` and `.`, not starting or ending with `.`. Otherwise the error is `not a valid DiameterIdentity`.
- `auth_application_ids`, `acct_application_ids` and `supported_vendor_ids`: arrays of integers from 0 to 4294967295, default empty. Otherwise the error is `must be an array of integers from 0 to 4294967295`.
- `vendor_specific_application_ids`: an array of tables, default empty, each with exactly the keys `vendor_id` and one of `auth_application_id` or `acct_application_id`, all integers from 0 to 4294967295. Otherwise the error is `each entry needs vendor_id and one of auth_application_id, acct_application_id`.
- `host_ip_address`: an IPv4 or IPv6 address. Otherwise the error is `not an IP address`.
- `tls`, `tls_verify`, `server_name` and `ca_file` with the rules they have for the other `emfor-check` plugins, including `requires tls = true`.
- `tls_cert_file` and `tls_key_file`: absolute paths of a PEM client certificate chain and of its PEM private key, set together and only with `tls = true` (otherwise `requires tls = true`). When only one is set, the error is on that key: `requires tls_key_file` or `requires tls_cert_file`. A path that is not absolute is `not an absolute path`; when options are validated, a file that cannot be read is `file not found` or `cannot read: <kind>`, a certificate file without a PEM certificate is `no certificate found`, and a key file without a PEM private key is `no private key found`.

When `auth_application_ids`, `acct_application_ids` and `vendor_specific_application_ids` are all empty or absent, the plugin SHALL report an error on `auth_application_ids`: `at least one application is required`. A missing `origin_host` or `origin_realm` SHALL be `missing required key`; type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Gx peer options

- **WHEN** a service with `check = "diameter"` sets `check_options = { origin_host = "emfor-app01.example.org", origin_realm = "example.org", vendor_specific_application_ids = [{ vendor_id = 10415, auth_application_id = 16777238 }], supported_vendor_ids = [10415] }`
- **THEN** the plugin reports no errors

##### Example: Diameter option validation

| `check_options` | Errors |
| --- | --- |
| `{ origin_realm = "example.org", auth_application_ids = [4] }` | `origin_host`: missing required key |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org" }` | `auth_application_ids`: at least one application is required |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", auth_application_ids = [4] }` | none |
| `{ origin_host = "emfor example", origin_realm = "example.org", auth_application_ids = [4] }` | `origin_host`: not a valid DiameterIdentity |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", auth_application_ids = [-1] }` | `auth_application_ids`: must be an array of integers from 0 to 4294967295 |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", vendor_specific_application_ids = [{ vendor_id = 10415 }] }` | `vendor_specific_application_ids`: each entry needs vendor_id and one of auth_application_id, acct_application_id |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", acct_application_ids = [3], host_ip_address = "emfor01" }` | `host_ip_address`: not an IP address |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", acct_application_ids = [3], ca_file = "/etc/emfor/ca.pem" }` | `ca_file`: requires tls = true |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", acct_application_ids = [3], tls = true, tls_cert_file = "/etc/emfor/diameter.crt" }` | `tls_cert_file`: requires tls_key_file |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", acct_application_ids = [3], tls_cert_file = "/etc/emfor/diameter.crt", tls_key_file = "/etc/emfor/diameter.key" }` | `tls_cert_file`: requires tls = true; `tls_key_file`: requires tls = true |


<!-- @trace
source: emfor-check-diameter
updated: 2026-11-13
code:
  - examples/emfor.toml
  - tests/plugins/run.sh
  - crates/emfor-check/src/options.rs
  - docs/manual.zh_TW/04-health-checks.md
  - crates/emfor-tls/src/testing.rs
  - tests/plugins/Dockerfile
  - docs/deploy/plugins.md
  - crates/emfor-check/src/diameter.rs
  - crates/emfor-check/src/main.rs
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/Cargo.toml
  - crates/emfor-tls/src/tests.rs
  - tests/plugins/inside.sh
  - CHANGELOG.md
  - crates/emfor-tls/src/lib.rs
  - docs/manual/04-health-checks.md
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: Diameter check

The `diameter` plugin SHALL open a new TCP connection to the backend on every check (a failure is `down` with a reason starting `connect: `) and, with `tls = true`, perform the TLS handshake at once (a failure is `down` with a reason starting `tls: `). With `tls_cert_file` and `tls_key_file` it SHALL present that certificate chain when the server asks for a client certificate; both files SHALL be read on every check, and a file that cannot be read or holds no certificate or no private key SHALL be `error` with a reason starting `tls_cert_file: ` or `tls_key_file: `, with nothing sent. It SHALL send a Capabilities-Exchange-Request (command code 257, R flag, Application-Id 0, random Hop-by-Hop and End-to-End identifiers) carrying Origin-Host, Origin-Realm, Host-IP-Address (`host_ip_address`, or the local address of the connection), Vendor-Id 0, Product-Name `emfor`, one Supported-Vendor-Id per `supported_vendor_ids` entry, one Auth-Application-Id per `auth_application_ids` entry, one Acct-Application-Id per `acct_application_ids` entry and one grouped Vendor-Specific-Application-Id per `vendor_specific_application_ids` entry, with the M flag set on every AVP except Product-Name.

A message SHALL be the answer to a request when its R flag is clear and its command code and Hop-by-Hop identifier equal the request's; requests from the peer SHALL be ignored and not answered. The plugin SHALL judge in this order:

1. A message whose version is not 1, whose length is below 20 or disagrees with its AVPs, whose AVP length is invalid, or that is longer than 65536 bytes SHALL be `down` with a reason starting `query: `.
2. The connection closed by the peer before the answer SHALL be `down` with reason `cea: connection closed by peer` before the Capabilities-Exchange-Answer and `dwa: connection closed by peer` before the Device-Watchdog-Answer.
3. An answer without Result-Code SHALL be `down` with reason `query: answer without Result-Code`.
4. A Capabilities-Exchange-Answer whose Result-Code is not from 2000 to 2999 SHALL be `down` with reason `cea: <code> <name>`; otherwise the plugin SHALL send a Device-Watchdog-Request (command code 280, R flag, Origin-Host, Origin-Realm), and a Device-Watchdog-Answer whose Result-Code is not from 2000 to 2999 SHALL be `down` with reason `dwa: <code> <name>`. `<name>` SHALL be the result code name defined in RFC 6733 section 7.1, such as `DIAMETER_UNKNOWN_PEER` for 3010, and SHALL be left out (keeping only the code) for codes RFC 6733 does not define.
5. A successful Device-Watchdog-Answer SHALL make the check `up`.

After a successful Capabilities-Exchange-Answer, whatever the result, the plugin SHALL send a Disconnect-Peer-Request (command code 282) with Disconnect-Cause 2 (`DO_NOT_WANT_TO_TALK_TO_YOU`), wait at most 200 ms or the time left before `timeout_ms`, whichever is shorter, for the Disconnect-Peer-Answer, and close the connection; the Disconnect-Peer-Answer SHALL NOT change the result.

#### Scenario: Peer accepts the connection

- **WHEN** the backend answers the CER with Result-Code 2001 and the DWR with Result-Code 2001
- **THEN** the check result is `up` and the plugin sends a DPR with Disconnect-Cause 2 before closing

#### Scenario: Unknown peer

- **WHEN** the backend answers the CER with Result-Code 3010 and the E flag set
- **THEN** the check result is `down` with reason `cea: 3010 DIAMETER_UNKNOWN_PEER`, and no DWR is sent

##### Example: Diameter outcomes

| Backend behavior | Result |
| --- | --- |
| CEA 2001, DWA 2001 | `up` |
| CEA 2002, DWA 2001 | `up` |
| CEA 5010 | `down`, `cea: 5010 DIAMETER_NO_COMMON_APPLICATION` |
| CEA 2001, DWA 3004 | `down`, `dwa: 3004 DIAMETER_TOO_BUSY` |
| CEA 4999 | `down`, `cea: 4999` |
| closes the connection after reading the CER | `down`, `cea: connection closed by peer` |
| CEA 2001, its own DWR, then DWA 2001 | `up` (the peer's DWR is ignored) |
| CEA without Result-Code | `down`, `query: answer without Result-Code` |
| a header with version 2 | `down`, reason starting `query: ` |

#### Scenario: CER encoding

- **WHEN** a CER is built with Hop-by-Hop and End-to-End identifiers 1, `origin_host = "emfor.example.org"`, `origin_realm = "example.org"`, local address `10.0.0.5` and `auth_application_ids = [4]`
- **THEN** it starts with the header bytes `01 <length> 80 00 01 01 00 00 00 00 00 00 00 01 00 00 00 01`, its first AVP is Origin-Host with code 264, flags `40` and the value `emfor.example.org` padded to a multiple of 4 bytes, and its Host-IP-Address value is `00 01 0a 00 00 05`

#### Scenario: Peer requires a client certificate

- **WHEN** the backend accepts TLS only with a client certificate signed by its CA, and the check sets `tls = true`, `tls_cert_file` and `tls_key_file` to such a certificate and its key
- **THEN** the check result is `up`

#### Scenario: Unreadable client key

- **WHEN** the check sets `tls = true`, `tls_cert_file` and `tls_key_file`, and the key file was removed after the options were validated
- **THEN** the check result is `error` with a reason starting `tls_key_file: `, and no connection is opened

#### Scenario: Silent peer

- **WHEN** the backend accepts the TCP connection and never answers the CER, and the check has `timeout_ms` 900
- **THEN** the check result is `down` with reason `timeout`

<!-- @trace
source: emfor-check-diameter
updated: 2026-11-13
code:
  - examples/emfor.toml
  - tests/plugins/run.sh
  - crates/emfor-check/src/options.rs
  - docs/manual.zh_TW/04-health-checks.md
  - crates/emfor-tls/src/testing.rs
  - tests/plugins/Dockerfile
  - docs/deploy/plugins.md
  - crates/emfor-check/src/diameter.rs
  - crates/emfor-check/src/main.rs
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/Cargo.toml
  - crates/emfor-tls/src/tests.rs
  - tests/plugins/inside.sh
  - CHANGELOG.md
  - crates/emfor-tls/src/lib.rs
  - docs/manual/04-health-checks.md
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: SMTP check options

The `smtp` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `helo`: a host name of 1 to 255 bytes made of ASCII letters, digits, `-` and `.`. Otherwise the error is `must be a host name`. When it is not set, the EHLO argument SHALL be the local address of the connection as an address literal: `[<IPv4>]` or `[IPv6:<IPv6>]`.
- `tls` and `starttls`: booleans, default `false`. Both `true` SHALL be an error on `starttls`: `conflicts with tls`.
- `tls_verify`, `server_name` and `ca_file`: the rules they have for the other `emfor-check` plugins, except that without `tls = true` or `starttls = true` each SHALL be an error: `requires tls = true or starttls = true`.

Type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Defaults

- **WHEN** a service with `check = "smtp"` sets no `check_options` and the plugin connects from `10.0.0.5`
- **THEN** the plugin reports no errors and sends `EHLO [10.0.0.5]` in plaintext

##### Example: SMTP option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | none |
| `{ helo = "app1.example.org" }` | none |
| `{ helo = "bad name" }` | `helo`: must be a host name |
| `{ helo = "" }` | `helo`: must be a host name |
| `{ starttls = true, ca_file = "/etc/emfor/ca.pem" }` | none |
| `{ tls = true, starttls = true }` | `starttls`: conflicts with tls |
| `{ server_name = "mx.example.org" }` | `server_name`: requires tls = true or starttls = true |
| `{ starttls = "yes" }` | `starttls`: must be a boolean |
| `{ user = "emfor" }` | `user`: unknown key |


<!-- @trace
source: emfor-check-smtp
updated: 2026-11-14
code:
  - crates/emfor-check/src/amqp.rs
  - crates/emfor-check/src/smtp.rs
  - crates/emfor-check/Cargo.toml
  - tests/plugins/grpc_server.py
  - crates/emfor-check/src/replica.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-check/src/redis.rs
  - crates/emfor-check/src/mysql.rs
  - examples/emfor.toml
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/mongodb.rs
  - crates/emfor-check/src/grpc.rs
  - tests/plugins/inside.sh
  - crates/emfor-check/src/radius.rs
  - tests/plugins/run.sh
  - CHANGELOG.md
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/main.rs
  - crates/emfor-daemon/src/checker/http.rs
  - docs/manual/04-health-checks.md
  - crates/emfor-check/src/options.rs
  - examples/freebsd/Makefile.crates
  - docs/deploy/plugins.md
  - docs/manual.zh_TW/04-health-checks.md
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: SMTP check

The `smtp` plugin SHALL open a new TCP connection per check (a failure is `down` with a reason starting `connect: `) and, with `tls = true`, perform the TLS handshake first. It SHALL then read the greeting, send `EHLO <helo>`, and with `starttls = true` send `STARTTLS`, perform the TLS handshake on the same connection and send `EHLO <helo>` again; finally it SHALL send `QUIT` and close the connection without waiting for the answer. Commands SHALL end in CRLF. A reply SHALL be read line by line, each line ending in LF with an optional CR before it, a line starting with three digits and `-` continuing the reply and a line starting with the same three digits and a space or nothing else ending it. A line longer than 2048 bytes or a reply longer than 65536 bytes SHALL be `down` with reason `query: reply too long`; a line that does not start with three digits, or a continuation with another code, SHALL be `down` with reason `query: malformed reply`; the connection closing before a reply ends SHALL be `down` with reason `query: connection closed`. The result SHALL be `up` only when the greeting is 220 and each EHLO is answered 250, and with `starttls = true` the first EHLO reply has a line whose text after the code is `STARTTLS` (case-insensitive), `STARTTLS` is answered 220 and the TLS handshake succeeds. Otherwise it SHALL be `down` with these reasons, where `<code> <text>` is the code and text of the reply's first line:

- greeting other than 220: `greeting: <code> <text>`;
- EHLO other than 250: `ehlo: <code> <text>`;
- STARTTLS not in the EHLO reply: `tls: STARTTLS not offered`, without sending `STARTTLS`;
- STARTTLS answered with another code: `tls: STARTTLS refused: <code> <text>`;
- a TLS handshake or certificate failure: a reason starting `tls: `.

The plugin SHALL NOT continue in plaintext after `STARTTLS` fails.

#### Scenario: Healthy server with STARTTLS

- **WHEN** `starttls = true`, the server greets with `220 mx ESMTP`, answers EHLO with `250-mx`, `250-STARTTLS` and `250 8BITMIME`, answers STARTTLS with `220 ready`, completes the TLS handshake and answers the second EHLO with 250
- **THEN** the check result is `up`

#### Scenario: STARTTLS not offered

- **WHEN** `starttls = true` and the EHLO reply has no `STARTTLS` line
- **THEN** the check result is `down` with reason `tls: STARTTLS not offered` and the plugin sends no `STARTTLS` command

##### Example: SMTP replies

| Options | Server behavior | Result |
| --- | --- | --- |
| `{}` | `220 mx ESMTP`, EHLO `250 mx` | up |
| `{}` | `554 5.3.2 no service` | down: `greeting: 554 5.3.2 no service` |
| `{}` | `421 4.3.2 busy` | down: `greeting: 421 4.3.2 busy` |
| `{}` | `220-mx`, `220 ESMTP` (two lines), EHLO `250 mx` | up |
| `{}` | `220 mx`, EHLO `550 5.7.1 go away` | down: `ehlo: 550 5.7.1 go away` |
| `{}` | `220-mx`, `250 x` | down: `query: malformed reply` |
| `{}` | `hello` | down: `query: malformed reply` |
| `{}` | `220 mx`, then closes | down: `query: connection closed` |
| `{}` | a 3000-byte greeting line | down: `query: reply too long` |
| `{ starttls = true }` | EHLO `250-mx`, `250 starttls` (lower case), STARTTLS `220`, TLS, EHLO `250 mx` | up |
| `{ starttls = true }` | EHLO `250 mx` | down: `tls: STARTTLS not offered` |
| `{ starttls = true }` | EHLO `250-mx`, `250 STARTTLS`, STARTTLS `454 4.7.0 TLS not available` | down: `tls: STARTTLS refused: 454 4.7.0 TLS not available` |
| `{ tls = true }` | plaintext `220 mx` instead of a TLS handshake | down: reason starts with `tls: ` |

<!-- @trace
source: emfor-check-smtp
updated: 2026-11-14
code:
  - crates/emfor-check/src/amqp.rs
  - crates/emfor-check/src/smtp.rs
  - crates/emfor-check/Cargo.toml
  - tests/plugins/grpc_server.py
  - crates/emfor-check/src/replica.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-check/src/redis.rs
  - crates/emfor-check/src/mysql.rs
  - examples/emfor.toml
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/mongodb.rs
  - crates/emfor-check/src/grpc.rs
  - tests/plugins/inside.sh
  - crates/emfor-check/src/radius.rs
  - tests/plugins/run.sh
  - CHANGELOG.md
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/main.rs
  - crates/emfor-daemon/src/checker/http.rs
  - docs/manual/04-health-checks.md
  - crates/emfor-check/src/options.rs
  - examples/freebsd/Makefile.crates
  - docs/deploy/plugins.md
  - docs/manual.zh_TW/04-health-checks.md
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: AMQP check options

The `amqp` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `user`: required, a string of 1 to 255 bytes. A missing key is `missing required key`; an empty or longer string is `must be 1 to 255 bytes`.
- `password_file`: required, with the rules it has for the other `emfor-check` plugins.
- `vhost`: a string of 1 to 255 bytes, default `/`. Otherwise the error is `must be 1 to 255 bytes`.
- `tls`, `tls_verify`, `server_name` and `ca_file`: the rules they have for the `redis`, `mysql` and `pgsql` plugins, including the error `requires tls = true`.

Type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Minimal options

- **WHEN** a service with `check = "amqp"` sets `check_options = { user = "emfor", password_file = "/etc/emfor/amqp.pass" }`
- **THEN** the plugin reports no errors and opens vhost `/`

##### Example: AMQP option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | `user`: missing required key; `password_file`: missing required key |
| `{ user = "emfor" }` | `password_file`: missing required key |
| `{ user = "emfor", password_file = "/etc/emfor/amqp.pass", vhost = "orders" }` | none |
| `{ user = "emfor", password_file = "/etc/emfor/amqp.pass", vhost = "" }` | `vhost`: must be 1 to 255 bytes |
| `{ user = "", password_file = "/etc/emfor/amqp.pass" }` | `user`: must be 1 to 255 bytes |
| `{ user = "emfor", password_file = "/etc/emfor/amqp.pass", server_name = "mq.internal" }` | `server_name`: requires tls = true |
| `{ user = "emfor", password_file = "/etc/emfor/amqp.pass", password = "x" }` | `password`: unknown key |


<!-- @trace
source: emfor-check-amqp
updated: 2026-11-14
code:
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/grpc.rs
  - examples/freebsd/Makefile.crates
  - crates/emfor-check/src/amqp.rs
  - docs/deploy/plugins.md
  - tests/plugins/run.sh
  - crates/emfor-check/src/main.rs
  - examples/emfor.toml
  - docs/manual.zh_TW/04-health-checks.md
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-check/src/mongodb.rs
  - crates/emfor-check/src/mysql.rs
  - docs/manual/04-health-checks.md
  - crates/emfor-check/src/redis.rs
  - tests/plugins/grpc_server.py
  - crates/emfor-check/src/radius.rs
  - crates/emfor-check/src/smtp.rs
  - crates/emfor-daemon/src/checker/http.rs
  - CHANGELOG.md
  - tests/plugins/Dockerfile
  - tests/plugins/inside.sh
  - crates/emfor-check/src/replica.rs
  - crates/emfor-check/src/options.rs
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: AMQP check

The `amqp` plugin SHALL open a new TCP connection per check (a failure is `down` with a reason starting `connect: `), perform the TLS handshake first when `tls = true` (a failure is `down` with a reason starting `tls: `), and speak AMQP 0-9-1 on it:

1. Send the protocol header `AMQP` followed by the bytes 0, 0, 9, 1.
2. Receive Connection.Start. When its mechanisms do not include `PLAIN`, the result SHALL be `down` with reason `auth: server does not offer PLAIN`.
3. Send Connection.Start-Ok with mechanism `PLAIN`, response `\0<user>\0<password>`, locale `en_US`, and client properties containing `product` = `emfor` and a `capabilities` table with `authentication_failure_close` = true.
4. Receive Connection.Tune and send Connection.Tune-Ok with the proposed channel-max, the proposed frame-max (131072 when it is 0) and heartbeat 0.
5. Send Connection.Open with `vhost` and receive Connection.Open-Ok; the result is then `up`.
6. Send Connection.Close with reply code 200, wait up to 200 ms for Connection.Close-Ok, and close the connection.

Each frame SHALL be read as type, channel, size, payload and the end byte `0xCE`. Heartbeat frames SHALL be skipped. A size above 131072 SHALL be `down` with reason `query: frame too large`; a wrong end byte or a payload that cannot be decoded SHALL be `down` with reason `query: malformed frame`; the connection closing before the expected frame SHALL be `down` with reason `query: connection closed`, except after Start-Ok, where it SHALL be `auth: connection closed after login`. When the server answers the protocol header with its own protocol header, the result SHALL be `down` with reason `query: server speaks AMQP <a>-<b>-<c>`, the last three bytes of that header. Another method than the expected one SHALL be `down` with reason `query: unexpected method <class-id>.<method-id>`.

When the server sends Connection.Close, the plugin SHALL answer Connection.Close-Ok, close the connection, and the result SHALL be `down` with reason `<prefix>: <reply-code> <reply-text>`, where the prefix is `auth` when the Close arrives after Start-Ok and before Tune, `vhost` when it arrives after Open, and `query` otherwise.

#### Scenario: Successful handshake

- **WHEN** a broker offers `PLAIN AMQPLAIN`, accepts the login, proposes channel-max 2047, frame-max 131072 and heartbeat 60, and answers Open for vhost `/` with Open-Ok
- **THEN** the check result is `up`, and the plugin sent Tune-Ok with channel-max 2047, frame-max 131072 and heartbeat 0

#### Scenario: Wrong password

- **WHEN** the broker answers Start-Ok with Connection.Close 403 `ACCESS_REFUSED - Login was refused using authentication mechanism PLAIN`
- **THEN** the check result is `down` with reason `auth: 403 ACCESS_REFUSED - Login was refused using authentication mechanism PLAIN`, and the reason does not contain the password

##### Example: AMQP broker answers

| Broker behavior | Result |
| --- | --- |
| Start, Tune, Open-Ok | up |
| Start with mechanisms `AMQPLAIN` | down: `auth: server does not offer PLAIN` |
| Start, then Close 403 `ACCESS_REFUSED - Login was refused` after Start-Ok | down: `auth: 403 ACCESS_REFUSED - Login was refused` |
| Start, then closes the connection after Start-Ok | down: `auth: connection closed after login` |
| Start, Tune, then Close 530 `NOT_ALLOWED - vhost orders not found` after Open | down: `vhost: 530 NOT_ALLOWED - vhost orders not found` |
| a heartbeat frame between Tune and Open-Ok | up |
| `AMQP` 0 1 0 0 instead of Start | down: `query: server speaks AMQP 1-0-0` |
| a frame with size 200000 | down: `query: frame too large` |
| a frame ending in `0x00` | down: `query: malformed frame` |
| Connection.Secure (10.20) instead of Tune | down: `query: unexpected method 10.20` |
| closes the connection before Start | down: `query: connection closed` |
| Start, Tune, Open-Ok, no Close-Ok within 200 ms | up |

<!-- @trace
source: emfor-check-amqp
updated: 2026-11-14
code:
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/grpc.rs
  - examples/freebsd/Makefile.crates
  - crates/emfor-check/src/amqp.rs
  - docs/deploy/plugins.md
  - tests/plugins/run.sh
  - crates/emfor-check/src/main.rs
  - examples/emfor.toml
  - docs/manual.zh_TW/04-health-checks.md
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-check/src/mongodb.rs
  - crates/emfor-check/src/mysql.rs
  - docs/manual/04-health-checks.md
  - crates/emfor-check/src/redis.rs
  - tests/plugins/grpc_server.py
  - crates/emfor-check/src/radius.rs
  - crates/emfor-check/src/smtp.rs
  - crates/emfor-daemon/src/checker/http.rs
  - CHANGELOG.md
  - tests/plugins/Dockerfile
  - tests/plugins/inside.sh
  - crates/emfor-check/src/replica.rs
  - crates/emfor-check/src/options.rs
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: MongoDB check options

The `mongodb` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `user`: a string of 1 to 255 bytes, otherwise `must be 1 to 255 bytes`. Without `password_file` it SHALL be an error on `user`: `requires password_file`.
- `password_file`: the rules it has for the other `emfor-check` plugins. Without `user` it SHALL be an error on `password_file`: `requires user`.
- `auth_source`: 1 to 64 ASCII letters, digits, `_` or `-`, default `admin`, otherwise `must be a database name`. Without `user` it SHALL be an error: `requires user`.
- `role`: `primary` or `secondary`, otherwise `must be one of primary, secondary`.
- `tls`, `tls_verify`, `server_name` and `ca_file`: the rules they have for the `redis`, `mysql` and `pgsql` plugins, including the error `requires tls = true`.

Type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: No credentials

- **WHEN** a service with `check = "mongodb"` sets `check_options = { role = "primary" }`
- **THEN** the plugin reports no errors and checks without logging in

##### Example: MongoDB option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | none |
| `{ user = "emfor", password_file = "/etc/emfor/mongo.pass", auth_source = "app" }` | none |
| `{ user = "emfor" }` | `user`: requires password_file |
| `{ password_file = "/etc/emfor/mongo.pass" }` | `password_file`: requires user |
| `{ auth_source = "app" }` | `auth_source`: requires user |
| `{ user = "emfor", password_file = "/etc/emfor/mongo.pass", auth_source = "a.b" }` | `auth_source`: must be a database name |
| `{ role = "replica" }` | `role`: must be one of primary, secondary |
| `{ server_name = "db.internal" }` | `server_name`: requires tls = true |
| `{ database = "app" }` | `database`: unknown key |


<!-- @trace
source: emfor-check-mongodb
updated: 2026-11-14
code:
  - crates/emfor-check/Cargo.toml
  - CHANGELOG.md
  - docs/manual/04-health-checks.md
  - crates/emfor-check/src/replica.rs
  - crates/emfor-check/src/amqp.rs
  - crates/emfor-check/src/pgsql.rs
  - docs/manual.zh_TW/04-health-checks.md
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/options.rs
  - tests/plugins/run.sh
  - crates/emfor-check/src/mysql.rs
  - crates/emfor-check/src/radius.rs
  - crates/emfor-check/src/redis.rs
  - tests/plugins/Dockerfile
  - tests/plugins/grpc_server.py
  - examples/emfor.toml
  - examples/freebsd/Makefile.crates
  - crates/emfor-check/src/smtp.rs
  - crates/emfor-check/src/mongodb.rs
  - tests/plugins/inside.sh
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-check/src/main.rs
  - crates/emfor-check/src/grpc.rs
  - docs/deploy/plugins.md
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: MongoDB check

The `mongodb` plugin SHALL open a new TCP connection per check (a failure is `down` with a reason starting `connect: `) and, with `tls = true`, perform the TLS handshake first (a failure is `down` with a reason starting `tls: `). It SHALL send commands as OP_MSG messages (opCode 2013, flagBits 0, one kind 0 section) and read each reply as one OP_MSG message of at most 65536 bytes, ignoring a trailing checksum when the reply's checksumPresent flag is set. A larger reply SHALL be `down` with reason `query: reply too large`; a reply that is not an OP_MSG answering the request or whose BSON cannot be decoded SHALL be `down` with reason `query: malformed reply`; the connection closing before a reply is complete SHALL be `down` with reason `query: connection closed`.

The plugin SHALL first send `{hello: 1, $db: "admin"}`. With `user` set it SHALL then authenticate with SCRAM-SHA-256 (RFC 7677) against `auth_source` using `saslStart` with `options: {skipEmptyExchange: true}` and `saslContinue`, escaping `=` and `,` in the user name as RFC 5802 requires, and SHALL verify the server signature. A password that is not printable ASCII SHALL be `error` with reason `password_file: only printable ASCII passwords are supported`. A command answered with `ok: 0` SHALL be `down` with reason `<prefix>: <codeName>: <errmsg>`, where the prefix is `auth` for `saslStart` and `saslContinue` and `query` for `hello`. A server-first-message with an iteration count below 4096, a nonce that does not start with the client nonce, or a missing field SHALL be `down` with reason `auth: malformed SCRAM message`; a server signature that does not match SHALL be `down` with reason `auth: server signature does not match`.

With `role` set, the plugin SHALL derive the actual role from the `hello` reply in this order: `arbiter` when `arbiterOnly` is true; `mongos` when `msg` is `isdbgrid`; `primary` when `isWritablePrimary` is true and `setName` is present; `standalone` when `isWritablePrimary` is true without `setName`; `secondary` when `secondary` is true; otherwise `unknown`. `role = "primary"` SHALL accept `primary`, `standalone` and `mongos`; `role = "secondary"` SHALL accept `secondary` only. Any other actual role SHALL be `down` with reason `role is <actual>, want <role>`. The result SHALL be `up` when every step succeeded and the role, if set, is accepted.

#### Scenario: Primary of a replica set

- **WHEN** `role = "primary"` and the server answers `hello` with `isWritablePrimary: true` and `setName: "rs0"`
- **THEN** the check result is `up`

#### Scenario: SCRAM example from RFC 7677

- **WHEN** the plugin authenticates user `user` with password `pencil`, client nonce `rOprNGfwEbeRWgbNEkqO`, and the server-first-message `r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096`
- **THEN** the client-final-message is `c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=` and the server signature `v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=` is accepted

##### Example: MongoDB replies

| Options | Server behavior | Result |
| --- | --- | --- |
| `{}` | `hello` ok, standalone | up |
| `{ role = "primary" }` | `hello` with `isWritablePrimary: true`, `setName: "rs0"` | up |
| `{ role = "primary" }` | `hello` with `isWritablePrimary: true`, no `setName` | up |
| `{ role = "primary" }` | `hello` with `isWritablePrimary: true`, `msg: "isdbgrid"` | up |
| `{ role = "primary" }` | `hello` with `secondary: true`, `setName: "rs0"` | down: `role is secondary, want primary` |
| `{ role = "secondary" }` | `hello` with `secondary: true`, `setName: "rs0"` | up |
| `{ role = "secondary" }` | `hello` with `isWritablePrimary: true`, no `setName` | down: `role is standalone, want secondary` |
| `{ role = "primary" }` | `hello` with `arbiterOnly: true`, `secondary: false` | down: `role is arbiter, want primary` |
| `{ role = "secondary" }` | `hello` with `isWritablePrimary: false`, `secondary: false`, `setName: "rs0"` | down: `role is unknown, want secondary` |
| `{}` | `hello` answered `ok: 0`, `codeName: "CommandNotFound"`, `errmsg: "no such command: 'hello'"` | down: `query: CommandNotFound: no such command: 'hello'` |
| user and password | `saslStart` answered `ok: 0`, `codeName: "AuthenticationFailed"`, `errmsg: "Authentication failed."` | down: `auth: AuthenticationFailed: Authentication failed.` |
| user and password | server-first-message with `i=1000` | down: `auth: malformed SCRAM message` |
| user and password | server-final-message with a wrong `v=` | down: `auth: server signature does not match` |
| user and password (`pässword`) | (no request sent after `hello`) | error: `password_file: only printable ASCII passwords are supported` |
| `{}` | a reply of 70000 bytes | down: `query: reply too large` |
| `{}` | closes the connection after the request | down: `query: connection closed` |

<!-- @trace
source: emfor-check-mongodb
updated: 2026-11-14
code:
  - crates/emfor-check/Cargo.toml
  - CHANGELOG.md
  - docs/manual/04-health-checks.md
  - crates/emfor-check/src/replica.rs
  - crates/emfor-check/src/amqp.rs
  - crates/emfor-check/src/pgsql.rs
  - docs/manual.zh_TW/04-health-checks.md
  - crates/emfor-check/src/lib.rs
  - crates/emfor-check/src/options.rs
  - tests/plugins/run.sh
  - crates/emfor-check/src/mysql.rs
  - crates/emfor-check/src/radius.rs
  - crates/emfor-check/src/redis.rs
  - tests/plugins/Dockerfile
  - tests/plugins/grpc_server.py
  - examples/emfor.toml
  - examples/freebsd/Makefile.crates
  - crates/emfor-check/src/smtp.rs
  - crates/emfor-check/src/mongodb.rs
  - tests/plugins/inside.sh
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-check/src/main.rs
  - crates/emfor-check/src/grpc.rs
  - docs/deploy/plugins.md
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: gRPC check options

The `grpc` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `service`: a string of 0 to 256 bytes, default the empty string (the whole server). A longer string is `must be at most 256 bytes`.
- `tls`, `tls_verify`, `server_name` and `ca_file`: the rules they have for the `redis`, `mysql` and `pgsql` plugins, including the error `requires tls = true`.

Type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Defaults

- **WHEN** a service with `check = "grpc"` sets no `check_options`
- **THEN** the plugin reports no errors and checks the whole server over plaintext HTTP/2

##### Example: gRPC option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | none |
| `{ service = "orders.v1.Orders" }` | none |
| `{ service = "" }` | none |
| `{ service = "<300 bytes>" }` | `service`: must be at most 256 bytes |
| `{ tls = true, server_name = "orders.internal" }` | none |
| `{ server_name = "orders.internal" }` | `server_name`: requires tls = true |
| `{ service = 1 }` | `service`: must be a string |
| `{ authority = "orders" }` | `authority`: unknown key |


<!-- @trace
source: emfor-check-grpc
updated: 2026-11-14
code:
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/amqp.rs
  - crates/emfor-check/src/mysql.rs
  - crates/emfor-check/src/lib.rs
  - docs/manual/04-health-checks.md
  - crates/emfor-check/src/smtp.rs
  - examples/emfor.toml
  - crates/emfor-check/src/mongodb.rs
  - crates/emfor-check/src/redis.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-daemon/src/checker/http.rs
  - CHANGELOG.md
  - examples/freebsd/Makefile.crates
  - crates/emfor-check/src/radius.rs
  - docs/deploy/plugins.md
  - crates/emfor-check/src/grpc.rs
  - crates/emfor-check/src/replica.rs
  - tests/plugins/run.sh
  - tests/plugins/grpc_server.py
  - tests/plugins/inside.sh
  - crates/emfor-check/src/options.rs
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/main.rs
  - docs/manual.zh_TW/04-health-checks.md
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: gRPC check

The `grpc` plugin SHALL open a new TCP connection per check (a failure is `down` with a reason starting `connect: `). With `tls = true` it SHALL perform the TLS handshake offering ALPN `h2` (a failure is `down` with a reason starting `tls: `; a server that does not select `h2` is `down` with reason `tls: server did not select h2 (ALPN)`); without TLS it SHALL start HTTP/2 with the connection preface (prior knowledge). It SHALL send one request: `POST /grpc.health.v1.Health/Check` with `:authority` set to `server_name` when it is set and otherwise to the backend address (`ip:port`, IPv6 in brackets), headers `content-type: application/grpc`, `te: trailers` and `user-agent: emfor-check/<package version>`, and a body of one uncompressed message holding `HealthCheckRequest` with field 1 set to `service`. It SHALL then read the response headers, the body and the trailers, and close the connection.

The result SHALL be decided in this order, and the first matching rule SHALL apply:

1. An HTTP/2 connection or stream error: `down`, `query: HTTP/2: <error>`.
2. A status other than 200: `down`, `query: HTTP status <status>`.
3. A `content-type` that does not start with `application/grpc`: `down`, `query: not a gRPC response (content-type <value>)`.
4. `grpc-status` (from the trailers, or from the headers when the response has no body) other than 0: `down`, `grpc: <code> <NAME>`, followed by `: ` and the percent-decoded `grpc-message` when it is present and not empty; `<NAME>` is the gRPC status code name, or `UNKNOWN_CODE` for a code above 16.
5. No `grpc-status`: `down`, `query: no grpc-status`.
6. The message: a compressed flag of 1 is `down`, `query: compressed message`; a length above 65536 is `down`, `query: reply too large`; no message is `down`, `query: no message`; protobuf that cannot be decoded is `down`, `query: malformed message`. Fields other than 1 SHALL be skipped by wire type, and a missing field 1 SHALL read as 0.
7. The health status: 1 (SERVING) is `up`; 0, 2 and 3 are `down` with reasons `status: UNKNOWN`, `status: NOT_SERVING` and `status: SERVICE_UNKNOWN`; any other value is `down` with reason `status: <value>`.

#### Scenario: Serving

- **WHEN** the server answers 200 with `content-type: application/grpc`, a message `08 01` and trailers `grpc-status: 0`
- **THEN** the check result is `up`

#### Scenario: No health service

- **WHEN** the server answers with headers only, holding `grpc-status: 12`
- **THEN** the check result is `down` with reason `grpc: 12 UNIMPLEMENTED`

##### Example: gRPC responses

| Server behavior | Result |
| --- | --- |
| 200, message `08 01`, `grpc-status: 0` | up |
| 200, message `08 02`, `grpc-status: 0` | down: `status: NOT_SERVING` |
| 200, message `08 03`, `grpc-status: 0` | down: `status: SERVICE_UNKNOWN` |
| 200, empty message (length 0), `grpc-status: 0` | down: `status: UNKNOWN` |
| 200, message `08 07`, `grpc-status: 0` | down: `status: 7` |
| 200, message `10 05 08 01` (an unknown field 2, then field 1), `grpc-status: 0` | up |
| 200, headers only, `grpc-status: 5`, `grpc-message: unknown%20service` | down: `grpc: 5 NOT_FOUND: unknown service` |
| 200, headers only, `grpc-status: 12` | down: `grpc: 12 UNIMPLEMENTED` |
| 200, headers only, `grpc-status: 99` | down: `grpc: 99 UNKNOWN_CODE` |
| 200, message `08 01`, no trailers | down: `query: no grpc-status` |
| 200, compressed flag 1 | down: `query: compressed message` |
| 200, message length 70000 | down: `query: reply too large` |
| 200, no message, `grpc-status: 0` | down: `query: no message` |
| 200, message `08`, `grpc-status: 0` | down: `query: malformed message` |
| 404, `content-type: application/grpc` | down: `query: HTTP status 404` |
| 200, `content-type: text/html` | down: `query: not a gRPC response (content-type text/html)` |
| an HTTP/1.1 server on the port | down: reason starts with `query: HTTP/2: ` |
| `tls = true`, server selects ALPN `http/1.1` | down: `tls: server did not select h2 (ALPN)` |

<!-- @trace
source: emfor-check-grpc
updated: 2026-11-14
code:
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/amqp.rs
  - crates/emfor-check/src/mysql.rs
  - crates/emfor-check/src/lib.rs
  - docs/manual/04-health-checks.md
  - crates/emfor-check/src/smtp.rs
  - examples/emfor.toml
  - crates/emfor-check/src/mongodb.rs
  - crates/emfor-check/src/redis.rs
  - crates/emfor-check/src/pgsql.rs
  - crates/emfor-daemon/src/checker/http.rs
  - CHANGELOG.md
  - examples/freebsd/Makefile.crates
  - crates/emfor-check/src/radius.rs
  - docs/deploy/plugins.md
  - crates/emfor-check/src/grpc.rs
  - crates/emfor-check/src/replica.rs
  - tests/plugins/run.sh
  - tests/plugins/grpc_server.py
  - tests/plugins/inside.sh
  - crates/emfor-check/src/options.rs
  - tests/plugins/Dockerfile
  - crates/emfor-check/src/main.rs
  - docs/manual.zh_TW/04-health-checks.md
tests:
  - crates/emfor-check/tests/cli.rs
  - crates/emfor-check/tests/live.rs
-->

---
### Requirement: QUIC check options

The `quic` plugin SHALL accept these `check_options` keys and SHALL report every other key, including `tls`, as `unknown key`:

- `alpn`: a non-empty array of strings of 1 to 255 bytes each, default `["h3"]`. An empty array or a value that is not an array is `must be a non-empty array`; an element that is not a string of 1 to 255 bytes is `must be strings of 1 to 255 bytes`.
- `tls_verify`, `server_name` and `ca_file`: the rules they have for the `grpc` plugin, except that they never require `tls = true`, because QUIC is always encrypted.
- `http3`: a boolean, default `false`. `true` while `alpn` does not contain `h3` is `requires "h3" in alpn`.
- `path`: a string starting with `/`, default `/`; otherwise `must start with /`. It SHALL be a URL path with an optional query that parses unchanged; a path with a space, a control character or `#` is `must be a URL path without spaces, control characters or #`.
- `host_header`: a string of 1 to 256 bytes that is a host with an optional port, as a URI authority without userinfo (RFC 9114 section 4.3.1); otherwise `must be a host with an optional port, 1 to 256 bytes`.
- `expect_status`: a non-empty array whose entries are status codes (integers or strings) or inclusive `"lo-hi"` ranges with `lo` not greater than `hi`, all from 100 to 599, default `["200-399"]`. An empty array or a value that is not an array is `must be a non-empty array`; an invalid entry is `must be a status code from 100 to 599 or a range lo-hi`.
- `expect_body`: a string of 1 to 16384 bytes; otherwise `must be 1 to 16384 bytes`.

`path`, `host_header`, `expect_status` and `expect_body` SHALL be reported as `requires http3 = true` when `http3` is not `true`. Type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Defaults

- **WHEN** a service with `check = "quic"` sets no `check_options`
- **THEN** the plugin reports no errors and each check is a QUIC handshake offering ALPN `h3` with certificate verification on

#### Scenario: HTTP option without http3

- **WHEN** a service with `check = "quic"` sets `check_options = { path = "/healthz" }`
- **THEN** validation reports `path`: requires http3 = true

##### Example: QUIC option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | none |
| `{ alpn = ["h3", "hq-interop"] }` | none |
| `{ alpn = [] }` | `alpn`: must be a non-empty array |
| `{ alpn = [""] }` | `alpn`: must be strings of 1 to 255 bytes |
| `{ alpn = "h3" }` | `alpn`: must be a non-empty array |
| `{ http3 = true, path = "/healthz", expect_status = [200, "204-299"], expect_body = "ok" }` | none |
| `{ http3 = true, alpn = ["doq"] }` | `http3`: requires "h3" in alpn |
| `{ http3 = true, path = "healthz" }` | `path`: must start with / |
| `{ http3 = true, expect_status = ["299-200"] }` | `expect_status`: must be a status code from 100 to 599 or a range lo-hi |
| `{ http3 = true, expect_status = [] }` | `expect_status`: must be a non-empty array |
| `{ http3 = true, host_header = "a\nb" }` | `host_header`: must be a host with an optional port, 1 to 256 bytes |
| `{ http3 = true, host_header = "api.internal/x" }` | `host_header`: must be a host with an optional port, 1 to 256 bytes |
| `{ http3 = true, host_header = "api.internal:8443" }` | none |
| `{ http3 = true, path = "/a b" }` | `path`: must be a URL path without spaces, control characters or # |
| `{ expect_body = "ok" }` | `expect_body`: requires http3 = true |
| `{ server_name = "web.internal", tls_verify = false }` | none |
| `{ tls = true }` | `tls`: unknown key |
| `{ http3 = "yes" }` | `http3`: must be a boolean |


<!-- @trace
source: emfor-quic-check
updated: 2026-11-14
code:
  - crates/emfor-check/src/lib.rs
  - docs/manual.zh_TW/04-health-checks.md
  - tests/plugins/inside.sh
  - CHANGELOG.md
  - crates/emfor-check/src/main.rs
  - examples/freebsd/Makefile.crates
  - crates/emfor-check/src/options.rs
  - fuzz/fuzz_targets/plugins.rs
  - tests/plugins/run.sh
  - docs/deploy/plugins.md
  - docs/manual/04-health-checks.md
  - examples/emfor.toml
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/quic.rs
  - tests/plugins/Dockerfile
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/cli.rs
-->

---
### Requirement: QUIC check

The `quic` plugin SHALL open a new QUIC version 1 connection per check to the backend's IP and UDP port, without 0-RTT, offering the ALPN values of `alpn` in order and verifying the certificate with TLS 1.3 against `server_name` (or the backend IP when it is not set) unless `tls_verify = false`. All checks of one plugin process SHALL share one local UDP socket per address family, opened when first needed. With `http3 = false`, a completed handshake whose negotiated ALPN is in `alpn` SHALL be `up`, and the plugin SHALL then close the connection with application error code 0. With `http3 = true`, the plugin SHALL then send one HTTP/3 `GET` request for `path` with `:authority` set to `host_header`, else to `server_name` (an IPv6 address in brackets), else to the backend address (`ip:port`, IPv6 in brackets), and `user-agent: emfor-check/<package version>`; it SHALL NOT follow redirects. When `expect_body` is set and the status matches, it SHALL read the body up to 65536 bytes, stopping as soon as `expect_body` is found as a case-sensitive byte substring; without `expect_body` it SHALL NOT read the body. It SHALL then close the connection with `H3_NO_ERROR` (0x100).

The result SHALL be decided in this order, and the first matching rule SHALL apply:

1. The check has not finished within `timeout_ms`: `down`, `timeout`.
2. The local UDP socket cannot be opened, or `ca_file` or the system CA certificates cannot be read: `error`, with a reason naming the failure.
3. Version negotiation fails: `down`, `quic: version negotiation failed`.
4. The TLS handshake fails: `down`, a reason starting `tls: `; a server that shares no ALPN value is `tls: no application protocol (ALPN)`.
5. The connection is closed by the peer, also while the HTTP/3 request or the body is in progress: `down`, `quic: closed by peer: <code>`, followed by `: ` and the peer's reason text when it is not empty; a stateless reset is `quic: stateless reset`; any other connection error that the plugin did not cause, such as an idle timeout, is `down` with a reason starting `quic: `.
6. The negotiated ALPN is not in `alpn`: `down`, `tls: server selected ALPN <value> not in alpn`.
7. `http3` is `false`: `up`.
8. Any other HTTP/3 connection or stream error: `down`, `query: HTTP/3: <error>`.
9. The status matches no entry of `expect_status`: `down`, `unexpected status <status>`.
10. A failure while reading the body: `down`, a reason starting `body: `; `expect_body` not found: `down`, `expect_body: got "<first 64 bytes of the body, escaped>"`.
11. Otherwise: `up`.

#### Scenario: Handshake only

- **WHEN** a `quic` check with default options reaches a server that completes the handshake with ALPN `h3` and a certificate valid for the backend IP
- **THEN** the check result is `up` and the plugin closes the connection with application error code 0

#### Scenario: Nothing answers on the UDP port

- **WHEN** a check request has `timeout_ms` 900 and nothing answers on the backend's UDP port
- **THEN** the plugin answers that request within about 900 ms with `result` `down` and reason `timeout`

#### Scenario: HTTP/3 health endpoint

- **WHEN** a `quic` check sets `http3 = true` and `path = "/healthz"` and the server answers status 200
- **THEN** the check result is `up`

##### Example: QUIC and HTTP/3 outcomes

| Options | Server behavior | Result |
| --- | --- | --- |
| default | handshake with ALPN `h3`, trusted certificate | up |
| `alpn = ["hq-interop"]` | supports only `h3` | down: `tls: no application protocol (ALPN)` |
| default | certificate from an untrusted CA | down: reason starts with `tls: ` |
| `tls_verify = false` | certificate from an untrusted CA | up |
| `server_name = "other.internal"` | certificate only for `web.internal` | down: reason starts with `tls: ` |
| default | refuses the connection (`CONNECTION_REFUSED`, no reason) | down: `quic: closed by peer: 2` |
| `http3 = true` | completes the handshake, then closes the connection with application code 0x10 and reason `busy` | down: `quic: closed by peer: 16: busy` |
| default | nothing listens on the UDP port | down: `timeout` |
| `http3 = true` | 200 | up |
| `http3 = true` | 302 to `/login` | up (redirect not followed) |
| `http3 = true` | 503 | down: `unexpected status 503` |
| `http3 = true, expect_body = "ok"` | 200, body `all ok` | up |
| `http3 = true, expect_body = "ok"` | 200, body `fail` | down: `expect_body: got "fail"` |
| `http3 = true, expect_body = "ok"` | 503, body `ok` | down: `unexpected status 503` |
| `http3 = true, host_header = "api.internal"` | records the request | `:authority` is `api.internal` |
| `http3 = true` | resets the request stream | down: reason starts with `query: HTTP/3: ` |
| `http3 = true, expect_body = "ok"` | 200, then resets the stream before the body | down: reason starts with `body: ` |
| `http3 = true, expect_body = "ok"` | 200 and body `partial`, then closes the connection with application code 0x10 and reason `busy` | down: `quic: closed by peer: 16: busy` |

<!-- @trace
source: emfor-quic-check
updated: 2026-11-14
code:
  - crates/emfor-check/src/lib.rs
  - docs/manual.zh_TW/04-health-checks.md
  - tests/plugins/inside.sh
  - CHANGELOG.md
  - crates/emfor-check/src/main.rs
  - examples/freebsd/Makefile.crates
  - crates/emfor-check/src/options.rs
  - fuzz/fuzz_targets/plugins.rs
  - tests/plugins/run.sh
  - docs/deploy/plugins.md
  - docs/manual/04-health-checks.md
  - examples/emfor.toml
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/quic.rs
  - tests/plugins/Dockerfile
tests:
  - crates/emfor-check/tests/live.rs
  - crates/emfor-check/tests/cli.rs
-->