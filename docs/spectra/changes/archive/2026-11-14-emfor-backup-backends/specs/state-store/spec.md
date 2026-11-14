## ADDED Requirements

### Requirement: Store record format version 2

The store SHALL contain these keys: `meta/schema_version` with value `2`; `meta/heartbeat` with the current Unix time in milliseconds as a decimal string; `meta/started_at` with the daemon start time in Unix milliseconds; `meta/site` with the config's top-level `site` label, present only when the config sets `site`; and one `svc/<service name>` key per configured service. Each service value SHALL be a JSON object with `name` (string), `on_all_down` (`"open"` or `"closed"`), `balance` (`"round-robin"`, `"sticky"`, `"failover"`, `"weighted"`, `"locality"`, or `"fastest"`), `all_down` (boolean), `slow_start_ms` (the service's `slow_start` in milliseconds as a non-negative integer, 0 when slow start is off), `backends` (array in config order), and, only when `balance` is `"fastest"`, `pool` (array of backend `id` strings in config order, empty when no backend is fast, as defined by the backend-selection requirement "Fast membership decided by the daemon"). Each backend object SHALL have `id` (backend string from config), `ip` (string), `port` (number), `weight` (number from 1 to 100), `site` (string, or `null` when the backend has no site), `backup` (boolean, `true` for a backend configured with `backup = true`), `state` (`"unknown"`, `"healthy"`, or `"unhealthy"`), `checked_at` (Unix milliseconds when the last check finished, or `null` before the first check), `reason` (message of the last `down` or `error` result, or `null` after an `up`), `latency_us` (the backend's check latency in microseconds as a non-negative integer, or `null` when it has none; see "Check latency"), and `recovered_at` (Unix milliseconds of the check that last turned the backend from `unhealthy` to `healthy`, or `null`; see "Recovery time in the store"). Readers SHALL ignore object fields and keys they do not know, and SHALL read a missing `weight` as 1, a missing `site` as `null`, a missing `latency_us` as `null`, a missing `pool` as empty, a missing `slow_start_ms` as 0, a missing `recovered_at` as `null`, and a missing `backup` as `false`. Version 2 differs from version 1 only by the `backup` field; a reader that accepts version 2 SHALL also accept a store whose `meta/schema_version` is `1` and read it the same way. A reload that sets, changes or removes the top-level `site` SHALL write or delete `meta/site` accordingly.

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
- **THEN** the value parses to `{"name":"db.svc.emfor","on_all_down":"closed","balance":"round-robin","all_down":false,"slow_start_ms":0,"backends":[{"id":"[2001:db8::5]:5432","ip":"2001:db8::5","port":5432,"weight":1,"site":null,"backup":false,"state":"healthy","checked_at":1762905600123,"reason":null,"latency_us":1834,"recovered_at":null}]}` when its smoothed check latency is 1834 microseconds

##### Example: weight and site written

- **GIVEN** service `api.svc.emfor` with backend `{ addr = "10.0.0.11:8080", weight = 3, site = "dc1" }`
- **WHEN** a reader gets key `svc/api.svc.emfor`
- **THEN** that backend object has `"weight":3` and `"site":"dc1"`

##### Example: backup written

- **GIVEN** service `api.svc.emfor` with backends `10.0.0.11:8080` and `{ addr = "10.0.1.11:8080", backup = true }`
- **WHEN** a reader gets key `svc/api.svc.emfor` and `meta/schema_version`
- **THEN** the first backend object has `"backup":false`, the second has `"backup":true`, and `meta/schema_version` is `2`

##### Example: slow start fields written

- **GIVEN** service `api.svc.emfor` with `slow_start = "30s"` whose backend `10.0.0.11:8080` turned from `unhealthy` to `healthy` after a check that finished at 1762905600123
- **WHEN** a reader gets key `svc/api.svc.emfor`
- **THEN** the service object has `"slow_start_ms":30000` and that backend object has `"recovered_at":1762905600123`

## REMOVED Requirements

### Requirement: Store record format version 1

**Reason**: Replaced by "Store record format version 2", which adds the backend field `backup` and raises `meta/schema_version` to `2`.

**Migration**: Upgrade every client and `emfor status` before the daemon; they read stores of version 1 and 2. Older clients fail with `INCOMPATIBLE` on a version 2 store.
