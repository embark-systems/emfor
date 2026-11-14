# status-command Specification

## Purpose

Defines `emfor status`, the read-only command that shows operators what the local emfor daemon has published: whether the daemon is still updating the state store, and the health, last failure reason, check age and latency of every backend of every service. It reads the same store as the clients and never changes it.

## Requirements

### Requirement: Status reads the store read-only

`emfor status` SHALL read the state store at the `[store] path` of the config file given with `--config` (the build-time default config path when `--config` is absent), or `/var/lib/emfor/state.mdb` when the config file has no `[store] path`. It SHALL read only the `[store]` table of the config file and SHALL NOT validate the rest of the config, start plugins, run checks, or write to the store. It SHALL NOT take the daemon's writer lock, so a daemon can start or keep running while `emfor status` runs. When the store file does not exist, `emfor status` SHALL print `cannot open store <path>: no such file` and exit 2 without creating the store file or its `-lock` file. It SHALL read a store whose `meta/schema_version` is `1` or `2`. When `meta/schema_version` is anything else, it SHALL print `store format version <n>; this emfor reads versions 1 and 2` and exit 2. The other subcommands of `emfor` and running it without a subcommand SHALL follow the service-config requirement "Command line subcommands".

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
source: emfor-backup-backends
updated: 2026-11-14
code:
  - docs/manual.zh_TW/05-backend-selection.md
  - clients/go/emfor.go
  - crates/emfor-daemon/src/status.rs
  - tests/conformance/v1/README.md
  - clients/python/emfor/__init__.py
  - crates/emfor-daemon/src/writer.rs
  - docs/manual.zh_TW/07-dns.md
  - examples/emfor.toml
  - tests/conformance/v1/cases.json
  - README.md
  - clients/rust/src/lib.rs
  - clients/perl/README.md
  - clients/rust/src/client.rs
  - crates/emfor-format/src/lib.rs
  - docs/manual.zh_TW/03-configuration.md
  - crates/emfor-daemon/src/state.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/select.rs
  - crates/emfor-daemon/src/config.rs
  - clients/python/README.md
  - clients/rust/README.md
  - docs/manual/06-clients.md
  - docs/manual/07-dns.md
  - CHANGELOG.md
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/dns.rs
  - docs/manual.zh_TW/06-clients.md
  - docs/manual/05-backend-selection.md
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/metrics.rs
  - crates/emfor-format/src/rules.rs
  - clients/go/README.md
  - clients/php/README.md
tests:
  - crates/emfor-daemon/tests/daemon.rs
  - clients/rust/tests/conformance_capi.rs
-->

---
### Requirement: Status text output

`emfor status` SHALL print, in this order:

1. `store: <path>`;
2. one daemon line: `daemon: running, heartbeat <age> ago, up <duration>` followed by `, site <site>` when `meta/site` is set, then `, dns <addresses>` when `meta/dns_listen` holds a JSON array of strings, and then `, metrics <addresses>` when `meta/metrics_listen` does, the addresses of each separated by single spaces in stored order; `daemon: stale, heartbeat <age> ago` when the heartbeat is older than 3 seconds; `daemon: stopped` when the heartbeat is `0`;
3. when the daemon is running and `meta/peers` holds a JSON object of the shape defined in spec state-store, a line `peers: <live>/<members> live`, where `<members>` is the number of entries in `members` and `<live>` the number of them with `live` true, followed by `, not live <addresses>` with the addresses of the members that are not live separated by single spaces in stored order when there are any; followed by `, checks <n>/<m> backends here` when at least one shown backend is voted (see below), where `<m>` is the number of shown backends and `<n>` the number of them that are not voted; then, for each member in stored order whose `services_off` (spec state-store) contains at least one shown service, a line `peers: <member> leaves out <services>` with those shown services in name order separated by single spaces; then, for each shown service with entries in `digest_mismatch`, a line `peers: check definition of <service> differs at <members>` with that service's members separated by single spaces in stored order;
4. for each service in name order, a line `<name>  <balance>  on_all_down=<open|closed>` (`unknown` for a value it does not know) followed by `  ALL DOWN` when `all_down` is true;
5. under each service, one line per backend in store order with the backend `id`, its health state (`healthy`, `unhealthy` or `unknown`, or `unrecognized`, colored like `unhealthy`, for a state written by a newer daemon), the time since `checked_at` as `<age> ago` (`never` when absent), the check latency (`-` when absent), the marker `pool` for members of a `fastest` service's pool, the marker `local-up` when the daemon is running and `meta/peers` lists the backend in `disagreements`, the marker `voted` when the backend is voted, and the last failure `reason` when present.

