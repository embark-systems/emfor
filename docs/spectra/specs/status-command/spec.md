# status-command Specification

## Purpose

Defines `emfor status`, the read-only command that shows operators what the local emfor daemon has published: whether the daemon is still updating the state store, and the health, last failure reason, check age and latency of every backend of every service. It reads the same store as the clients and never changes it.

## Requirements

### Requirement: Status reads the store read-only

`emfor status` SHALL read the state store at the `[store] path` of the config file given with `--config` (the build-time default config path when `--config` is absent), or `/var/lib/emfor/state.mdb` when the config file has no `[store] path`. It SHALL read only the `[store]` table of the config file and SHALL NOT validate the rest of the config, start plugins, run checks, or write to the store. It SHALL NOT take the daemon's writer lock, so a daemon can start or keep running while `emfor status` runs. When the store file does not exist, `emfor status` SHALL print `cannot open store <path>: no such file` and exit 2 without creating the store file or its `-lock` file. When `meta/schema_version` is not `1`, it SHALL print `store format version <n>; this emfor reads version 1` and exit 2. Running `emfor` without the `status` subcommand SHALL keep its existing behavior.

#### Scenario: Status beside a running daemon

- **WHEN** a daemon is running with the store at `/tmp/t/state.mdb` and an operator runs `emfor status --config <that config>`
- **THEN** the command prints the services and exits, and the daemon keeps writing heartbeats

#### Scenario: Store missing

- **WHEN** the config sets `[store] path = "/tmp/t/none.mdb"` and that file does not exist
- **THEN** `emfor status` prints `cannot open store /tmp/t/none.mdb: no such file`, exits 2, and neither `/tmp/t/none.mdb` nor `/tmp/t/none.mdb-lock` exists afterwards

#### Scenario: Rest of the config is not validated

- **WHEN** the config file has a valid `[store]` table and a service with an unknown key `intervall`
- **THEN** `emfor status` shows the store contents instead of a validation error


<!-- @trace
source: emfor-status
updated: 2026-11-13
code:
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/build/prefix.rs
  - crates/emfor-daemon/Cargo.toml
  - docs/deploy/daemon.md
  - crates/emfor-daemon/build.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/config.rs
  - examples/rc.d/emfor
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Status text output

`emfor status` SHALL print, in this order:

1. `store: <path>`;
2. one daemon line: `daemon: running, heartbeat <age> ago, up <duration>` followed by `, site <site>` when `meta/site` is set; `daemon: stale, heartbeat <age> ago` when the heartbeat is older than 3 seconds; `daemon: stopped` when the heartbeat is `0`;
3. for each service in name order, a line `<name>  <balance>  on_all_down=<open|closed>` followed by `  ALL DOWN` when `all_down` is true;
4. under each service, one line per backend in store order with the backend `id`, its health state (`healthy`, `unhealthy` or `unknown`), the time since `checked_at` as `<age> ago` (`never` when absent), the check latency (`-` when absent), the marker `pool` for members of a `fastest` service's pool, and the last failure `reason` when present.

Ages and durations SHALL be written as `<s>.<tenths>s` under one minute, `<m>m<ss>s` under one hour, `<h>h<mm>m` under one day, and `<d>d<hh>h` otherwise. Latency SHALL be written in milliseconds with one decimal (`1.2ms`), or in seconds with one decimal at 1000 ms and above (`1.4s`). When standard output is not a terminal or the environment variable `NO_COLOR` is set, the output SHALL contain no terminal escape sequences.

#### Scenario: One healthy and one failing backend

- **WHEN** the store holds `api.svc.emfor` (`round-robin`, `open`) with backend `10.0.0.11:8080` healthy, checked 800 ms ago, latency 1200 µs, and backend `10.0.0.12:8080` unhealthy, checked 900 ms ago, reason `connect: Connection refused`
- **THEN** the backend lines contain, in order, `10.0.0.11:8080`, `healthy`, `0.8s ago`, `1.2ms`, and `10.0.0.12:8080`, `unhealthy`, `0.9s ago`, `-`, `connect: Connection refused`

##### Example: age and latency formatting

| Value | Printed |
| --- | --- |
| age 800 ms | `0.8s` |
| age 61 s | `1m01s` |
| age 3 h 12 min | `3h12m` |
| age 2 days 3 h | `2d03h` |
| latency 350 µs | `0.4ms` |
| latency 1200 µs | `1.2ms` |
| latency 1 400 000 µs | `1.4s` |

#### Scenario: Stopped daemon

- **WHEN** the daemon shut down cleanly and the heartbeat is `0`
- **THEN** the daemon line is `daemon: stopped` and the services from the last run are still listed

#### Scenario: Service with no healthy backend

