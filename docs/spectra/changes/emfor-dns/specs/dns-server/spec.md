## Purpose

Defines the DNS server inside the emfor daemon. It answers A and AAAA queries for services under one zone with a single backend chosen from live health state, so that programs without an emfor client can find a healthy backend through the host's resolver. It answers only its own zone and never forwards other names.

## ADDED Requirements

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

### Requirement: Answers for service names

For an A query on a service name the server SHALL use the IPv4 backends, and for AAAA the IPv6 backends, choosing as defined by the backend-selection capability. A chosen backend SHALL be returned as exactly one A or AAAA record with the authoritative-answer flag set and a TTL equal to the service's `dns_ttl` when set, otherwise `[dns] ttl` (default 1 second). When the service has no backend of the requested family, the server SHALL return NOERROR with no answer and the zone SOA in the authority section. When the result is "all down", the server SHALL return SERVFAIL. Any query type other than A and AAAA on a service name SHALL return NOERROR with no answer and the zone SOA.

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

#### Scenario: Per-service TTL

- **WHEN** `[dns] ttl = "1s"` and service `api.svc.emfor` sets `dns_ttl = "5s"`
- **THEN** A records for `api.svc.emfor` carry TTL 5

#### Scenario: Health change visible immediately

- **WHEN** the only healthy IPv4 backend of a `failover` service becomes unhealthy and another IPv4 backend is healthy
- **THEN** the next A query returns the other backend

### Requirement: Answers for names that are not services

The server SHALL return REFUSED for any name that is neither the zone nor under it. For the zone apex it SHALL return the zone SOA for an SOA query and NOERROR with no answer and the SOA for other types. For a name under the zone that is a proper ancestor of a configured service name, it SHALL return NOERROR with no answer and the SOA. For any other name under the zone it SHALL return NXDOMAIN with the AA flag and the SOA in the authority section. Name matching SHALL be case-insensitive and SHALL ignore a trailing dot.

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

### Requirement: Zone SOA record

The server SHALL synthesize the zone SOA as `<zone>. <ttl> IN SOA ns.<zone>. hostmaster.<zone>. <serial> 3600 600 86400 <ttl>`, where `<ttl>` is `[dns] ttl` in seconds and `<serial>` is the daemon start time in Unix seconds truncated to 32 bits.

#### Scenario: SOA values

- **WHEN** `[dns] zone = "svc.emfor"`, `ttl = "1s"`, and a client queries `svc.emfor` type SOA
- **THEN** the answer has MNAME `ns.svc.emfor.`, RNAME `hostmaster.svc.emfor.`, refresh 3600, retry 600, expire 86400, minimum 1, and record TTL 1

### Requirement: Malformed and unsupported requests

The server SHALL return FORMERR for a query whose question count is not 1 and NOTIMP for any opcode other than QUERY.

#### Scenario: Unsupported opcode

- **WHEN** a client sends an UPDATE message for `svc.emfor`
- **THEN** the reply is NOTIMP

### Requirement: Services outside the zone are reported

On startup and after every successful reload, when `[dns]` is present, the daemon SHALL log one `warn` line for each service whose name is not under the zone, containing the service name and the zone.

#### Scenario: Service outside zone

- **WHEN** `[dns] zone = "svc.emfor"` and a service is named `db.internal`
- **THEN** the log contains a `warn` line with `db.internal` and `svc.emfor`, and A queries for `db.internal` are REFUSED
