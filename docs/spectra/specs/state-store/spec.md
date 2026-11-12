# state-store Specification

## Purpose

Defines the local LMDB file through which emfor publishes health state to readers on the same host. The layout and record format are a versioned cross-language contract, so every client binding can read it directly and detect when the daemon is no longer running.

## Requirements

### Requirement: LMDB file layout

The daemon SHALL store state in an LMDB environment opened with `MDB_NOSUBDIR` at `[store] path` (default `/var/lib/emfor/state.mdb`), producing one data file and one lock file named `<path>-lock`. It SHALL use only the unnamed default database. Keys and values SHALL be UTF-8 strings. The data file SHALL be created with `[store] file_mode` (default `0644`) and the lock file with `[store] lock_file_mode` (default `0666`). The map size SHALL be `[store] map_size` (default `64MiB`).

#### Scenario: Files created on first start

- **WHEN** the daemon starts with `path = "/tmp/t/state.mdb"` and no existing files
- **THEN** `/tmp/t/state.mdb` and `/tmp/t/state.mdb-lock` exist with modes 0644 and 0666


<!-- @trace
source: emfor-core
updated: 2026-11-12
code:
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/writer.rs
  - Cargo.toml
  - crates/emfor-daemon/src/cli.rs
  - CLAUDE.md
  - crates/emfor-daemon/src/main.rs
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/Cargo.toml
  - .spectra.yaml
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-daemon/src/checker/mod.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - examples/emfor.toml
  - crates/emfor-format/Cargo.toml
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Store record format version 1

The store SHALL contain these keys: `meta/schema_version` with value `1`; `meta/heartbeat` with the current Unix time in milliseconds as a decimal string; `meta/started_at` with the daemon start time in Unix milliseconds; and one `svc/<service name>` key per configured service. Each service value SHALL be a JSON object with `name` (string), `on_all_down` (`"open"` or `"closed"`), `all_down` (boolean), and `backends` (array in config order). Each backend object SHALL have `id` (backend string from config), `ip` (string), `port` (number), `state` (`"unknown"`, `"healthy"`, or `"unhealthy"`), `checked_at` (Unix milliseconds when the last check finished, or `null` before the first check), and `reason` (message of the last `down` or `error` result, or `null` after an `up`).

#### Scenario: Service record after checks

- **WHEN** service `api.svc.emfor` has backend `10.0.0.11:8080` healthy and `10.0.0.12:8080` unhealthy after a refused connection
- **THEN** key `svc/api.svc.emfor` holds a JSON object with `all_down` false and two backends in config order, the second having `state` `"unhealthy"` and a non-null `reason`

##### Example: record shape

- **GIVEN** service `db.svc.emfor` with `on_all_down = "closed"` and one backend `[2001:db8::5]:5432` that has passed `rise` checks, last check finished at 1762905600123
- **WHEN** a reader gets key `svc/db.svc.emfor`
- **THEN** the value parses to `{"name":"db.svc.emfor","on_all_down":"closed","all_down":false,"backends":[{"id":"[2001:db8::5]:5432","ip":"2001:db8::5","port":5432,"state":"healthy","checked_at":1762905600123,"reason":null}]}`


<!-- @trace
source: emfor-core
updated: 2026-11-12
code:
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/writer.rs
  - Cargo.toml
  - crates/emfor-daemon/src/cli.rs
  - CLAUDE.md
  - crates/emfor-daemon/src/main.rs
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/Cargo.toml
  - .spectra.yaml
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-daemon/src/checker/mod.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - examples/emfor.toml
  - crates/emfor-format/Cargo.toml
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: all_down reflects absence of healthy backends

A service's `all_down` SHALL be `true` exactly when none of its backends is `healthy`, including when backends are `unknown`. The daemon SHALL write the configured `on_all_down` value into the record unchanged; acting on it is the reader's responsibility.

#### Scenario: Startup state is all down

- **WHEN** the daemon has just started and no backend of a service has yet reached `rise`
- **THEN** that service's record has `all_down` true

##### Example: all_down computation

| Backend states | `all_down` |
| --- | --- |
| healthy, unhealthy | false |
| unhealthy, unhealthy | true |
| unknown, unhealthy | true |
| unknown, healthy | false |


<!-- @trace
source: emfor-core
updated: 2026-11-12
code:
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/writer.rs
  - Cargo.toml
  - crates/emfor-daemon/src/cli.rs
  - CLAUDE.md
  - crates/emfor-daemon/src/main.rs
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/Cargo.toml
  - .spectra.yaml
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-daemon/src/checker/mod.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - examples/emfor.toml
  - crates/emfor-format/Cargo.toml
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Heartbeat and timely state updates

The daemon SHALL update `meta/heartbeat` at least once per second while running. A change of any backend's health state SHALL be committed to the store immediately, without waiting for the next heartbeat. Changes to `checked_at` or `reason` alone SHALL be committed no later than the next heartbeat.

#### Scenario: Heartbeat advances

- **WHEN** a reader reads `meta/heartbeat` twice 2 seconds apart while the daemon runs
- **THEN** the second value is greater than the first

#### Scenario: State change visible before next heartbeat

- **WHEN** a backend changes from `healthy` to `unhealthy`
- **THEN** its new state is readable from the store before the next heartbeat commit


