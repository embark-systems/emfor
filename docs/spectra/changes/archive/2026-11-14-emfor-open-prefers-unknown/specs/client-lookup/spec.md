## MODIFIED Requirements

### Requirement: Candidates follow health and on_all_down

When the store is fresh, the candidates SHALL be the passing backends, in config order: a backend is passing when its `state` is `healthy` and its `reason` is `null`, and failing when its `state` is `healthy` and its `reason` is not `null`. When the store is fresh and no backend is passing, the candidates SHALL be every `healthy` backend, failing ones included, in config order. When the store is stale, `reason` SHALL NOT affect the candidates. When no backend is a candidate (none healthy, or the store is stale), a service with `on_all_down = "closed"` SHALL fail with `ALL_DOWN`. A service with `on_all_down = "open"` SHALL then use, in config order: when the store is fresh and at least one backend is unknown and passing (`state` `unknown` and `reason` `null`), only those backends; otherwise, when the store is fresh and at least one backend's `state` is `unknown`, every `unknown` backend; otherwise every backend. When the store is stale, every backend SHALL be a candidate of an open service. The lookup SHALL NOT tell the caller that the result came from an all-down service or from a service whose healthy backends are all failing.

#### Scenario: Unhealthy backends are skipped

- **WHEN** a fresh failover service lists `10.0.1.5:5432` (`unhealthy`) then `10.0.1.6:5432` (`healthy`)
- **THEN** the lookup returns `10.0.1.6:5432`

#### Scenario: Open service with every backend down

- **WHEN** a fresh failover service with `on_all_down = "open"` lists `10.0.1.5:5432` (`unhealthy`) then `10.0.1.6:5432` (`unknown`)
- **THEN** the lookup returns `10.0.1.6:5432`

#### Scenario: Open service prefers a passing unknown backend

- **WHEN** a fresh round-robin service with `on_all_down = "open"` lists `10.0.1.5:5432` (`unhealthy`, reason `refused`), `10.0.1.6:5432` (`unknown`, reason `timeout`) and `10.0.1.7:5432` (`unknown`, reason `null`)
- **THEN** consecutive lookups return only `10.0.1.7:5432`

#### Scenario: Open service with every unknown backend failing

- **WHEN** a fresh round-robin service with `on_all_down = "open"` lists `10.0.1.5:5432` (`unhealthy`, reason `refused`), `10.0.1.6:5432` (`unknown`, reason `timeout`) and `10.0.1.7:5432` (`unknown`, reason `refused`)
- **THEN** consecutive lookups return `10.0.1.6:5432` and `10.0.1.7:5432` and never `10.0.1.5:5432`

#### Scenario: Open service with only unhealthy backends

- **WHEN** a fresh round-robin service with `on_all_down = "open"` lists `10.0.1.5:5432` and `10.0.1.6:5432`, both `unhealthy`
- **THEN** consecutive lookups return both backends

#### Scenario: Failing backend is skipped while another passes

- **WHEN** a fresh failover service lists `10.0.1.5:5432` (`healthy`, reason `connect: Connection refused`) then `10.0.1.6:5432` (`healthy`, reason `null`)
- **THEN** the lookup returns `10.0.1.6:5432`

#### Scenario: Every healthy backend failing

- **WHEN** a fresh round-robin service with `on_all_down = "closed"` lists `10.0.1.5:5432` (`healthy`, reason `timeout`), `10.0.1.6:5432` (`healthy`, reason `timeout`) and `10.0.1.7:5432` (`unhealthy`)
- **THEN** consecutive lookups return `10.0.1.5:5432` and `10.0.1.6:5432` and never `10.0.1.7:5432`

#### Scenario: Stale store ignores reason

- **WHEN** a stale failover service with `on_all_down = "open"` lists `10.0.1.5:5432` (`healthy`, reason `timeout`) then `10.0.1.6:5432` (`healthy`, reason `null`)
- **THEN** the lookup returns `10.0.1.5:5432`

##### Example: candidates by state, reason and heartbeat

| Backends in config order (state, reason) | Heartbeat | `on_all_down` | Candidates |
| --- | --- | --- | --- |
| A (healthy, null), B (healthy, "refused"), C (unhealthy, "refused") | fresh | closed | A |
| A (healthy, "refused"), B (healthy, "timeout"), C (unhealthy, "refused") | fresh | closed | A, B |
| A (unhealthy, "refused"), B (unknown, null) | fresh | closed | none: `ALL_DOWN` |
| A (unhealthy, "refused"), B (unknown, null) | fresh | open | B |
| A (unhealthy, "refused"), B (unknown, "timeout"), C (unknown, null) | fresh | open | C |
| A (unhealthy, "refused"), B (unknown, "timeout"), C (unknown, "refused") | fresh | open | B, C |
| A (unhealthy, "refused"), B (unhealthy, "timeout") | fresh | open | A, B |
| A (unhealthy, "refused"), B (unknown, "timeout"), C (unknown, null) | fresh | closed | none: `ALL_DOWN` |
| A (unhealthy, "refused"), B (unknown, null) | stale | open | A, B |
| A (healthy, "refused"), B (healthy, null) | stale | open | A, B |
| A (healthy, "refused"), B (healthy, null) | stale | closed | none: `ALL_DOWN` |