A backend SHALL be voted when the daemon is running, `meta/peers` has the shape defined in spec state-store with a `listen` and member addresses that are all valid `ip:port` strings, and the backend's assigned checkers, chosen as in spec peer-group "Assigned checkers and check results" with `listen` as this daemon and, as the live members for the backend's service, the members whose `live` is true and whose `services_off` does not contain that service, exist and do not include `listen`, and the backend's service is not in `services_off`. `services_off`, at the top level and in each member, SHALL count as empty when it is absent, so that a store written by a daemon without these fields still shows voted backends. The same choice of assigned checkers SHALL give the `checkers` of `assigned_checkers` in `--json` output (see "Status filtering and JSON output"). A voted backend is one this daemon does not check at every check time (spec peer-group, "Split checks").

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

#### Scenario: Metrics addresses on the daemon line

- **WHEN** the daemon is running with heartbeat 400 ms old, started 3 h 12 min ago, `meta/site` `tpe1` and `meta/metrics_listen` `["127.0.0.1:5301","[::1]:5301"]`
- **THEN** the daemon line is `daemon: running, heartbeat 0.4s ago, up 3h12m, site tpe1, metrics 127.0.0.1:5301 [::1]:5301`

##### Example: metrics part of the daemon line

| `meta/metrics_listen` | Daemon state | Daemon line ends with |
| --- | --- | --- |
| `["127.0.0.1:5301"]` | running | `, metrics 127.0.0.1:5301` |
| `["127.0.0.1:5301","[::1]:5301"]` | running | `, metrics 127.0.0.1:5301 [::1]:5301` |
| absent | running | `up <duration>` or `site <site>`, no `metrics` |
| `not json` | running | no `metrics` |
| `["127.0.0.1:5301"]` | stopped | `daemon: stopped` |

#### Scenario: DNS and metrics addresses on the daemon line

- **WHEN** the daemon is running with heartbeat 400 ms old, started 3 h 12 min ago, `meta/site` `tpe1`, `meta/dns_listen` `["127.0.0.1:5300"]` and `meta/metrics_listen` `["127.0.0.1:5301"]`
- **THEN** the daemon line is `daemon: running, heartbeat 0.4s ago, up 3h12m, site tpe1, dns 127.0.0.1:5300, metrics 127.0.0.1:5301`

##### Example: DNS part of the daemon line

| `meta/dns_listen` | Daemon state | Daemon line contains |
| --- | --- | --- |
| `["127.0.0.1:5300"]` | running | `, dns 127.0.0.1:5300` |
| `["127.0.0.1:5300","[::1]:5300"]` | running | `, dns 127.0.0.1:5300 [::1]:5300` |
| absent | running | no `dns` |
| `not json` | running | no `dns` |
| `["127.0.0.1:5300"]` | stopped | no `dns` (the line is `daemon: stopped`) |

#### Scenario: Stopped daemon

- **WHEN** the daemon shut down cleanly and the heartbeat is `0`
- **THEN** the daemon line is `daemon: stopped` and the services from the last run are still listed

#### Scenario: Service with no healthy backend

- **WHEN** `db.svc.emfor` has `all_down` true
- **THEN** its service line ends with `ALL DOWN`

#### Scenario: Peer group lines

- **WHEN** the daemon is running and `meta/peers` holds `{"listen":"10.0.0.5:5302","members":[{"addr":"10.0.0.6:5302","live":true},{"addr":"10.0.0.7:5302","live":false},{"addr":"10.0.0.8:5302","live":true}],"digest_mismatch":[{"service":"api.svc.emfor","member":"10.0.0.8:5302"}],"disagreements":[{"service":"api.svc.emfor","backend":"10.0.0.11:8080","down":2,"fresh":3}]}`
- **THEN** right after the daemon line the output has `peers: 2/3 live, not live 10.0.0.7:5302` and `peers: check definition of api.svc.emfor differs at 10.0.0.8:5302`, and the line of backend `10.0.0.11:8080` of `api.svc.emfor` contains `local-up`

