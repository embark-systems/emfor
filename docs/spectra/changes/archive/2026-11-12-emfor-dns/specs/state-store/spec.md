## MODIFIED Requirements

### Requirement: Store record format version 1

The store SHALL contain these keys: `meta/schema_version` with value `1`; `meta/heartbeat` with the current Unix time in milliseconds as a decimal string; `meta/started_at` with the daemon start time in Unix milliseconds; and one `svc/<service name>` key per configured service. Each service value SHALL be a JSON object with `name` (string), `on_all_down` (`"open"` or `"closed"`), `balance` (`"round-robin"`, `"sticky"`, or `"failover"`), `all_down` (boolean), and `backends` (array in config order). Each backend object SHALL have `id` (backend string from config), `ip` (string), `port` (number), `state` (`"unknown"`, `"healthy"`, or `"unhealthy"`), `checked_at` (Unix milliseconds when the last check finished, or `null` before the first check), and `reason` (message of the last `down` or `error` result, or `null` after an `up`).

#### Scenario: Service record after checks

- **WHEN** service `api.svc.emfor` has backend `10.0.0.11:8080` healthy and `10.0.0.12:8080` unhealthy after a refused connection
- **THEN** key `svc/api.svc.emfor` holds a JSON object with `all_down` false and two backends in config order, the second having `state` `"unhealthy"` and a non-null `reason`

#### Scenario: Balance written from config

- **WHEN** service `api.svc.emfor` sets `balance = "failover"` and the daemon starts
- **THEN** key `svc/api.svc.emfor` holds a JSON object whose `balance` is `"failover"`

##### Example: record shape

- **GIVEN** service `db.svc.emfor` with `on_all_down = "closed"`, no `balance` setting, and one backend `[2001:db8::5]:5432` that has passed `rise` checks, last check finished at 1762905600123
- **WHEN** a reader gets key `svc/db.svc.emfor`
- **THEN** the value parses to `{"name":"db.svc.emfor","on_all_down":"closed","balance":"round-robin","all_down":false,"backends":[{"id":"[2001:db8::5]:5432","ip":"2001:db8::5","port":5432,"state":"healthy","checked_at":1762905600123,"reason":null}]}`
