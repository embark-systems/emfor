## ADDED Requirements

### Requirement: Drained backends are never selected

A backend whose record has `drain` true SHALL NOT be a candidate in any tier, including the last tier that `on_all_down = "open"` falls back to and the tiers used while the heartbeat is stale, for every `balance` method (`sticky` and `failover` included), for DNS A, AAAA and SRV answers, and for the `fastest` pool. Other backends SHALL be chosen by the existing rules as if the drained ones were not in the list. When every backend of a service is drained, no backend SHALL be chosen: a lookup SHALL fail as `ALL_DOWN` and a DNS query SHALL be answered with SERVFAIL, whatever `on_all_down` is.

#### Scenario: Drained backend is skipped

- **WHEN** a service has backends A and B, and A is drained
- **THEN** every pick for every `balance` method returns B

#### Scenario: Sticky key moves away

- **WHEN** a `sticky` key mapped to A and A becomes drained
- **THEN** the key maps to another backend until A is no longer drained, when it maps to A again

#### Scenario: Not even the last tier

- **WHEN** A is drained, B is unhealthy and `on_all_down` is `open`
- **THEN** B is chosen, never A

#### Scenario: Everything drained

- **WHEN** both backends are drained and `on_all_down` is `open`
- **THEN** a lookup fails as `ALL_DOWN` and a DNS query is answered with SERVFAIL

#### Scenario: Fast pool

- **WHEN** a `fastest` service has a drained backend with the lowest latency
- **THEN** it is not in `pool` and does not set the latency that the other backends are compared with