##### Example: peers line

| `meta/peers` | Daemon state | Output |
| --- | --- | --- |
| 2 members, both live, no mismatch | running | `peers: 2/2 live` |
| 2 members, none live | running | `peers: 0/2 live, not live 10.0.0.6:5302 10.0.0.7:5302` |
| absent | running | no `peers:` line |
| `not json` | running | no `peers:` line |
| 2 members, both live | stopped | no `peers:` line and no `local-up` marker |

#### Scenario: Backends this daemon is not an assigned checker of

- **WHEN** the daemon is running, `meta/peers` holds `{"listen":"10.0.0.5:5302","members":[{"addr":"10.0.0.6:5302","live":true},{"addr":"10.0.0.7:5302","live":true},{"addr":"10.0.0.8:5302","live":true},{"addr":"10.0.0.9:5302","live":true}],"digest_mismatch":[],"disagreements":[]}`, and `api.svc.emfor` has the backends `10.0.0.11:8080` and `10.0.0.12:8080`
- **THEN** the output has `peers: 4/4 live, checks 1/2 backends here`, the line of `10.0.0.12:8080` contains `voted`, and the line of `10.0.0.11:8080` does not

##### Example: voted backends of api.svc.emfor at 10.0.0.5:5302

| Live members | `10.0.0.11:8080` | `10.0.0.12:8080` | `peers:` line ends with |
| --- | --- | --- | --- |
| .6, .7, .8, .9 | not voted (assigned .5, .8, .6) | voted (assigned .9, .6, .7) | `live, checks 1/2 backends here` |
| .6, .7, .9 (.8 not live) | not voted (assigned .5, .6, .7) | voted (assigned .9, .6, .7) | `not live 10.0.0.8:5302, checks 1/2 backends here` |
| .6 only | not voted (no assigned checkers) | not voted (no assigned checkers) | `not live 10.0.0.7:5302 10.0.0.8:5302 10.0.0.9:5302` |
| `listen` is `"bad"` | not voted | not voted | `live` (no `checks` part) |

#### Scenario: Service left out of the peer group is not voted

- **WHEN** `meta/peers` is as in "Backends this daemon is not an assigned checker of" with `"services_off":["api.svc.emfor"]` added, and `api.svc.emfor` is the only service
- **THEN** the output has `peers: 4/4 live` with no `checks` part, and no backend line contains `voted`

#### Scenario: Members that leave a service out are not candidates

- **WHEN** the daemon is running, `meta/peers` is as in "Backends this daemon is not an assigned checker of" except that the members `10.0.0.6:5302` and `10.0.0.9:5302` have `"services_off":["api.svc.emfor"]`, and `api.svc.emfor` is the only service
- **THEN** the output has `peers: 4/4 live` with no `checks` part, no backend line contains `voted`, and `emfor status --json` gives `assigned_checkers` equal to `[{"service":"api.svc.emfor","backend":"10.0.0.11:8080","checkers":["10.0.0.5:5302","10.0.0.8:5302","10.0.0.7:5302"]},{"service":"api.svc.emfor","backend":"10.0.0.12:8080","checkers":["10.0.0.7:5302","10.0.0.8:5302","10.0.0.5:5302"]}]`

##### Example: members' services_off and voted backends of api.svc.emfor at 10.0.0.5:5302

All four other members are live.

| Members with `api.svc.emfor` in `services_off` | `10.0.0.11:8080` | `10.0.0.12:8080` | `peers:` line ends with |
| --- | --- | --- | --- |
| none | not voted (assigned .5, .8, .6) | voted (assigned .9, .6, .7) | `live, checks 1/2 backends here` |
| .9 | not voted (assigned .5, .8, .6) | voted (assigned .6, .7, .8) | `live, checks 1/2 backends here` |
| .6, .9 | not voted (assigned .5, .8, .7) | not voted (assigned .7, .8, .5) | `live` (no `checks` part) |
| .6, .7, .9 | not voted (no assigned checkers) | not voted (no assigned checkers) | `live` (no `checks` part) |

