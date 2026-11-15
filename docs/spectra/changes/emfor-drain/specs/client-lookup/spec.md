## ADDED Requirements

### Requirement: Clients exclude drained backends

A client SHALL read a missing `drain` in a backend record as `false`, and SHALL exclude a backend with `drain` true from every candidate tier as defined by the backend-selection requirement "Drained backends are never selected". When no backend can be chosen because all are drained, the lookup SHALL fail with `ALL_DOWN`. A record with `drain` or `checked_state` SHALL NOT make the lookup fail, and the store keeps format version 2.

#### Scenario: Lookup with a drained backend

- **WHEN** the store holds backends A (drained, `state` unhealthy) and B (healthy)
- **THEN** every lookup returns B

#### Scenario: Lookup when all are drained

- **WHEN** every backend in the record is drained
- **THEN** the lookup fails with `ALL_DOWN`, also with `on_all_down` `open`

#### Scenario: Older store

- **WHEN** the record has no `drain` field
- **THEN** the backends are read exactly as before
