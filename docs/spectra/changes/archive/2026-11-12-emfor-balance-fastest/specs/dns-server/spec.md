## MODIFIED Requirements

### Requirement: SRV answers

For an SRV query on a service name or on one of its SRV owner names, the server SHALL take as candidates the service's backends of both address families in config order, keeping only healthy backends when at least one backend is healthy. When no backend is healthy, the candidates SHALL be every backend if `on_all_down` is `open`, and the server SHALL return SERVFAIL if it is `closed`. The server SHALL return NOERROR with the AA flag and one SRV record per candidate, whose owner is the query name as received, whose port is the backend's port, whose target is the backend's target name, and whose TTL equals the service's `dns_ttl` when set, otherwise `[dns] ttl`. Priority, weight and record order SHALL follow the service's `balance`:

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

#### Scenario: All down closed

- **WHEN** every backend of a service is unhealthy and `on_all_down = "closed"`
- **THEN** an SRV query returns SERVFAIL

#### Scenario: All down open

- **WHEN** every backend of a `round-robin` service is unhealthy and `on_all_down = "open"`
- **THEN** an SRV query lists every backend

#### Scenario: Shared IP

- **WHEN** service `api.svc.emfor` has backends `10.0.0.11:8080` and `10.0.0.11:8081`, both healthy
- **THEN** the SRV answer has two records with target `10-0-0-11.api.svc.emfor.` and ports 8080 and 8081, and the additional section has a single A record `10.0.0.11`

#### Scenario: SRV TTL

- **WHEN** `[dns] ttl = "1s"` and service `api.svc.emfor` sets `dns_ttl = "5s"`
- **THEN** SRV records and their additional A/AAAA records carry TTL 5