- **WHEN** `db.svc.emfor` has `all_down` true
- **THEN** its service line ends with `ALL DOWN`


<!-- @trace
source: emfor-status
updated: 2026-11-13
code:
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/build/prefix.rs
  - crates/emfor-daemon/Cargo.toml
  - docs/deploy/daemon.md
  - crates/emfor-daemon/build.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/config.rs
  - examples/rc.d/emfor
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Status filtering and JSON output

`emfor status --service <name>` SHALL show only that service; when the store has no such service it SHALL print `service <name> not found` and exit 2. `emfor status --json` SHALL print one JSON object instead of text: `store` (path), `daemon` (`running`, `stale` or `stopped`), `heartbeat_ms` (the stored value), `heartbeat_age_ms` (null when stopped), `started_at_ms`, `site` (null when absent), `services`, an array of the service records exactly as stored, in name order, `history`, an object mapping each listed service name that has a `hist/` key to that key's value exactly as stored, and `flapping`, an array of `{"service":<name>,"backend":<id>,"changes":<n>}` for every flapping backend of the listed services (see "Flapping backends"), in service name order and then store order. `--json` and `--service` SHALL combine, and `--service` SHALL limit `history` and `flapping` the same way as `services`. Exit statuses SHALL be the same as for text output.

#### Scenario: JSON for one service

- **WHEN** an operator runs `emfor status --json --service api.svc.emfor` while the daemon is running
- **THEN** the output parses as JSON with `daemon` equal to `running` and `services` holding one record whose `name` is `api.svc.emfor`

#### Scenario: Unknown service

- **WHEN** an operator runs `emfor status --service nope.svc.emfor`
- **THEN** the command prints `service nope.svc.emfor not found` and exits 2

#### Scenario: JSON includes history and flapping

- **WHEN** `hist/api.svc.emfor` holds 6 healthy↔unhealthy entries from the last 8 minutes for `10.0.0.12:8080` and an operator runs `emfor status --json`
- **THEN** `history["api.svc.emfor"]` equals the stored value and `flapping` contains `{"service":"api.svc.emfor","backend":"10.0.0.12:8080","changes":6}`

#### Scenario: JSON without history

- **WHEN** the store has no `hist/` keys and an operator runs `emfor status --json`
- **THEN** `history` is `{}` and `flapping` is `[]`


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
### Requirement: Status exit status

`emfor status` without `--watch` SHALL exit 2 when it cannot read the config file, cannot open or read the store, finds an unsupported format version, or does not find the service named by `--service`. Otherwise it SHALL exit 1 when the daemon is stale or stopped, or when any shown service has `all_down` true, and exit 0 in every other case. An unhealthy or unknown backend in a service that still has a healthy backend SHALL NOT by itself make the exit status non-zero.

#### Scenario: Exit statuses

- **WHEN** an operator runs `emfor status` in each state below
- **THEN** it exits with the listed status

##### Example: exit status table

| Daemon | Services | Exit |
| --- | --- | --- |
| running | every service has a healthy backend, one backend unhealthy | 0 |
| running | `db.svc.emfor` has `all_down` true | 1 |
| running, `--service api.svc.emfor` | only `db.svc.emfor` has `all_down` true | 0 |
| stale (heartbeat 5 s old) | all healthy | 1 |
| stopped (heartbeat 0) | all healthy | 1 |
| store file missing | — | 2 |


<!-- @trace
source: emfor-status
updated: 2026-11-13
code:
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/build/prefix.rs
  - crates/emfor-daemon/Cargo.toml
  - docs/deploy/daemon.md
  - crates/emfor-daemon/build.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/config.rs
  - examples/rc.d/emfor
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Status watch mode

`emfor status --watch` SHALL redraw the text output every second until interrupted, then restore the terminal and exit 0. A backend row whose health state or reason changed since the previous frame, and a service line whose `all_down` changed, SHALL be highlighted for one frame; ages and latency changing alone SHALL NOT highlight a row. Below the services it SHALL list the most recent 10 changes, newest first, taken together from the stored history of the listed services (see "Backend state history") and from the daemon state changes seen during this run. A backend change SHALL be shown as `<HH:MM:SS> <service> <backend id> <old state> -> <new state>` using its stored `at`, and a daemon change as `<HH:MM:SS> daemon <old> -> <new>`; a change from before the current local day SHALL be prefixed with `<YYYY-MM-DD> `. The list SHALL therefore show changes that happened before `--watch` started. A failed read SHALL be shown as an error line in place of the services and retried on the next refresh instead of ending the command. When standard output is not a terminal, `emfor status --watch` SHALL print `--watch needs a terminal` and exit 2. `--watch` SHALL NOT combine with `--json`.

#### Scenario: Backend goes down while watching