<!-- @trace
source: emfor-core
updated: 2026-11-12
code:
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/writer.rs
  - Cargo.toml
  - crates/emfor-daemon/src/cli.rs
  - CLAUDE.md
  - crates/emfor-daemon/src/main.rs
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/Cargo.toml
  - .spectra.yaml
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-daemon/src/checker/mod.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - examples/emfor.toml
  - crates/emfor-format/Cargo.toml
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Fresh state on every start

On startup the daemon SHALL write `meta/schema_version` and `meta/started_at`, delete every existing `svc/*` key, and write a record for every configured service with every backend `unknown`, `checked_at` null, and `reason` null, before running any check. State from a previous run SHALL NOT be reused.

#### Scenario: Old state discarded

- **WHEN** the store contains `svc/old.svc.emfor` and `svc/api.svc.emfor` with healthy backends from a previous run, and the daemon starts with a config containing only `api.svc.emfor`
- **THEN** `svc/old.svc.emfor` is gone and every backend in `svc/api.svc.emfor` is `unknown`


<!-- @trace
source: emfor-core
updated: 2026-11-12
code:
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/writer.rs
  - Cargo.toml
  - crates/emfor-daemon/src/cli.rs
  - CLAUDE.md
  - crates/emfor-daemon/src/main.rs
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/Cargo.toml
  - .spectra.yaml
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-daemon/src/checker/mod.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - examples/emfor.toml
  - crates/emfor-format/Cargo.toml
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Store updates on reload

After a successful reload the daemon SHALL delete records of services no longer configured, write records for new services with all backends `unknown`, and add new backends of existing services as `unknown`. When a service's `check` or `check_options` changed, all its backends SHALL be reset to `unknown` with counters at 0. Backends whose `check` and `check_options` are unchanged SHALL keep their state and counters even when `interval`, `timeout`, `rise`, `fall`, `jitter`, or `on_all_down` changed.

#### Scenario: Timing change keeps state

- **WHEN** a service with healthy backends has `interval` changed from 2s to 1s and the daemon reloads
- **THEN** its backends remain `healthy`

#### Scenario: Check options change resets state

- **WHEN** an `http` service's `check_options.path` changes from `/` to `/healthz` and the daemon reloads
- **THEN** all of its backends become `unknown`

#### Scenario: Removed service deleted

- **WHEN** service `old.svc.emfor` is removed from the config and the daemon reloads
- **THEN** key `svc/old.svc.emfor` no longer exists


<!-- @trace
source: emfor-core
updated: 2026-11-12
code:
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/writer.rs
  - Cargo.toml
  - crates/emfor-daemon/src/cli.rs
  - CLAUDE.md
  - crates/emfor-daemon/src/main.rs
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/Cargo.toml
  - .spectra.yaml
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-daemon/src/checker/mod.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - examples/emfor.toml
  - crates/emfor-format/Cargo.toml
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Clean shutdown marks the store stale

On `SIGTERM` or `SIGINT` the daemon SHALL stop all checks, set `meta/heartbeat` to `0`, close the store, and exit with status 0.

#### Scenario: Heartbeat zero after shutdown

- **WHEN** the running daemon receives `SIGTERM`
- **THEN** the process exits 0 and `meta/heartbeat` reads `0`


<!-- @trace
source: emfor-core
updated: 2026-11-12
code:
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/writer.rs
  - Cargo.toml
  - crates/emfor-daemon/src/cli.rs
  - CLAUDE.md
  - crates/emfor-daemon/src/main.rs
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/Cargo.toml
  - .spectra.yaml
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-daemon/src/checker/mod.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - examples/emfor.toml
  - crates/emfor-format/Cargo.toml
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Single writer per store

Before opening the store the daemon SHALL take an exclusive lock on the file `<path>.writer`. If the lock is already held, the daemon SHALL exit with status 1 and an error message naming that lock file.

#### Scenario: Second daemon refused

- **WHEN** a daemon is running with `[store] path = "/tmp/t/state.mdb"` and a second daemon starts with the same path
- **THEN** the second daemon exits 1 with an error naming `/tmp/t/state.mdb.writer`, and the first keeps running


<!-- @trace
source: emfor-core
updated: 2026-11-12
code:
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/writer.rs
  - Cargo.toml
  - crates/emfor-daemon/src/cli.rs
  - CLAUDE.md
  - crates/emfor-daemon/src/main.rs
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/Cargo.toml
  - .spectra.yaml
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-daemon/src/checker/mod.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - examples/emfor.toml
  - crates/emfor-format/Cargo.toml
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Write failures are surfaced, not fatal

When a store commit fails, the daemon SHALL log the error at `error` level, keep running checks, and retry the pending writes at the next heartbeat tick.

#### Scenario: Map full

- **WHEN** a commit fails with an LMDB map-full error
- **THEN** the daemon logs an `error` naming the failure and continues running

<!-- @trace
source: emfor-core
updated: 2026-11-12
code:
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/writer.rs
  - Cargo.toml
  - crates/emfor-daemon/src/cli.rs
  - CLAUDE.md
  - crates/emfor-daemon/src/main.rs
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/Cargo.toml
  - .spectra.yaml
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-daemon/src/checker/mod.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - examples/emfor.toml
  - crates/emfor-format/Cargo.toml
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->