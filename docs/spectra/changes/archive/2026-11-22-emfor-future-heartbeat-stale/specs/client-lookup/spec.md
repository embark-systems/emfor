## MODIFIED Requirements

### Requirement: Stale heartbeat makes every backend unknown

The store SHALL be treated as stale when `meta/heartbeat` is `0`, or when the heartbeat differs from the reader's current Unix time in milliseconds by more than the maximum staleness (default 3 seconds), whether the heartbeat is older or later than the reader's clock. A heartbeat later than the reader's clock by no more than the maximum staleness SHALL count as fresh. When the store is stale, the lookup SHALL ignore every backend's `state` and treat no backend as healthy, so the service's `on_all_down` decides the result. There SHALL be no separate error code for staleness.

#### Scenario: Stale store with each on_all_down setting

- **WHEN** the heartbeat is older than the maximum staleness and every backend is recorded `healthy`
- **THEN** a service with `on_all_down = "closed"` fails with `ALL_DOWN`, and a service with `on_all_down = "open"` returns one of its backends

#### Scenario: Heartbeat boundaries

- **WHEN** the maximum staleness is 3000 ms and the reader's clock reads 1700000010000
- **THEN** freshness is decided as listed

##### Example: heartbeat values

| `meta/heartbeat` | Store is |
| --- | --- |
| `1700000007000` (3000 ms old) | fresh |
| `1700000006999` (3001 ms old) | stale |
| `1700000013000` (3000 ms in the future) | fresh |
| `1700000013001` (3001 ms in the future) | stale |
| `1700000015000` (5000 ms in the future) | stale |
| `0` | stale |

#### Scenario: Future heartbeat of a stopped daemon

- **WHEN** the reader's clock was set back by 60 seconds after the daemon last wrote its heartbeat, the daemon no longer runs, the service has `on_all_down = "closed"` and every backend is recorded `healthy`
- **THEN** the lookup fails with `ALL_DOWN`, because the store is stale, instead of returning a backend until the clock catches up with the heartbeat
