# dns-server Specification

## Purpose

Defines the DNS server inside the emfor daemon. It answers A and AAAA queries for services under one zone with a single backend chosen from live health state, and SRV queries with every candidate backend and its port, so that programs without an emfor client can find a healthy backend through the host's resolver. It answers only its own zone and never forwards other names.

## Requirements

### Requirement: DNS server enabled by the dns table

The daemon SHALL run the DNS server only when the config has a `[dns]` table. It SHALL listen on UDP and TCP on every address in `[dns] listen` (default `["127.0.0.1:5300"]`). If any address cannot be bound at startup, the daemon SHALL exit with status 1 and an error naming that address.

#### Scenario: No dns table

- **WHEN** the config has no `[dns]` table
- **THEN** the daemon opens no DNS socket

#### Scenario: UDP and TCP

- **WHEN** the config has `[dns]` with default settings and a healthy IPv4 backend for `api.svc.emfor`
- **THEN** an A query for `api.svc.emfor` returns that backend both over UDP and over TCP on 127.0.0.1:5300

#### Scenario: Address in use

- **WHEN** another process already listens on UDP 127.0.0.1:5300 and the daemon starts with default `[dns]` settings
- **THEN** the daemon exits with status 1 and its error message contains `127.0.0.1:5300`


<!-- @trace
source: emfor-dns
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: Answers for service names

For an A query on a service name the server SHALL use the IPv4 backends, and for AAAA the IPv6 backends, choosing as defined by the backend-selection capability. A chosen backend SHALL be returned as exactly one A or AAAA record with the authoritative-answer flag set and a TTL equal to the service's `dns_ttl` when set, otherwise `[dns] ttl` (default 1 second). When the service has no backend of the requested family, the server SHALL return NOERROR with no answer and the zone SOA in the authority section. When the result is "all down", the server SHALL return SERVFAIL. An SRV query on a service name SHALL be answered as defined by the requirement "SRV answers". Any query type other than A, AAAA and SRV on a service name SHALL return NOERROR with no answer and the zone SOA.

#### Scenario: Healthy backend answered

- **WHEN** service `api.svc.emfor` has healthy backend `10.0.0.11:8080` and a client queries `api.svc.emfor` type A
- **THEN** the reply is NOERROR with the AA flag and one A record `10.0.0.11` with TTL 1

##### Example: replies by case

| Service backends (state) | `on_all_down` | Query | Reply |
| --- | --- | --- | --- |
| 10.0.0.11 (healthy) | open | A | NOERROR, A 10.0.0.11 |
| 10.0.0.11 (healthy) | open | AAAA | NOERROR, no answer, SOA in authority |
| [2001:db8::11] (healthy) | open | AAAA | NOERROR, AAAA 2001:db8::11 |
| 10.0.0.11 (unhealthy) | open | A | NOERROR, A 10.0.0.11 |
| 10.0.0.11 (unhealthy) | closed | A | SERVFAIL |
| 10.0.0.11 (healthy) | open | MX | NOERROR, no answer, SOA in authority |
| 10.0.0.11 (healthy) | open | SRV | NOERROR, SRV `0 1 8080 10-0-0-11.api.svc.emfor.` |

#### Scenario: Per-service TTL

- **WHEN** `[dns] ttl = "1s"` and service `api.svc.emfor` sets `dns_ttl = "5s"`
- **THEN** A records for `api.svc.emfor` carry TTL 5

#### Scenario: Health change visible immediately

- **WHEN** the only healthy IPv4 backend of a `failover` service becomes unhealthy and another IPv4 backend is healthy
- **THEN** the next A query returns the other backend


<!-- @trace
source: emfor-dns-srv
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: Answers for names that are not services

The server SHALL return REFUSED for any name that is neither the zone nor under it. For the zone apex it SHALL return the zone SOA for an SOA query and NOERROR with no answer and the SOA for other types. A name of the form `_<label>._tcp.<service name>`, where `<label>` is one label that starts with `_` and has at least one more character, is an SRV owner name: an SRV query on it SHALL be answered as defined by the requirement "SRV answers", and any other type SHALL return NOERROR with no answer and the SOA. A name `_tcp.<service name>` SHALL return NOERROR with no answer and the SOA for every type. A target name as defined by the requirement "SRV target names" SHALL be answered as defined there. For a name under the zone that is a proper ancestor of a configured service name, the server SHALL return NOERROR with no answer and the SOA. For any other name under the zone it SHALL return NXDOMAIN with the AA flag and the SOA in the authority section. A configured service name SHALL be answered as a service even when it also has the form of an SRV owner name or a target name. Name matching SHALL be case-insensitive and SHALL ignore a trailing dot.

