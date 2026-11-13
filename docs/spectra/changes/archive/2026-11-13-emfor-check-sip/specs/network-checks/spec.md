## ADDED Requirements

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
