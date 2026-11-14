## MODIFIED Requirements

### Requirement: Lookup failures use four shared error codes

A lookup that cannot return an address SHALL fail with exactly one of these codes, decided by the first step that fails, in this order:

1. `UNAVAILABLE` — the store file cannot be opened (missing, permission denied, LMDB lock or data format different from the client's, any other open error), the key `meta/schema_version` is absent, or `EMFOR_MAX_STALENESS` holds an invalid value.
2. `INCOMPATIBLE` — `meta/schema_version` is neither `1` nor `2`, `meta/heartbeat` is missing or not a decimal integer, or the service record cannot be parsed as a version 2 record. A `balance` string the client does not know SHALL NOT count as a parse failure (see "Unknown balance falls back to round-robin").
3. `NOT_FOUND` — no `svc/<name>` key exists for the normalized name.
4. `ALL_DOWN` — no backend is usable and the service's `on_all_down` is `closed`.

Each failure SHALL carry a human-readable message naming the cause. Each language SHALL expose the code in its own idiom (a status value in the C library, an error value in Rust) with the code's name available as the exact strings above. A malformed store SHALL NOT make the lookup crash, panic or abort.

#### Scenario: Error code by store condition

- **WHEN** a lookup runs against each store condition below
- **THEN** it fails with the listed code

##### Example: store conditions

| Store condition | Code |
| --- | --- |
| store file does not exist | `UNAVAILABLE` |
| store exists, `meta/schema_version` absent | `UNAVAILABLE` |
| `EMFOR_MAX_STALENESS=fast` | `UNAVAILABLE` |
| `meta/schema_version` = `3` | `INCOMPATIBLE` |
| `meta/heartbeat` absent | `INCOMPATIBLE` |
| `svc/api.svc.emfor` = `{not json` | `INCOMPATIBLE` |
| `svc/api.svc.emfor` has `"balance":5` (not a string) | `INCOMPATIBLE` |
| `svc/api.svc.emfor` has `"balance":"some-future-method"` | no error: chosen as `round-robin` |
| no `svc/nope.svc.emfor` key, lookup `nope.svc.emfor` | `NOT_FOUND` |
| every backend `unhealthy`, `on_all_down` = `closed` | `ALL_DOWN` |

#### Scenario: Schema check wins over a missing service

- **WHEN** `meta/schema_version` is `3` and the looked-up name has no record
- **THEN** the lookup fails with `INCOMPATIBLE`, not `NOT_FOUND`

#### Scenario: Missing store is retried

- **WHEN** a lookup fails with `UNAVAILABLE` because the store file does not exist, and the daemon then creates the store
- **THEN** the next lookup in the same process opens the store and returns an address

### Requirement: Candidates follow health and on_all_down

A backend is passing when its `state` is `healthy` and its `reason` is `null`, failing when its `state` is `healthy` and its `reason` is not `null`, a backup when its `backup` is true, and primary otherwise. When the store is fresh, the candidates SHALL be, in config order, the first of these sets that is not empty: the passing primary backends (`state` `healthy`, `reason` `null`, `backup` false); when there are none, the passing backup backends (`backup` true); when there are none, every `healthy` primary backend, failing ones included; when there are none, every `healthy` backup backend, failing ones included. When the store is stale, `reason` SHALL NOT affect the candidates. When no backend is a candidate (none healthy, or the store is stale), a service with `on_all_down = "closed"` SHALL fail with `ALL_DOWN`. A service with `on_all_down = "open"` SHALL then use, in config order: when the store is fresh and at least one backend is unknown and passing (`state` `unknown` and `reason` `null`), only those backends; otherwise, when the store is fresh and at least one backend's `state` is `unknown`, every `unknown` backend; otherwise every backend. When the store is stale, every backend SHALL be a candidate of an open service. The lookup SHALL NOT tell the caller that the result came from an all-down service or from a service whose healthy backends are all failing.

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

#### Scenario: Backup takes over while no primary passes

- **WHEN** a fresh round-robin service with `on_all_down = "closed"` lists `10.0.1.5:5432` (`healthy`, reason `timeout`), backup `10.0.2.5:5432` (`healthy`, reason `null`) and backup `10.0.2.6:5432` (`healthy`, reason `null`)
- **THEN** consecutive lookups return `10.0.2.5:5432` and `10.0.2.6:5432` and never `10.0.1.5:5432`

#### Scenario: Primary passes again

- **WHEN** the same service's `10.0.1.5:5432` has reason `null` again
- **THEN** every lookup returns `10.0.1.5:5432`

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
| A (healthy, "timeout"), B backup (healthy, null) | fresh | closed | B |
| A (healthy, "timeout"), B backup (healthy, "refused") | fresh | closed | A |
| A (unhealthy, "refused"), B backup (healthy, "refused") | fresh | closed | B |
| A (unhealthy, "refused"), B backup (unknown, null) | fresh | open | B |
| A (healthy, null), B backup (healthy, null) | stale | open | A, B |
