## MODIFIED Requirements

### Requirement: Balance methods

Each service SHALL have a `balance` of `round-robin`, `sticky`, `failover`, `weighted`, `locality`, or `fastest`, taken from the service table, otherwise from `[defaults]`, otherwise `round-robin`.

#### Scenario: Default balance

- **WHEN** neither `[defaults]` nor the service sets `balance`
- **THEN** the service's `balance` is `round-robin`

#### Scenario: Service overrides default

- **WHEN** `[defaults]` sets `balance = "failover"` and a service sets `balance = "round-robin"`
- **THEN** that service's `balance` is `round-robin` and services without their own `balance` use `failover`

#### Scenario: New methods accepted

- **WHEN** one service sets `balance = "weighted"`, another sets `balance = "locality"`, and a third sets `balance = "fastest"`
- **THEN** the config is valid and the store records show `"weighted"`, `"locality"` and `"fastest"` respectively

#### Scenario: Unknown value rejected

- **WHEN** a service sets `balance = "random"`
- **THEN** the config is invalid and the error for `balance` lists `round-robin`, `sticky`, `failover`, `weighted`, `locality` and `fastest`

## ADDED Requirements

### Requirement: Fast membership decided by the daemon

For every `fastest` service, the daemon SHALL keep a fast flag for each backend and SHALL recompute it for all backends of the service whenever a backend's state or latency changes and after a reload. Only backends that are `healthy` and have a latency, of either address family, SHALL be in scope; every other backend SHALL NOT be fast. With `min` the lowest latency in scope in microseconds, a backend in scope that is not fast SHALL become fast when its latency is at most the larger of `min * 3 / 2` (integer division) and `min + 1000`, and a fast backend in scope SHALL stay fast while its latency is at most the larger of `min * 2` and `min + 2000`, and SHALL become not fast otherwise. A backend that is added, reset to `unknown`, or becomes `unhealthy` SHALL NOT be fast. The service's pool SHALL be the ids of its fast backends in config order; the daemon SHALL write it to the store and SHALL use it for DNS answers. Readers SHALL use the pool as written and SHALL NOT recompute it.

#### Scenario: Nearly equal backends are fast

- **WHEN** a service has healthy backends with latencies 2000, 2800 and 5000 microseconds, none of them fast before
- **THEN** after the recomputation the first two are fast and the third is not

#### Scenario: A fast backend near the edge stays fast

- **WHEN** a fast backend's latency rises from 2900 to 3500 microseconds while the lowest latency in the service stays 2000
- **THEN** the backend stays fast, and it becomes not fast only when its latency exceeds 4000

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

### Requirement: Fastest choice in the daemon

For a `fastest` service, the daemon SHALL take the backends of the service's pool that are of the requested family. When there are any, it SHALL choose among them by round-robin with the service's per-family counter; otherwise it SHALL choose as for `round-robin`, with the same counter, including the rules for no backend of the family and all down.

#### Scenario: Fast backends preferred

- **WHEN** a `fastest` service has healthy 10.0.0.1, 10.0.0.2 and 10.0.0.3 with pool [10.0.0.1, 10.0.0.2], and four A queries are made
- **THEN** the answers alternate between 10.0.0.1 and 10.0.0.2 and never include 10.0.0.3

#### Scenario: Empty pool

- **WHEN** a `fastest` service has healthy 10.0.0.1 and 10.0.0.2 with an empty pool (no latency yet), and two A queries are made
- **THEN** the two answers are different backends

#### Scenario: All down open

- **WHEN** every backend of a `fastest` service is unhealthy and `on_all_down = "open"`
- **THEN** the pool is empty and the A answers rotate over every backend

#### Scenario: Pool without the family

- **WHEN** a `fastest` service has healthy 10.0.0.1 and 2001:db8::1 and the pool is [2001:db8::1], and two A queries are made
- **THEN** both answers are 10.0.0.1
