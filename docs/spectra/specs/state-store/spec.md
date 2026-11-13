# state-store Specification

## Purpose

Defines the local LMDB file through which emfor publishes health state to readers on the same host. The layout and record format are a versioned cross-language contract, so every client binding can read it directly and detect when the daemon is no longer running.

## Requirements

### Requirement: LMDB file layout

The daemon SHALL store state in an LMDB environment opened with `MDB_NOSUBDIR` at `[store] path` (default `/var/lib/emfor/state.mdb`), producing one data file and one lock file named `<path>-lock`. It SHALL use only the unnamed default database. Keys and values SHALL be UTF-8 strings. The data file SHALL be created with `[store] file_mode` (default `0644`) and the lock file with `[store] lock_file_mode` (default `0666`). The map size SHALL be `[store] map_size` (default `64MiB`). `[store] sync` SHALL be a boolean, default `false`: with `false` the environment SHALL be opened with `MDB_NOSYNC`, so a commit does not wait for the disk; with `true` it SHALL be opened without `MDB_NOSYNC`, so every commit is flushed to disk before it completes. Commits SHALL run outside the async worker threads, so a slow disk does not stall the DNS server, the metrics endpoint or the checks.

#### Scenario: Files created on first start

- **WHEN** the daemon starts with `path = "/tmp/t/state.mdb"` and no existing files
- **THEN** `/tmp/t/state.mdb` and `/tmp/t/state.mdb-lock` exist with modes 0644 and 0666

#### Scenario: Commits do not wait for the disk by default

- **WHEN** the daemon opens the store with no `sync` key in `[store]`
- **THEN** the environment flags include `MDB_NOSYNC`

##### Example: sync setting and environment flags

| `[store] sync` | `MDB_NOSYNC` set |
| --- | --- |
| absent | yes |
| `false` | yes |
| `true` | no |


<!-- @trace
source: emfor-store-resilience
updated: 2026-11-13
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/schedule.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/store.rs
  - examples/emfor.toml
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/writer.rs
  - crates/emfor-daemon/src/plugin/host.rs
  - crates/emfor-daemon/src/checker/mod.rs
tests:
  - crates/emfor-daemon/tests/support/test_plugin.rs
  - crates/emfor-daemon/tests/plugins.rs
  - crates/emfor-daemon/tests/daemon.rs
-->

---
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


<!-- @trace
source: emfor-balance-fastest
updated: 2026-11-12
code:
  - crates/emfor-format/src/lib.rs
  - crates/emfor-daemon/src/select.rs
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/dns.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/config.rs
  - examples/emfor.toml
  - docs/deploy/dns-resolver.md
  - crates/emfor-daemon/src/checker/mod.rs
  - clients/rust/src/client.rs
  - .spectra.yaml
  - tests/conformance/v1/cases.json
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/schedule.rs
  - clients/rust/README.md
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - tools/emfor-fixture/tests/conformance_data.rs
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

On startup the daemon SHALL write `meta/schema_version` and `meta/started_at`, delete every existing `svc/*` key, and write a record for every configured service with every backend `unknown`, `checked_at` null, and `reason` null, before running any check. Backend state from a previous run SHALL NOT be reused. The backend state history under `hist/*` is the one exception: it SHALL be kept and pruned as described in "Backend state history".

#### Scenario: Old state discarded

- **WHEN** the store contains `svc/old.svc.emfor` and `svc/api.svc.emfor` with healthy backends from a previous run, and the daemon starts with a config containing only `api.svc.emfor`
- **THEN** `svc/old.svc.emfor` is gone and every backend in `svc/api.svc.emfor` is `unknown`


<!-- @trace
source: emfor-status-history
updated: 2026-11-13
code:
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/writer.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/status.rs
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

On `SIGTERM` or `SIGINT` the daemon SHALL stop all checks, set `meta/heartbeat` to `0`, close the store, and exit with status 0. A signal that arrives during a reload SHALL abandon the reload, without applying it, instead of waiting for it to finish. A signal that arrives while the daemon is still loading its config at start SHALL end the start the same way: the daemon SHALL stop the plugins started so far and exit with status 0 without starting checks.

#### Scenario: Stop during startup

- **WHEN** the daemon receives `SIGTERM` while loading its config at start waits for a plugin's `validate` answer
- **THEN** the daemon exits 0 within about a second, without logging that it started, and no plugin process is left running

#### Scenario: Stop during a reload

- **WHEN** the daemon receives `SIGTERM` while a reload waits for a plugin's `validate` answer
- **THEN** the daemon exits 0 within about a second, the reload is not applied, and no plugin process is left running

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

---
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

<!-- @trace
source: emfor-balance-fastest
updated: 2026-11-12
code:
  - crates/emfor-format/src/lib.rs
  - crates/emfor-daemon/src/select.rs
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/dns.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/config.rs
  - examples/emfor.toml
  - docs/deploy/dns-resolver.md
  - crates/emfor-daemon/src/checker/mod.rs
  - clients/rust/src/client.rs
  - .spectra.yaml
  - tests/conformance/v1/cases.json
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/schedule.rs
  - clients/rust/README.md
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - crates/emfor-daemon/tests/daemon.rs
-->

