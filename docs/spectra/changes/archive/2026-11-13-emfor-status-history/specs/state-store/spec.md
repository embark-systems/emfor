## ADDED Requirements

### Requirement: Backend state history

The daemon SHALL keep, for every configured service, a key `hist/<service name>` holding the recent health state transitions of its backends. The value SHALL be a JSON object `{"backends":{"<backend id>":[<entry>, ...]}}` whose entry arrays are ordered oldest first; each entry SHALL be `{"at":<Unix ms>,"from":"<state>","to":"<state>","reason":<string or null>}`, where `at` is the `checked_at` of the check that caused the transition, `from` and `to` are `"unknown"`, `"healthy"` or `"unhealthy"`, and `reason` is the backend's `reason` after that check. A backend SHALL keep at most 20 entries; adding a 21st SHALL drop the oldest. A backend with no entries SHALL be absent from `backends`, and a service with no entries SHALL have no `hist/` key.

Only transitions produced by a check result SHALL be recorded; resetting backends to `unknown` on start or because `check` or `check_options` changed SHALL NOT add entries. The `hist/` key SHALL be written in the same transaction as the `svc/` record that carries the transition, and a failed commit SHALL be retried at the next heartbeat with the history as it is in memory at that time.

History SHALL survive a daemon restart. On start the daemon SHALL read every existing `hist/*` key, drop the keys of services that are not configured and the entries of backends that are not configured, and write the result in the same transaction that writes the fresh `svc/*` records. A `hist/` value that does not parse SHALL be logged as a warning and discarded. After a successful reload the daemon SHALL delete `hist/<name>` of each removed service and drop the entries of each removed backend. `meta/schema_version` SHALL stay `1`; readers that do not know `hist/` keys ignore them.

#### Scenario: Transition recorded with its record

- **WHEN** backend `10.0.0.12:8080` of `api.svc.emfor` turns from healthy to unhealthy after a check that finished at 1762905600123 with reason `connect: Connection refused`
- **THEN** in the same commit that writes `svc/api.svc.emfor`, key `hist/api.svc.emfor` gains the entry `{"at":1762905600123,"from":"healthy","to":"unhealthy","reason":"connect: Connection refused"}` as the last element of `backends["10.0.0.12:8080"]`

#### Scenario: Oldest entry dropped past 20

- **WHEN** a backend already has 20 entries and makes another transition
- **THEN** it still has 20 entries, the oldest one is gone, and the newest is last

##### Example: cap at 20

- **GIVEN** backend `10.0.0.11:8080` with entries whose `at` values are 1 through 20
- **WHEN** a transition with `at` 21 is recorded
- **THEN** its entries have `at` values 2 through 21

#### Scenario: History kept across a restart

- **WHEN** the store holds `hist/api.svc.emfor` with 3 entries for `10.0.0.11:8080` and the daemon restarts with the same config
- **THEN** every backend in `svc/api.svc.emfor` is `unknown` and `hist/api.svc.emfor` still holds those 3 entries

#### Scenario: History pruned on start and reload

- **WHEN** the store holds `hist/old.svc.emfor` and `hist/api.svc.emfor` with entries for `10.0.0.11:8080` and `10.0.0.99:8080`, and the daemon starts with a config holding only `api.svc.emfor` with backend `10.0.0.11:8080`
- **THEN** `hist/old.svc.emfor` is gone and `hist/api.svc.emfor` holds entries only for `10.0.0.11:8080`

#### Scenario: Unparseable history discarded

- **WHEN** `hist/api.svc.emfor` holds `not json` and the daemon starts
- **THEN** the daemon logs a warning naming `hist/api.svc.emfor`, starts normally, and the key holds only transitions recorded after the start

## MODIFIED Requirements

### Requirement: Fresh state on every start

On startup the daemon SHALL write `meta/schema_version` and `meta/started_at`, delete every existing `svc/*` key, and write a record for every configured service with every backend `unknown`, `checked_at` null, and `reason` null, before running any check. Backend state from a previous run SHALL NOT be reused. The backend state history under `hist/*` is the one exception: it SHALL be kept and pruned as described in "Backend state history".

#### Scenario: Old state discarded

- **WHEN** the store contains `svc/old.svc.emfor` and `svc/api.svc.emfor` with healthy backends from a previous run, and the daemon starts with a config containing only `api.svc.emfor`
- **THEN** `svc/old.svc.emfor` is gone and every backend in `svc/api.svc.emfor` is `unknown`