#### Scenario: Unknown service

- **WHEN** no service is named `nope.svc.emfor` and a client queries it type A
- **THEN** the reply is NXDOMAIN with the zone SOA in the authority section

##### Example: name classification with zone `svc.emfor` and service `a.b.svc.emfor`

| Query name | Type | Reply |
| --- | --- | --- |
| `example.com` | A | REFUSED |
| `svc.emfor` | SOA | NOERROR, SOA answer |
| `svc.emfor` | A | NOERROR, no answer, SOA |
| `b.svc.emfor` | A | NOERROR, no answer, SOA |
| `c.svc.emfor` | A | NXDOMAIN, SOA |
| `A.B.SVC.EMFOR.` | A | answered as `a.b.svc.emfor` |

#### Scenario: SRV-related names

- **WHEN** service `api.svc.emfor` has the single backend `10.0.0.11:8080` and a client queries the names below
- **THEN** each reply is as listed

##### Example: SRV owner, empty non-terminal and target names

| Query name | Type | Reply |
| --- | --- | --- |
| `_http._tcp.api.svc.emfor` | SRV | NOERROR, SRV `0 1 8080 10-0-0-11.api.svc.emfor.` |
| `_HTTP._TCP.API.SVC.EMFOR.` | SRV | answered as `_http._tcp.api.svc.emfor` |
| `_http._tcp.api.svc.emfor` | TXT | NOERROR, no answer, SOA |
| `_tcp.api.svc.emfor` | SRV | NOERROR, no answer, SOA |
| `_http._udp.api.svc.emfor` | SRV | NXDOMAIN, SOA |
| `_._tcp.api.svc.emfor` | SRV | NXDOMAIN, SOA |
| `x._http._tcp.api.svc.emfor` | SRV | NXDOMAIN, SOA |
| `10-0-0-11.api.svc.emfor` | A | NOERROR, A 10.0.0.11 |
| `10-0-0-99.api.svc.emfor` | A | NXDOMAIN, SOA |


<!-- @trace
source: emfor-dns-srv
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: Zone SOA record

The server SHALL synthesize the zone SOA as `<zone>. <ttl> IN SOA ns.<zone>. hostmaster.<zone>. <serial> 3600 600 86400 <ttl>`, where `<ttl>` is `[dns] ttl` in seconds and `<serial>` is the daemon start time in Unix seconds truncated to 32 bits.

#### Scenario: SOA values

- **WHEN** `[dns] zone = "svc.emfor"`, `ttl = "1s"`, and a client queries `svc.emfor` type SOA
- **THEN** the answer has MNAME `ns.svc.emfor.`, RNAME `hostmaster.svc.emfor.`, refresh 3600, retry 600, expire 86400, minimum 1, and record TTL 1


<!-- @trace
source: emfor-dns
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: Malformed and unsupported requests

The server SHALL return FORMERR for a query whose question count is not 1 and NOTIMP for any opcode other than QUERY.

#### Scenario: Unsupported opcode

- **WHEN** a client sends an UPDATE message for `svc.emfor`
- **THEN** the reply is NOTIMP


<!-- @trace
source: emfor-dns
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: Services outside the zone are reported

On startup and after every successful reload, when `[dns]` is present, the daemon SHALL log one `warn` line for each service whose name is not under the zone, containing the service name and the zone.

#### Scenario: Service outside zone

- **WHEN** `[dns] zone = "svc.emfor"` and a service is named `db.internal`
- **THEN** the log contains a `warn` line with `db.internal` and `svc.emfor`, and A queries for `db.internal` are REFUSED

<!-- @trace
source: emfor-dns
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: SRV answers

For an SRV query on a service name or on one of its SRV owner names, the server SHALL take as candidates the service's backends of both address families in config order, keeping the first of these sets that is not empty: the passing primary backends (`state` `healthy`, `reason` `null`, `backup` false), the passing backup backends (`backup` true), the healthy primary backends, failing ones (`state` `healthy` and `reason` not `null`) included, and the healthy backup backends, failing ones included. When no backend is healthy and `on_all_down` is `open`, the candidates SHALL be the backends whose `state` is `unknown` and `reason` is `null` when there is at least one, otherwise every `unknown` backend when there is at least one, otherwise every backend; when no backend is healthy and `on_all_down` is `closed`, the server SHALL return SERVFAIL. The server SHALL return NOERROR with the AA flag and one SRV record per candidate, whose owner is the query name as received, whose port is the backend's port, whose target is the backend's target name, and whose TTL equals the service's `dns_ttl` when set, otherwise `[dns] ttl`. Priority, weight and record order SHALL follow the service's `balance`:

