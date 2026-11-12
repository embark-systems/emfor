## MODIFIED Requirements

### Requirement: Store record format version 1

The store SHALL contain these keys: `meta/schema_version` with value `1`; `meta/heartbeat` with the current Unix time in milliseconds as a decimal string; `meta/started_at` with the daemon start time in Unix milliseconds; `meta/site` with the config's top-level `site` label, present only when the config sets `site`; and one `svc/<service name>` key per configured service. Each service value SHALL be a JSON object with `name` (string), `on_all_down` (`"open"` or `"closed"`), `balance` (`"round-robin"`, `"sticky"`, `"failover"`, `"weighted"`, `"locality"`, or `"fastest"`), `all_down` (boolean), `backends` (array in config order), and, only when `balance` is `"fastest"`, `pool` (array of backend `id` strings in config order, empty when no backend is fast, as defined by the backend-selection requirement "Fast membership decided by the daemon"). Each backend object SHALL have `id` (backend string from config), `ip` (string), `port` (number), `weight` (number from 1 to 100), `site` (string, or `null` when the backend has no site), `state` (`"unknown"`, `"healthy"`, or `"unhealthy"`), `checked_at` (Unix milliseconds when the last check finished, or `null` before the first check), `reason` (message of the last `down` or `error` result, or `null` after an `up`), and `latency_us` (the backend's check latency in microseconds as a non-negative integer, or `null` when it has none; see "Check latency"). Readers SHALL ignore object fields and keys they do not know, and SHALL read a missing `weight` as 1 a missing `site` as `null`, a missing `latency_us` as `null`, and a missing `pool` as empty. A reload that sets, changes or removes the top-level `site` SHALL write or delete `meta/site` accordingly.

#### Scenario: Service record after checks

- **WHEN** service `api.svc.emfor` has backend `10.0.0.11:8080` healthy and `10.0.0.12:8080` unhealthy after a refused connection
- **THEN** key `svc/api.svc.emfor` holds a JSON object with `all_down` false and two backends in config order, the second having `state` `"unhealthy"` and a non-null `reason`

#### Scenario: Balance written from config

- **WHEN** service `api.svc.emfor` sets `balance = "failover"` and the daemon starts
- **THEN** key `svc/api.svc.emfor` holds a JSON object whose `balance` is `"failover"`

#### Scenario: Host site written and removed

- **WHEN** the daemon starts with top-level `site = "dc1"`, then the config is reloaded without `site`
- **THEN** key `meta/site` holds `dc1` after the start and does not exist after the reload

##### Example: record shape

- **GIVEN** service `db.svc.emfor` with `on_all_down = "closed"`, no `balance` setting, and one backend `[2001:db8::5]:5432` given as a string, that has passed `rise` checks, last check finished at 1762905600123
- **WHEN** a reader gets key `svc/db.svc.emfor`
- **THEN** the value parses to `{"name":"db.svc.emfor","on_all_down":"closed","balance":"round-robin","all_down":false,"backends":[{"id":"[2001:db8::5]:5432","ip":"2001:db8::5","port":5432,"weight":1,"site":null,"state":"healthy","checked_at":1762905600123,"reason":null,"latency_us":1834}]}` when its smoothed check latency is 1834 microseconds

##### Example: weight and site written

- **GIVEN** service `api.svc.emfor` with backend `{ addr = "10.0.0.11:8080", weight = 3, site = "dc1" }`
- **WHEN** a reader gets key `svc/api.svc.emfor`
- **THEN** that backend object has `"weight":3` and `"site":"dc1"`

## ADDED Requirements

### Requirement: Check latency

For every backend, regardless of the service's `balance`, the daemon SHALL time each check with a monotonic clock from the start of the check to its result. For an `up` result it SHALL set the backend's latency to the measured duration in whole microseconds when the backend has no latency, and otherwise to `round(0.05 * sample + 0.95 * previous)`, computed as `(sample + 19 * previous + 10) / 20` in integer arithmetic. A `down` or `error` result SHALL NOT change the latency. When the backend's state changes to `unhealthy`, its latency SHALL become empty. The latency SHALL be empty when the daemon starts and when a backend is added or reset to `unknown` on reload, and SHALL be kept on reload whenever the backend keeps its state. The daemon SHALL log each up check's raw duration at `debug` level with the service and backend. A change of latency or of `pool` alone SHALL be committed to the store no later than the next heartbeat; a health state change SHALL be committed immediately together with the `pool` recomputed for it. The DNS server and the store SHALL use the same latency and pool values.

#### Scenario: Latency after up checks

- **WHEN** a backend's first three up checks take 1000, 2000 and 2000 microseconds
- **THEN** its `latency_us` is 1000, then 1050, then 1098

#### Scenario: Pool written for a fastest service

- **WHEN** a `fastest` service `api.svc.emfor` has one backend `10.0.0.11:8080` that is healthy with a latency, and it later becomes unhealthy
- **THEN** its record has `"pool":["10.0.0.11:8080"]` while the backend is healthy and `"pool":[]` after it becomes unhealthy, and records of services with other `balance` values have no `pool` key

#### Scenario: Cleared on unhealthy

- **WHEN** a healthy backend with `latency_us` 1200 fails `fall` checks in a row and becomes unhealthy
- **THEN** its `latency_us` is `null`, and after it recovers its first up check of 3000 microseconds sets `latency_us` to 3000

#### Scenario: Down result keeps latency

- **WHEN** a healthy backend with `latency_us` 1200 has one `down` result that does not reach `fall`
- **THEN** its `latency_us` stays 1200

##### Example: smoothing

| Previous latency | Sample (µs) | Result | New latency |
| --- | --- | --- | --- |
| none | 1000 | up | 1000 |
| 1000 | 2000 | up | 1050 |
| 1050 | 2000 | up | 1098 |
| 1098 | 900 | down | 1098 |
| 1098 | 900 | error | 1098 |
