## MODIFIED Requirements

### Requirement: Candidate backends per address family

To choose a backend for an address family (IPv4 or IPv6), the system SHALL use only the service's backends of that family, in config order. If there are none, the result SHALL be "no backend of this family". Otherwise the candidates SHALL be the passing backends of that family, where a backend is passing when its `state` is `healthy` and its `reason` is `null`; if none of them is passing, the candidates SHALL be the `healthy` backends of that family, including those whose `reason` is not `null` (failing backends); if none of them is `healthy` and `on_all_down` is `open`, the candidates SHALL be the backends of that family whose `state` is `unknown` and `reason` is `null` (passing unknown backends), or, when there are none, every `unknown` backend of that family, or, when there are none, all backends of that family; if none of them is `healthy` and `on_all_down` is `closed`, the result SHALL be "all down".

#### Scenario: Unhealthy backends are skipped

- **WHEN** a service has IPv4 backends 10.0.0.1 (healthy) and 10.0.0.2 (unhealthy) and an IPv4 backend is chosen
- **THEN** the only candidate is 10.0.0.1

#### Scenario: Failing backends are skipped while another passes

- **WHEN** a service has IPv4 backends 10.0.0.1 (healthy, reason `connect: Connection refused`) and 10.0.0.2 (healthy, reason `null`) and an IPv4 backend is chosen
- **THEN** the only candidate is 10.0.0.2

#### Scenario: Every healthy backend failing

- **WHEN** a service with `on_all_down = "closed"` has IPv4 backends 10.0.0.1 (healthy, reason `timeout`), 10.0.0.2 (healthy, reason `timeout`) and 10.0.0.3 (unhealthy) and an IPv4 backend is chosen
- **THEN** the candidates are 10.0.0.1 and 10.0.0.2

#### Scenario: Open service prefers unknown backends

- **WHEN** a service with `on_all_down = "open"` has IPv4 backends 10.0.0.1 (unhealthy), 10.0.0.2 (unknown, reason `timeout`) and 10.0.0.3 (unknown, reason `null`) and an IPv4 backend is chosen
- **THEN** the only candidate is 10.0.0.3

##### Example: candidate sets

| IPv4 backends (state) | IPv6 backends (state) | `on_all_down` | Family asked | Result |
| --- | --- | --- | --- | --- |
| 10.0.0.1 (healthy), 10.0.0.2 (unhealthy) | none | open | IPv4 | candidates: 10.0.0.1 |
| 10.0.0.1 (unhealthy), 10.0.0.2 (unknown) | none | open | IPv4 | candidates: 10.0.0.2 |
| 10.0.0.1 (unhealthy), 10.0.0.2 (unknown, reason "timeout"), 10.0.0.3 (unknown, reason "refused") | none | open | IPv4 | candidates: 10.0.0.2, 10.0.0.3 |
| 10.0.0.1 (unhealthy), 10.0.0.2 (unhealthy) | 2001:db8::1 (unknown) | open | IPv4 | candidates: 10.0.0.1, 10.0.0.2 |
| 10.0.0.1 (unhealthy), 10.0.0.2 (unknown) | none | closed | IPv4 | all down |
| 10.0.0.1 (healthy) | none | open | IPv6 | no backend of this family |
| 10.0.0.1 (unhealthy) | 2001:db8::1 (healthy) | closed | IPv4 | all down |
| 10.0.0.1 (unhealthy) | 2001:db8::1 (healthy) | closed | IPv6 | candidates: 2001:db8::1 |
| 10.0.0.1 (healthy, reason "refused"), 10.0.0.2 (healthy, reason null) | none | closed | IPv4 | candidates: 10.0.0.2 |
| 10.0.0.1 (healthy, reason "refused"), 10.0.0.2 (unhealthy) | 2001:db8::1 (healthy, reason null) | closed | IPv4 | candidates: 10.0.0.1 |