- `failover`: priority equals the candidate's position among the candidates (0 for the first), weight 1, config order.
- `weighted`: priority 0, weight equals the backend's `weight`, config order.
- `locality`: priority 0 for candidates whose `site` equals the host's `site` and 1 for the others; priority 0 for every candidate when the host has no `site` or no candidate matches it. Weight 1. Order is the rotated candidate list, stably sorted by priority.
- `fastest`: priority 0 for candidates in the service's pool (as defined by the backend-selection requirement "Fast membership decided by the daemon") and 1 for the others; priority 0 for every candidate when the pool is empty. Weight 1. Order is the rotated candidate list, stably sorted by priority.
- `round-robin`, `sticky`, and any value the daemon does not recognize: priority 0, weight 1, order is the rotated candidate list.

The rotated candidate list SHALL start at position `n mod len` and wrap around, where `n` is a per-service SRV counter that starts at 0 when the service is first configured, increases by one on every SRV answer for that service, is kept across reloads for services that stay, and is independent of the A and AAAA choice state.

The additional section SHALL contain one A or AAAA record per distinct target name, in answer order, with the target's address and the same TTL as the SRV records, subject to the requirement "SRV responses fit the transport".

#### Scenario: Failover priorities

- **WHEN** a `failover` service `api.svc.emfor` has backends `10.0.0.11:8080` (unhealthy), `10.0.0.12:8080` (healthy), `10.0.0.13:8080` (healthy) in config order, and a client queries SRV
- **THEN** the answer is `0 1 8080 10-0-0-12.api.svc.emfor.` followed by `1 1 8080 10-0-0-13.api.svc.emfor.`

#### Scenario: Weighted weights

- **WHEN** a `weighted` service `api.svc.emfor` has healthy backends `10.0.0.11:8080` weight 1, `10.0.0.12:8080` weight 3, `10.0.0.13:8080` weight 1
- **THEN** every SRV answer is `0 1 8080 10-0-0-11.api.svc.emfor.`, `0 3 8080 10-0-0-12.api.svc.emfor.`, `0 1 8080 10-0-0-13.api.svc.emfor.` in that order

#### Scenario: Round-robin rotation

- **WHEN** a `round-robin` service has healthy backends A, B, C in config order and receives three SRV queries in a row, starting from a fresh SRV counter
- **THEN** the answers are ordered A,B,C then B,C,A then C,A,B, all with priority 0 and weight 1

#### Scenario: Locality priorities

- **WHEN** the host `site` is `dc1` and a `locality` service has healthy backends `10.0.0.11:8080` (site dc1), `10.0.1.11:8080` (site dc2), `10.0.0.12:8080` (site dc1)
- **THEN** each SRV answer lists the two dc1 backends first with priority 0 and the dc2 backend last with priority 1

##### Example: locality priorities by host site

| Host site | Backend sites in config order | Priorities in config order |
| --- | --- | --- |
| dc1 | dc1, dc2, dc1 | 0, 1, 0 |
| dc3 | dc1, dc2 | 0, 0 |
| none | dc1, dc2 | 0, 0 |

#### Scenario: Fastest priorities

- **WHEN** a `fastest` service has healthy backends `10.0.0.11:8080`, `10.0.0.12:8080` and `[2001:db8::11]:8080` in config order, and its pool is `10.0.0.11:8080` and `[2001:db8::11]:8080`
- **THEN** each SRV answer lists `10-0-0-11.api.svc.emfor.` and `2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor.` with priority 0 first and `10-0-0-12.api.svc.emfor.` with priority 1 last

#### Scenario: Both families listed

- **WHEN** a `round-robin` service `api.svc.emfor` has healthy backends `10.0.0.11:8080` and `[2001:db8::11]:8080`
- **THEN** the SRV answer has two records, with targets `10-0-0-11.api.svc.emfor.` and `2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor.`, and the additional section has A `10.0.0.11` and AAAA `2001:db8::11`

#### Scenario: Health across families