#### Scenario: Members that leave services out

- **WHEN** the daemon is running, the shown services are `api.svc.emfor` and `db.svc.emfor`, and `meta/peers` holds `{"listen":"10.0.0.5:5302","members":[{"addr":"10.0.0.6:5302","live":true,"services_off":["db.svc.emfor","api.svc.emfor"]},{"addr":"10.0.0.7:5302","live":true,"services_off":[]},{"addr":"10.0.0.8:5302","live":true,"services_off":["api.svc.emfor"]}],"digest_mismatch":[{"service":"api.svc.emfor","member":"10.0.0.7:5302"}],"disagreements":[]}`
- **THEN** right after the `peers: 3/3 live` line the output has `peers: 10.0.0.6:5302 leaves out api.svc.emfor db.svc.emfor` and then `peers: 10.0.0.8:5302 leaves out api.svc.emfor`, followed by `peers: check definition of api.svc.emfor differs at 10.0.0.7:5302`

##### Example: leaves-out lines for member 10.0.0.6:5302

| Member's `services_off` | Shown services | Daemon state | Line for 10.0.0.6:5302 |
| --- | --- | --- | --- |
| `["db.svc.emfor","api.svc.emfor"]` | all | running | `peers: 10.0.0.6:5302 leaves out api.svc.emfor db.svc.emfor` |
| `["db.svc.emfor","api.svc.emfor"]` | `--service api.svc.emfor` | running | `peers: 10.0.0.6:5302 leaves out api.svc.emfor` |
| `["db.svc.emfor"]` | `--service api.svc.emfor` | running | none |
| `[]` or absent | all | running | none |
| `["api.svc.emfor"]` | all | stopped | none (no `peers:` lines) |


<!-- @trace
source: emfor-status-peers-off
updated: 2026-11-14
code:
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/status.rs
  - docs/manual.zh_TW/08-operations.md
  - CHANGELOG.md
-->

---
### Requirement: Status filtering and JSON output

`emfor status --service <name>` SHALL show only that service; when the store has no such service it SHALL print `service <name> not found` and exit 2. `emfor status --json` SHALL print one JSON object instead of text: `store` (path), `daemon` (`running`, `stale` or `stopped`), `heartbeat_ms` (the stored value), `heartbeat_age_ms` (null when stopped), `started_at_ms`, `site` (null when absent), `dns_listen` (the array of strings stored in `meta/dns_listen`, null when the key is absent or does not hold a JSON array of strings), `metrics_listen` (the array of strings stored in `meta/metrics_listen`, null when the key is absent or does not hold a JSON array of strings), `services`, an array of the service records exactly as stored, in name order, `history`, an object mapping each listed service name that has a `hist/` key to that key's value exactly as stored, `peers`, the value of `meta/peers` parsed as JSON when it holds a JSON object and null otherwise, not limited by `--service`, `assigned_checkers`, null unless the daemon is running and `meta/peers` has the shape and valid addresses required for voted backends (see "Status text output"), and otherwise an array of `{"service":<name>,"backend":<id>,"checkers":[<listen>, ...]}` for every backend of the listed services, in service name order and then store order, `checkers` holding the backend's assigned checkers in rank order, or empty when the service is in `services_off`, each written in canonical `ip:port` form as `meta/peers` stores addresses (IPv6 as `[addr]:port`) (empty when it has none), and `flapping`, an array of `{"service":<name>,"backend":<id>,"changes":<n>}` for every flapping backend of the listed services (see "Flapping backends"), in service name order and then store order. `--json` and `--service` SHALL combine, and `--service` SHALL limit `history` and `flapping` the same way as `services`. Exit statuses SHALL be the same as for text output.

#### Scenario: JSON for one service

- **WHEN** an operator runs `emfor status --json --service api.svc.emfor` while the daemon is running
- **THEN** the output parses as JSON with `daemon` equal to `running` and `services` holding one record whose `name` is `api.svc.emfor`

#### Scenario: JSON metrics addresses

