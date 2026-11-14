## MODIFIED Requirements

### Requirement: Store record format version 1

The store SHALL contain these keys: `meta/schema_version` with value `1`; `meta/heartbeat` with the current Unix time in milliseconds as a decimal string; `meta/started_at` with the daemon start time in Unix milliseconds; `meta/site` with the config's top-level `site` label, present only when the config sets `site`; and one `svc/<service name>` key per configured service. Each service value SHALL be a JSON object with `name` (string), `on_all_down` (`"open"` or `"closed"`), `balance` (`"round-robin"`, `"sticky"`, `"failover"`, `"weighted"`, `"locality"`, or `"fastest"`), `all_down` (boolean), `slow_start_ms` (the service's `slow_start` in milliseconds as a non-negative integer, 0 when slow start is off), `backends` (array in config order), and, only when `balance` is `"fastest"`, `pool` (array of backend `id` strings in config order, empty when no backend is fast, as defined by the backend-selection requirement "Fast membership decided by the daemon"). Each backend object SHALL have `id` (backend string from config), `ip` (string), `port` (number), `weight` (number from 1 to 100), `site` (string, or `null` when the backend has no site), `state` (`"unknown"`, `"healthy"`, or `"unhealthy"`), `checked_at` (Unix milliseconds when the last check finished, or `null` before the first check), `reason` (message of the last `down` or `error` result, or `null` after an `up`), `latency_us` (the backend's check latency in microseconds as a non-negative integer, or `null` when it has none; see "Check latency"), and `recovered_at` (Unix milliseconds of the check that last turned the backend from `unhealthy` to `healthy`, or `null`; see "Recovery time in the store"). Readers SHALL ignore object fields and keys they do not know, and SHALL read a missing `weight` as 1 a missing `site` as `null`, a missing `latency_us` as `null`, a missing `pool` as empty, a missing `slow_start_ms` as 0, and a missing `recovered_at` as `null`. A reload that sets, changes or removes the top-level `site` SHALL write or delete `meta/site` accordingly.

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
- **THEN** the value parses to `{"name":"db.svc.emfor","on_all_down":"closed","balance":"round-robin","all_down":false,"slow_start_ms":0,"backends":[{"id":"[2001:db8::5]:5432","ip":"2001:db8::5","port":5432,"weight":1,"site":null,"state":"healthy","checked_at":1762905600123,"reason":null,"latency_us":1834,"recovered_at":null}]}` when its smoothed check latency is 1834 microseconds

##### Example: weight and site written

- **GIVEN** service `api.svc.emfor` with backend `{ addr = "10.0.0.11:8080", weight = 3, site = "dc1" }`
- **WHEN** a reader gets key `svc/api.svc.emfor`
- **THEN** that backend object has `"weight":3` and `"site":"dc1"`

##### Example: slow start fields written

- **GIVEN** service `api.svc.emfor` with `slow_start = "30s"` whose backend `10.0.0.11:8080` turned from `unhealthy` to `healthy` after a check that finished at 1762905600123
- **WHEN** a reader gets key `svc/api.svc.emfor`
- **THEN** the service object has `"slow_start_ms":30000` and that backend object has `"recovered_at":1762905600123`

## ADDED Requirements

### Requirement: Recovery time in the store

The daemon SHALL set a backend's `recovered_at` to the `checked_at` of the check that turns it from `unhealthy` to `healthy`, and SHALL set it to `null` on every other state transition, including `unknown` to `healthy` and any transition out of `healthy`. Without a transition `recovered_at` SHALL keep its value. The new value SHALL be committed together with the state change. On reload, a backend whose state is kept SHALL keep its `recovered_at`, and a backend that starts again from `unknown` SHALL have `recovered_at` `null`. On startup every backend SHALL have `recovered_at` `null`.

#### Scenario: Recovery recorded

- **WHEN** backend `10.0.0.11:8080` turns from `unhealthy` to `healthy` after a check that finished at 1762905600123
- **THEN** the same commit that writes its `state` `"healthy"` writes `recovered_at` 1762905600123

#### Scenario: First healthy after start is not a recovery

- **WHEN** the daemon starts and backend `10.0.0.11:8080` turns from `unknown` to `healthy`
- **THEN** its `recovered_at` is `null`

#### Scenario: Recovery cleared when the backend fails again

- **WHEN** a backend with `recovered_at` 1762905600123 turns from `healthy` to `unhealthy`
- **THEN** its `recovered_at` is `null`

##### Example: recovered_at after each transition

| Transition | `recovered_at` before | `checked_at` of the check | `recovered_at` after |
| --- | --- | --- | --- |
| unhealthy → healthy | null | 1762905600123 | 1762905600123 |
| unknown → healthy | null | 1762905600123 | null |
| healthy → unhealthy | 1762905600123 | 1762905609000 | null |
| none (healthy stays healthy) | 1762905600123 | 1762905602000 | 1762905600123 |