- **WHEN** a service has `10.0.0.11:8080` (unhealthy) and `[2001:db8::11]:8080` (healthy) with `on_all_down = "closed"`
- **THEN** the SRV answer lists only the IPv6 backend

#### Scenario: Failing backend left out

- **WHEN** a `round-robin` service `api.svc.emfor` has backends `10.0.0.11:8080` (healthy, reason `connect: Connection refused`) and `[2001:db8::11]:8080` (healthy, reason `null`), and a client queries SRV
- **THEN** the SRV answer has one record, with target `2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor.`

#### Scenario: Every healthy backend failing in SRV

- **WHEN** a `failover` service `api.svc.emfor` has backends `10.0.0.11:8080` (unhealthy), `10.0.0.12:8080` (healthy, reason `timeout`), `10.0.0.13:8080` (healthy, reason `timeout`) in config order, and a client queries SRV
- **THEN** the answer is `0 1 8080 10-0-0-12.api.svc.emfor.` followed by `1 1 8080 10-0-0-13.api.svc.emfor.`

#### Scenario: Backups listed only while no primary passes

- **WHEN** a `failover` service `api.svc.emfor` has backends `10.0.0.11:8080` (healthy, reason `timeout`), backup `10.0.1.11:8080` (healthy, reason `null`) and backup `10.0.1.12:8080` (healthy, reason `null`) in config order, and a client queries SRV
- **THEN** the answer is `0 1 8080 10-0-1-11.api.svc.emfor.` followed by `1 1 8080 10-0-1-12.api.svc.emfor.`, and once `10.0.0.11:8080` passes again the answer is only `0 1 8080 10-0-0-11.api.svc.emfor.`

#### Scenario: All down closed

- **WHEN** every backend of a service is unhealthy and `on_all_down = "closed"`
- **THEN** an SRV query returns SERVFAIL

#### Scenario: All down open

- **WHEN** every backend of a `round-robin` service is unhealthy and `on_all_down = "open"`
- **THEN** an SRV query lists every backend

#### Scenario: All down open with an unknown backend

- **WHEN** a `round-robin` service with `on_all_down = "open"` has backends `10.0.0.11:8080` (unhealthy) and `[2001:db8::11]:8080` (unknown, reason `null`), and a client queries SRV
- **THEN** the SRV answer has one record, with target `2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor.`

#### Scenario: Shared IP

- **WHEN** service `api.svc.emfor` has backends `10.0.0.11:8080` and `10.0.0.11:8081`, both healthy
- **THEN** the SRV answer has two records with target `10-0-0-11.api.svc.emfor.` and ports 8080 and 8081, and the additional section has a single A record `10.0.0.11`

#### Scenario: SRV TTL

- **WHEN** `[dns] ttl = "1s"` and service `api.svc.emfor` sets `dns_ttl = "5s"`
- **THEN** SRV records and their additional A/AAAA records carry TTL 5


<!-- @trace
source: emfor-backup-backends
updated: 2026-11-14
code:
  - docs/manual.zh_TW/05-backend-selection.md
  - clients/go/emfor.go
  - crates/emfor-daemon/src/status.rs
  - tests/conformance/v1/README.md
  - clients/python/emfor/__init__.py
  - crates/emfor-daemon/src/writer.rs
  - docs/manual.zh_TW/07-dns.md
  - examples/emfor.toml
  - tests/conformance/v1/cases.json
  - README.md
  - clients/rust/src/lib.rs
  - clients/perl/README.md
  - clients/rust/src/client.rs
  - crates/emfor-format/src/lib.rs
  - docs/manual.zh_TW/03-configuration.md
  - crates/emfor-daemon/src/state.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/select.rs
  - crates/emfor-daemon/src/config.rs
  - clients/python/README.md
  - clients/rust/README.md
  - docs/manual/06-clients.md
  - docs/manual/07-dns.md
  - CHANGELOG.md
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/dns.rs
  - docs/manual.zh_TW/06-clients.md
  - docs/manual/05-backend-selection.md
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/metrics.rs
  - crates/emfor-format/src/rules.rs
  - clients/go/README.md
  - clients/php/README.md
tests:
  - crates/emfor-daemon/tests/daemon.rs
  - clients/rust/tests/conformance_capi.rs
-->

---
### Requirement: SRV target names