- **WHEN** `meta/metrics_listen` holds `["127.0.0.1:5301"]` and an operator runs `emfor status --json`
- **THEN** `metrics_listen` equals `["127.0.0.1:5301"]`, and with the key absent it is `null`

#### Scenario: JSON DNS addresses

- **WHEN** `meta/dns_listen` holds `["127.0.0.1:5300"]` and an operator runs `emfor status --json`
- **THEN** `dns_listen` equals `["127.0.0.1:5300"]`, and with the key absent it is `null`

#### Scenario: Unknown service

- **WHEN** an operator runs `emfor status --service nope.svc.emfor`
- **THEN** the command prints `service nope.svc.emfor not found` and exits 2

#### Scenario: JSON includes history and flapping

- **WHEN** `hist/api.svc.emfor` holds 6 healthy↔unhealthy entries from the last 8 minutes for `10.0.0.12:8080` and an operator runs `emfor status --json`
- **THEN** `history["api.svc.emfor"]` equals the stored value and `flapping` contains `{"service":"api.svc.emfor","backend":"10.0.0.12:8080","changes":6}`

#### Scenario: JSON without history

- **WHEN** the store has no `hist/` keys and an operator runs `emfor status --json`
- **THEN** `history` is `{}` and `flapping` is `[]`

#### Scenario: JSON peer state

- **WHEN** `meta/peers` holds `{"listen":"127.0.0.1:5302","members":[{"addr":"127.0.0.1:5303","live":true}],"digest_mismatch":[],"disagreements":[]}` and an operator runs `emfor status --json --service api.svc.emfor`
- **THEN** `peers` equals that object, and with the key absent it is `null`

#### Scenario: JSON assigned checkers

- **WHEN** `meta/peers` is as in "Backends this daemon is not an assigned checker of" and an operator runs `emfor status --json --service api.svc.emfor` while the daemon is running
- **THEN** `assigned_checkers` equals `[{"service":"api.svc.emfor","backend":"10.0.0.11:8080","checkers":["10.0.0.5:5302","10.0.0.8:5302","10.0.0.6:5302"]},{"service":"api.svc.emfor","backend":"10.0.0.12:8080","checkers":["10.0.0.9:5302","10.0.0.6:5302","10.0.0.7:5302"]}]`, and with `meta/peers` absent or the daemon stopped it is `null`

#### Scenario: JSON assigned checkers of a service left out

- **WHEN** `meta/peers` is as in "Backends this daemon is not an assigned checker of" with `"services_off":["api.svc.emfor"]` added, and an operator runs `emfor status --json --service api.svc.emfor` while the daemon is running
- **THEN** `assigned_checkers` equals `[{"service":"api.svc.emfor","backend":"10.0.0.11:8080","checkers":[]},{"service":"api.svc.emfor","backend":"10.0.0.12:8080","checkers":[]}]`


<!-- @trace
source: emfor-peers-service-off
updated: 2026-11-13
code:
  - crates/emfor-daemon/src/writer.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/quorum.rs
  - crates/emfor-daemon/src/peers/
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/config.rs
  - examples/emfor.toml
  - CHANGELOG.md
  - docs/manual/01-concepts.md
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Status exit status

`emfor status` without `--watch` SHALL exit 2 when it cannot read the config file, cannot open or read the store, finds an unsupported format version, or does not find the service named by `--service`. Otherwise it SHALL exit 1 when the daemon is stale or stopped, or when any shown service has `all_down` true, and exit 0 in every other case. An unhealthy or unknown backend in a service that still has a healthy backend SHALL NOT by itself make the exit status non-zero. When standard output is closed before the output is written (a broken pipe, as in `emfor status | head -1`), the exit status SHALL be the one the output describes; any other error writing the output SHALL be printed to standard error and exit 2.

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
  - crates/emfor-daemon/Cargo.toml
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/config.rs
  - examples/freebsd/files/emfor.in
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Status watch mode