- **WHEN** `emfor status --watch` is running and backend `10.0.0.12:8080` of `api.svc.emfor` turns from healthy to unhealthy at 14:03:12
- **THEN** the next frame highlights that backend's row and the change list starts with `14:03:12 api.svc.emfor 10.0.0.12:8080 healthy -> unhealthy`

#### Scenario: Earlier changes shown at start

- **WHEN** backend `10.0.0.12:8080` of `api.svc.emfor` turned unhealthy at 09:15:00 today and an operator starts `emfor status --watch` at 10:00:00
- **THEN** the first frame's change list contains `09:15:00 api.svc.emfor 10.0.0.12:8080 healthy -> unhealthy`

##### Example: merged change list

- **GIVEN** stored entries for `api.svc.emfor` backend `10.0.0.12:8080` at 2026-11-12 23:50:00 (`healthy -> unhealthy`) and 2026-11-13 09:15:00 (`unhealthy -> healthy`), and the daemon went from `running` to `stale` at 2026-11-13 10:00:05 while watching on 2026-11-13
- **WHEN** the frame is drawn
- **THEN** the change list is, in order, `10:00:05 daemon running -> stale`, `09:15:00 api.svc.emfor 10.0.0.12:8080 unhealthy -> healthy`, `2026-11-12 23:50:00 api.svc.emfor 10.0.0.12:8080 healthy -> unhealthy`

#### Scenario: Watch without a terminal

- **WHEN** an operator runs `emfor status --watch > out.txt`
- **THEN** the command prints `--watch needs a terminal` and exits 2


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
### Requirement: Flapping backends

`emfor status` SHALL count, for each backend, the entries of its stored history (see the state-store requirement "Backend state history") that go from `healthy` to `unhealthy` or from `unhealthy` to `healthy` and whose `at` is at most 600 000 ms before the current time. A backend with 5 or more such entries is flapping: its line in the text output SHALL carry the marker `FLAPPING (<n> changes in 10m)`, where `<n>` is the count, placed after the `pool` marker and before the `reason`. Transitions to or from `unknown` SHALL NOT count. Flapping SHALL NOT change the exit status and SHALL NOT change any daemon behavior. When the store has no history for a backend, it SHALL NOT be flapping.

#### Scenario: Backend marked as flapping

- **WHEN** backend `10.0.0.12:8080` of `api.svc.emfor` has 6 healthy↔unhealthy transitions in the last 8 minutes and is currently healthy
- **THEN** its line contains `FLAPPING (6 changes in 10m)` and the exit status is 0

##### Example: counting rule

| History entries (age, from -> to) | Marker |
| --- | --- |
| 5 entries aged 1–5 min, alternating healthy/unhealthy | `FLAPPING (5 changes in 10m)` |
| 4 entries aged 1–4 min, alternating healthy/unhealthy | none |
| `unknown -> healthy` at 1 min, then 4 alternating entries aged 2–5 min | none |
| 5 alternating entries aged 1, 2, 3, 4 and 10 min 1 s | none |
| 5 alternating entries aged 1, 2, 3, 4 and exactly 10 min | `FLAPPING (5 changes in 10m)` |

#### Scenario: Flapping does not change the exit status

- **WHEN** every backend of every service is unhealthy except one healthy backend that is flapping
- **THEN** the exit status is the same as without the flapping marker


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
### Requirement: Status history output

`emfor status --history` SHALL print the `store:` and daemon lines as in the text output, then, for each service in name order, the service name on its own line and, for each backend in store order, the backend `id` indented by two spaces followed by its history entries newest first, each indented by four spaces as `<YYYY-MM-DD HH:MM:SS> <from> -> <to>` in local time, followed by two spaces and the `reason` when it is not null. A backend with no entries SHALL show the single line `no changes recorded` indented by four spaces. `--history` SHALL combine with `--service` and SHALL NOT combine with `--json` or `--watch`. Exit statuses and the rule on terminal escape sequences SHALL be the same as for text output.

#### Scenario: History of one service

- **WHEN** backend `10.0.0.12:8080` of `api.svc.emfor` turned healthy at 2026-11-13 14:02:50 and unhealthy at 14:03:12 with reason `connect: Connection refused`, backend `10.0.0.11:8080` has no history, and an operator runs `emfor status --history --service api.svc.emfor`
- **THEN** the output lists `10.0.0.11:8080` followed by `no changes recorded`, then `10.0.0.12:8080` followed by `2026-11-13 14:03:12 healthy -> unhealthy  connect: Connection refused` and then `2026-11-13 14:02:50 unknown -> healthy`

#### Scenario: History with watch refused

- **WHEN** an operator runs `emfor status --history --watch`
- **THEN** the command reports that the two options cannot be used together and exits 2

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