The target name of a backend SHALL be `<ip label>.<service name>.`, where the IP label is, for an IPv4 address, its dotted decimal form with every `.` replaced by `-`, and for an IPv6 address, its eight 16-bit groups each written as four lowercase hexadecimal digits and joined by `-`. For an A query on a name whose first label equals the IP label of an IPv4 backend of the service named by the rest of the name, the server SHALL return NOERROR with the AA flag and one A record with that address; for AAAA and an IPv6 backend, one AAAA record. The answer SHALL NOT depend on the backend's health state. Its TTL SHALL be the service's `dns_ttl` when set, otherwise `[dns] ttl`. Any other query type on a target name SHALL return NOERROR with no answer and the zone SOA. A first label that equals no backend's IP label SHALL NOT make a target name; such a name is classified by the requirement "Answers for names that are not services".

#### Scenario: Target of a down backend

- **WHEN** backend `10.0.0.11:8080` of `api.svc.emfor` is unhealthy and `on_all_down = "closed"`
- **THEN** an A query for `10-0-0-11.api.svc.emfor` returns NOERROR with A `10.0.0.11`

##### Example: target labels and queries for service `api.svc.emfor`

| Backend | Query name | Type | Reply |
| --- | --- | --- | --- |
| `10.0.0.11:8080` | `10-0-0-11.api.svc.emfor` | A | NOERROR, A 10.0.0.11 |
| `10.0.0.11:8080` | `10-0-0-11.api.svc.emfor` | AAAA | NOERROR, no answer, SOA |
| `10.0.0.11:8080` | `010-0-0-11.api.svc.emfor` | A | NXDOMAIN, SOA |
| `[2001:db8::11]:8080` | `2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor` | AAAA | NOERROR, AAAA 2001:db8::11 |
| `[2001:db8::11]:8080` | `2001-db8--11.api.svc.emfor` | AAAA | NXDOMAIN, SOA |
| `[::1]:8080` | `0000-0000-0000-0000-0000-0000-0000-0001.api.svc.emfor` | AAAA | NOERROR, AAAA ::1 |


<!-- @trace
source: emfor-dns-srv
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: SRV responses fit the transport

Over UDP the server SHALL keep every response within a size limit equal to the request's EDNS UDP payload size clamped to the range 512 to 1232 bytes, or 512 bytes when the request has no EDNS; over TCP the limit SHALL be 65535 bytes. When the request has EDNS, the response's EDNS UDP payload size SHALL equal that clamped limit. When an SRV response including its additional records exceeds the limit, the server SHALL omit every additional A and AAAA record and SHALL NOT set the TC flag for that reason. When the SRV records alone exceed the limit, the server SHALL send as many SRV records as fit, without additional records, and SHALL set the TC flag.

#### Scenario: Glue dropped without truncation

- **WHEN** an SRV response for a service fits in 512 bytes without additional records but not with them, and the request has no EDNS
- **THEN** the UDP reply has every SRV record, no additional A or AAAA records, and the TC flag clear

#### Scenario: Truncated over UDP, complete over TCP

- **WHEN** a service has more backends than SRV records fit in 512 bytes and a client without EDNS queries SRV over UDP, then over TCP
- **THEN** the UDP reply has the TC flag set, and the TCP reply has every SRV record and every additional record

##### Example: size limit by request

| Transport | Request EDNS payload size | Limit (bytes) |
| --- | --- | --- |
| UDP | none | 512 |
| UDP | 400 | 512 |
| UDP | 1232 | 1232 |
| UDP | 4096 | 1232 |
| TCP | any | 65535 |

<!-- @trace
source: emfor-dns-srv
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: Services with backends on different ports are reported

On startup and after every successful reload, when `[dns]` is present, the daemon SHALL log one `warn` line for each service whose name is under the zone and whose backends do not all have the same port. The line SHALL contain the service name, the service's distinct backend ports in ascending order, and the words `SRV` and `port`, telling the operator that A and AAAA answers carry no port. Backends of both address families SHALL be counted together. A service whose name is not under the zone SHALL NOT get this warning. The warning SHALL NOT change DNS answers, the validity of the config, or the output and exit status of `emfor check-config`.

#### Scenario: Mixed ports in the zone

- **WHEN** `[dns] zone = "svc.emfor"` and service `api.svc.emfor` has backends `10.0.0.11:8080` and `10.0.0.12:8081`
- **THEN** after startup the log contains one `warn` line with `api.svc.emfor`, `8080, 8081`, `SRV` and `port`, and A queries for `api.svc.emfor` are answered as before

##### Example: which services are reported