`emfor status --watch` SHALL redraw the text output every second until the operator presses `q`, `Q` or Ctrl-C, or it receives `SIGINT`, `SIGTERM` or `SIGHUP`, then restore the terminal (main screen, cursor shown, line wrapping on, raw mode off) and exit 0. The terminal SHALL also be restored when the command ends because of a panic. A backend row whose health state or reason changed since the previous frame, and a service line whose `all_down` changed, SHALL be highlighted for one frame; ages and latency changing alone SHALL NOT highlight a row. Below the services it SHALL list the most recent 10 changes, newest first, taken together from the stored history of the listed services (see "Backend state history") and from the daemon state changes seen during this run. A backend change SHALL be shown as `<HH:MM:SS> <service> <backend id> <old state> -> <new state>` using its stored `at`, and a daemon change as `<HH:MM:SS> daemon <old> -> <new>`; a change from before the current local day SHALL be prefixed with `<YYYY-MM-DD> `. The list SHALL therefore show changes that happened before `--watch` started. A failed read SHALL be shown as an error line in place of the services and retried on the next refresh instead of ending the command. When the store file at the configured path is no longer the file that was opened (it was replaced or removed), the next refresh SHALL open the path again instead of reading the old file. When standard output is not a terminal, `emfor status --watch` SHALL print `--watch needs a terminal` and exit 2. `--watch` SHALL NOT combine with `--json`.

The `store:` line and the daemon line SHALL stay at the top of the screen; everything after them (the services, the blank line, `recent changes:` and the changes) is the scrollable content. Each line SHALL take exactly one screen row: lines wider than the terminal are cut at the right edge instead of wrapping. When the top lines and the content fit in the terminal height, the screen SHALL show them without a footer. Otherwise the last row SHALL be the footer `-- lines <first>-<last> of <total>  PgUp/PgDn Up/Down Home/End  q quit --`, where `<total>` is the number of content lines and `<first>`-`<last>` the 1-based range shown, and the content area SHALL be the rows between the top lines and the footer (at least one row). PageDown and the space bar SHALL scroll down by the height of the content area, PageUp up by the same amount, the down and up arrow keys by one line, Home to the first content line and End so that the last content line is shown. The scroll position SHALL be kept across refreshes and SHALL be limited after every refresh, key and terminal resize so that the content area never starts before the first line or shows rows past the last line while more lines are above. A terminal resize SHALL redraw at the new size without waiting for the next refresh. Other keys SHALL be ignored.

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

#### Scenario: Store replaced while watching

- **WHEN** `emfor status --watch` is running and the daemon replaces a damaged store file with a new one at start
- **THEN** the next frame shows the contents of the new file

#### Scenario: Watch without a terminal

- **WHEN** an operator runs `emfor status --watch > out.txt`
- **THEN** the command prints `--watch needs a terminal` and exits 2

#### Scenario: Output fits the terminal

- **WHEN** the terminal has 24 rows and the top lines and content take 15 lines
- **THEN** all 15 lines are shown from the top and no footer is shown

#### Scenario: Scrolling a long list

- **WHEN** the terminal has 24 rows, the two top lines are shown, and the content has 60 lines
- **THEN** the content area is 21 rows, the first frame shows content lines 1-21 with the footer `-- lines 1-21 of 60  PgUp/PgDn Up/Down Home/End  q quit --`, and the `store:` and daemon lines stay on the first two rows after every key

##### Example: keys from the first frame

| Keys pressed | Footer range |
| --- | --- |
| none | `1-21` |
| PageDown | `22-42` |
| PageDown, PageDown | `40-60` |
| PageDown, PageUp | `1-21` |
| Down | `2-22` |
| Up | `1-21` |
| End | `40-60` |
| End, Home | `1-21` |
| End, Down | `40-60` |

#### Scenario: Position kept across refreshes and limited when the list shrinks

- **WHEN** the terminal has 24 rows, the content has 60 lines and the operator pressed PageDown once, and on the next refresh services were removed so the content has 30 lines
- **THEN** before the removal each refresh keeps the footer range at `22-42`, and after it the footer range is `10-30`

#### Scenario: Ending with a key

- **WHEN** `emfor status --watch` is running and the operator presses `q`, or Ctrl-C
- **THEN** the command returns to the main screen with the cursor shown and the terminal echoing typed characters again, and exits 0

#### Scenario: Ending with a signal

- **WHEN** `emfor status --watch` is running and receives `SIGTERM`
- **THEN** the command restores the terminal the same way and exits 0

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