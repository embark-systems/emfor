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