| `[dns]` | Service name | Backends | Warning |
| --- | --- | --- | --- |
| zone `svc.emfor` | `api.svc.emfor` | `10.0.0.11:8080`, `10.0.0.12:8081` | yes, ports `8080, 8081` |
| zone `svc.emfor` | `api.svc.emfor` | `10.0.0.12:9000`, `10.0.0.11:8080`, `10.0.0.13:8080` | yes, ports `8080, 9000` |
| zone `svc.emfor` | `api.svc.emfor` | `10.0.0.11:8080`, `[2001:db8::11]:8080` | no |
| zone `svc.emfor` | `api.svc.emfor` | `10.0.0.11:8080`, `[2001:db8::11]:8443` | yes, ports `8080, 8443` |
| zone `svc.emfor` | `db.internal` | `10.0.0.21:5432`, `10.0.0.22:5433` | no (outside-zone warning only) |
| absent | `api.svc.emfor` | `10.0.0.11:8080`, `10.0.0.12:8081` | no |

#### Scenario: Reload reports the new config

- **WHEN** the daemon runs with service `api.svc.emfor` whose backends all use one port (for example 8080), and a reload adds a backend on another port (for example `10.0.0.13:9090`)
- **THEN** there is no port warning before the reload, and after it the log contains one `warn` line with `api.svc.emfor` and both ports in ascending order (`8080, 9090` in the example)

<!-- @trace
source: emfor-server-subcommand
updated: 2026-11-13
code:
  - README.md
  - docs/deploy/plugins.md
  - examples/systemd/emfor.service
  - tests/spike/run.sh
  - docs/plugins/protocol-v1.md
  - examples/freebsd/Makefile
  - examples/freebsd/README.md
  - examples/rc.d/emfor
  - CHANGELOG.md
  - tests/plugins/inside.sh
  - README.zh-TW.md
  - docs/deploy/daemon.md
  - tests/resolver/resolved.sh
  - examples/emfor.toml
  - tests/resolver/inside.sh
  - crates/emfor-daemon/src/cli.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - crates/emfor-daemon/tests/metrics.rs
  - crates/emfor-daemon/tests/daemon.rs
  - crates/emfor-daemon/tests/plugins.rs
-->

---
### Requirement: SRV weights during slow start

When at least one SRV candidate is ramping, as defined by the backend-selection requirement "Slow start effective weight", each SRV record's weight SHALL be that candidate's effective weight, for every `balance` value except `failover`; priorities and record order SHALL follow the requirement "SRV answers" unchanged. When no candidate is ramping, or for `failover`, weights SHALL be as in "SRV answers".

#### Scenario: Round-robin with a ramping backend

- **WHEN** a `round-robin` service `api.svc.emfor` has healthy backends `10.0.0.11:8080` (not ramping) and `10.0.0.12:8080` (step 3), and a client queries SRV with a fresh SRV counter
- **THEN** the answer is `0 10 8080 10-0-0-11.api.svc.emfor.` followed by `0 3 8080 10-0-0-12.api.svc.emfor.`

#### Scenario: Weighted with a ramping backend

- **WHEN** a `weighted` service has healthy backends `10.0.0.11:8080` (weight 3, step 2) and `10.0.0.12:8080` (weight 1, not ramping)
- **THEN** the SRV weights are 6 and 10 in config order

#### Scenario: Failover keeps weight 1

- **WHEN** a `failover` service has healthy backends `10.0.0.11:8080` (step 2) and `10.0.0.12:8080` (not ramping)
- **THEN** the answer is `0 1 8080 10-0-0-11.api.svc.emfor.` followed by `1 1 8080 10-0-0-12.api.svc.emfor.`

<!-- @trace
source: emfor-slow-start
updated: 2026-11-14
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/state.rs
  - tools/emfor-fixture/src/lib.rs
  - docs/manual.zh_TW/07-dns.md
  - tests/conformance/v1/README.md
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-format/src/rules.rs
  - tests/conformance/v1/cases.json
  - crates/emfor-format/src/lib.rs
  - docs/manual/07-dns.md
  - crates/emfor-daemon/src/select.rs
  - crates/emfor-daemon/src/writer.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/supervisor.rs
  - docs/manual.zh_TW/05-backend-selection.md
  - crates/emfor-daemon/src/metrics.rs
  - CHANGELOG.md
  - docs/manual/05-backend-selection.md
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/dns.rs
  - docs/manual.zh_TW/03-configuration.md
  - clients/rust/src/client.rs
-->