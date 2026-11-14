## MODIFIED Requirements

### Requirement: Candidate backends per address family

To choose a backend for an address family (IPv4 or IPv6), the system SHALL use only the service's backends of that family, in config order. If there are none, the result SHALL be "no backend of this family". Otherwise the candidates SHALL be the first of these sets of that family's backends that is not empty: the passing primary backends, where a backend is passing when its `state` is `healthy` and its `reason` is `null` and primary when its `backup` is false; the passing backup backends (`backup` true); the `healthy` primary backends, including those whose `reason` is not `null` (failing backends); the `healthy` backup backends, failing ones included. If none of them is `healthy` and `on_all_down` is `open`, the candidates SHALL be the backends of that family whose `state` is `unknown` and `reason` is `null` (passing unknown backends), or, when there are none, every `unknown` backend of that family, or, when there are none, all backends of that family; if none of them is `healthy` and `on_all_down` is `closed`, the result SHALL be "all down".

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

#### Scenario: Backup used while no primary passes

- **WHEN** a service has IPv4 backends 10.0.0.1 (healthy, reason `timeout`), 10.0.0.2 (unhealthy) and backup 10.0.1.1 (healthy, reason `null`) and an IPv4 backend is chosen
- **THEN** the only candidate is 10.0.1.1

#### Scenario: Passing primary wins over a backup

- **WHEN** a service has IPv4 backends 10.0.0.1 (healthy, reason `null`) and backup 10.0.1.1 (healthy, reason `null`) and an IPv4 backend is chosen
- **THEN** the only candidate is 10.0.0.1

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
| 10.0.0.1 (healthy, reason "timeout"), backup 10.0.1.1 (healthy, reason "timeout") | none | closed | IPv4 | candidates: 10.0.0.1 |
| 10.0.0.1 (unhealthy), backup 10.0.1.1 (healthy, reason "timeout") | none | closed | IPv4 | candidates: 10.0.1.1 |
| 10.0.0.1 (unhealthy), backup 10.0.1.1 (unknown) | none | open | IPv4 | candidates: 10.0.1.1 |
| 10.0.0.1 (unhealthy), backup 10.0.1.1 (unhealthy) | none | open | IPv4 | candidates: 10.0.0.1, 10.0.1.1 |

### Requirement: Fast membership decided by the daemon

For every `fastest` service, the daemon SHALL keep a fast flag for each backend and SHALL recompute it for all backends of the service whenever a backend's state or latency changes and after a reload. Only backends that are `healthy` and have a latency, of either address family, SHALL be in scope; every other backend SHALL NOT be fast. Primaries (backends without `backup`) and backups SHALL be decided separately: with `min` the lowest latency in microseconds among the backends in scope of the same kind (primary or backup) as the backend, a backend in scope that is not fast SHALL become fast when its latency is at most the larger of `min * 3 / 2` (integer division) and `min + 1000`, and a fast backend in scope SHALL stay fast while its latency is at most the larger of `min * 2` and `min + 2000`, and SHALL become not fast otherwise. A backend that is added, reset to `unknown`, or becomes `unhealthy` SHALL NOT be fast. The service's pool SHALL be the ids of its fast backends in config order; the daemon SHALL write it to the store and SHALL use it for DNS answers. Readers SHALL use the pool as written and SHALL NOT recompute it.

#### Scenario: Nearly equal backends are fast

- **WHEN** a service has healthy backends with latencies 2000, 2800 and 5000 microseconds, none of them fast before
- **THEN** after the recomputation the first two are fast and the third is not

#### Scenario: A fast backend near the edge stays fast

- **WHEN** a fast backend's latency rises from 2900 to 3500 microseconds while the lowest latency in the service stays 2000
- **THEN** the backend stays fast, and it becomes not fast only when its latency exceeds 4000

#### Scenario: Backups do not decide which primaries are fast

- **WHEN** a service has healthy primaries 10.0.0.1 with latency 20000 and 10.0.0.2 with latency 100000, and a healthy backup 10.0.1.1 with latency 2000, none of them fast before
- **THEN** the pool is 10.0.0.1 and 10.0.1.1, so while both primaries pass their checks every lookup returns 10.0.0.1

#### Scenario: Missing latency is not fast

- **WHEN** a service has healthy 10.0.0.1 with latency 900 and healthy 10.0.0.2 with no latency
- **THEN** 10.0.0.1 is fast and 10.0.0.2 is not

##### Example: fast membership

| Lowest latency in scope (µs) | Backend (state, latency µs, fast before) | Fast after (in the pool) |
| --- | --- | --- |
| 2000 | healthy, 2000, false | true |
| 2000 | healthy, 3000, false | true |
| 2000 | healthy, 3500, false | false |
| 2000 | healthy, 3500, true | true |
| 2000 | healthy, 4000, true | true |
| 2000 | healthy, 4001, true | false |
| 2000 | unknown, 100, false | false |
| 2000 | healthy, none, true | false |
| 200 | healthy, 1200, false | true |
| 200 | healthy, 1201, false | false |
| 200 | healthy, 2200, true | true |
| 200 | healthy, 2201, true | false |