---
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

<!-- @trace
source: emfor-status-history
updated: 2026-11-13
code:
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/writer.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/status.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Metrics listen addresses in the store

When the config has a `[metrics]` table, the daemon SHALL write `meta/metrics_listen` on startup, in the same transaction that writes `meta/started_at`: a JSON array of strings holding the `[metrics] listen` addresses in config order, IPv4 as `<ip>:<port>` and IPv6 as `[<ip>]:<port>`. When the config has no `[metrics]` table, the daemon SHALL delete `meta/metrics_listen` in that transaction. A reload SHALL NOT change the key, because `[metrics] listen` cannot change without a restart. Clients and the DNS server SHALL NOT read the key, and `meta/schema_version` SHALL stay `1`.

#### Scenario: Metrics table written at start

- **WHEN** the daemon starts with `[metrics] listen = ["127.0.0.1:5301", "[::1]:5301"]`
- **THEN** key `meta/metrics_listen` holds `["127.0.0.1:5301","[::1]:5301"]` once `meta/started_at` is written

#### Scenario: Key from an earlier run removed

- **WHEN** the store holds `meta/metrics_listen` from an earlier run and the daemon starts with a config that has no `[metrics]` table
- **THEN** key `meta/metrics_listen` does not exist once `meta/started_at` is written

<!-- @trace
source: emfor-status-metrics
updated: 2026-11-13
code:
  - crates/emfor-format/src/lib.rs
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/writer.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/cli.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: DNS listen addresses in the store

When the config has a `[dns]` table, the daemon SHALL write `meta/dns_listen` on startup, in the same transaction that writes `meta/started_at`: a JSON array of strings holding the `[dns] listen` addresses in config order, IPv4 as `<ip>:<port>` and IPv6 as `[<ip>]:<port>`. When the config has no `[dns]` table, the daemon SHALL delete `meta/dns_listen` in that transaction. A reload SHALL NOT change the key, because `[dns] listen` cannot change without a restart. Clients and the DNS server SHALL NOT read the key, and `meta/schema_version` SHALL stay `1`.

#### Scenario: DNS table written at start

- **WHEN** the daemon starts with `[dns] listen = ["127.0.0.1:5300", "[::1]:5300"]`
- **THEN** key `meta/dns_listen` holds `["127.0.0.1:5300","[::1]:5300"]` once `meta/started_at` is written

#### Scenario: DNS key from an earlier run removed

- **WHEN** the store holds `meta/dns_listen` from an earlier run and the daemon starts with a config that has no `[dns]` table
- **THEN** key `meta/dns_listen` does not exist once `meta/started_at` is written

<!-- @trace
source: emfor-status-dns
updated: 2026-11-13
code:
  - crates/emfor-daemon/src/writer.rs
  - crates/emfor-daemon/src/cli.rs
  - README.md
  - docs/deploy/daemon.md
  - README.zh-TW.md
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-format/src/lib.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Damaged store is replaced at start

When the daemon, while starting, opening the store or reading it before the first check fails with an LMDB error that means the file is damaged (`MDB_CORRUPTED`, `MDB_PAGE_NOTFOUND` or `MDB_INVALID`), it SHALL log a warning naming the store path and the error, rename the data file to `<path>.corrupt` (replacing an existing file of that name), delete `<path>-lock`, and create a new store at `<path>` as on a first start. Backend state history is lost with the damaged file. The writer lock (`<path>.writer`) SHALL be held throughout. Any other open error, and a damaged store found after start, SHALL keep their existing handling (exit 1 at start, a failed commit afterwards).

#### Scenario: Damaged store after a power loss

- **WHEN** `/tmp/t/state.mdb` exists but its meta pages are overwritten with zero bytes, and the daemon starts with `path = "/tmp/t/state.mdb"`
- **THEN** the daemon logs a warning containing `/tmp/t/state.mdb`, `/tmp/t/state.mdb.corrupt` holds the damaged file, and the new `/tmp/t/state.mdb` has a fresh heartbeat and a record for every configured service

#### Scenario: Another daemon's store is not replaced

- **WHEN** a daemon is running on `/tmp/t/state.mdb` and a second daemon starts with the same path
- **THEN** the second daemon exits 1 with the writer lock error and `/tmp/t/state.mdb.corrupt` does not exist

<!-- @trace
source: emfor-store-resilience
updated: 2026-11-13
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/schedule.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/store.rs
  - examples/emfor.toml
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/writer.rs
  - crates/emfor-daemon/src/plugin/host.rs
  - crates/emfor-daemon/src/checker/mod.rs
tests:
  - crates/emfor-daemon/tests/support/test_plugin.rs
  - crates/emfor-daemon/tests/plugins.rs
  - crates/emfor-daemon/tests/daemon.rs
-->