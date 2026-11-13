## Purpose

Defines the network protocol plugins of `emfor-check` (DNS, and later RADIUS, SIP and Diameter) that check backends speaking protocols other than databases, often over UDP, as found in closed data centres such as telecom sites. It covers how they exchange datagrams, how their check options are validated, and how their outcomes map to `up`, `down` and `error`.

## ADDED Requirements

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

### Requirement: Network check outcome classification

A network plugin SHALL answer `error` only for failures on the plugin's side: a file named in its `check_options` that cannot be read at check time, or a local resource limit (too many open files, out of memory, no socket of the address family available). Every failure caused by the backend or the network SHALL be `down`. A reason SHALL never contain a secret or password read from a file.

#### Scenario: Local socket failure

- **WHEN** the plugin cannot create a socket because the process has no free file descriptors
- **THEN** the check result is `error`, which does not count against the backend

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
