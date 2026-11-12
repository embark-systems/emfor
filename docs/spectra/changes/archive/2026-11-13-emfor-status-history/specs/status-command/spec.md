## ADDED Requirements

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

### Requirement: Status history output

`emfor status --history` SHALL print the `store:` and daemon lines as in the text output, then, for each service in name order, the service name on its own line and, for each backend in store order, the backend `id` indented by two spaces followed by its history entries newest first, each indented by four spaces as `<YYYY-MM-DD HH:MM:SS> <from> -> <to>` in local time, followed by two spaces and the `reason` when it is not null. A backend with no entries SHALL show the single line `no changes recorded` indented by four spaces. `--history` SHALL combine with `--service` and SHALL NOT combine with `--json` or `--watch`. Exit statuses and the rule on terminal escape sequences SHALL be the same as for text output.

#### Scenario: History of one service

- **WHEN** backend `10.0.0.12:8080` of `api.svc.emfor` turned healthy at 2026-11-13 14:02:50 and unhealthy at 14:03:12 with reason `connect: Connection refused`, backend `10.0.0.11:8080` has no history, and an operator runs `emfor status --history --service api.svc.emfor`
- **THEN** the output lists `10.0.0.11:8080` followed by `no changes recorded`, then `10.0.0.12:8080` followed by `2026-11-13 14:03:12 healthy -> unhealthy  connect: Connection refused` and then `2026-11-13 14:02:50 unknown -> healthy`

#### Scenario: History with watch refused

- **WHEN** an operator runs `emfor status --history --watch`
- **THEN** the command reports that the two options cannot be used together and exits 2

## MODIFIED Requirements

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
