## MODIFIED Requirements

### Requirement: Status text output

`emfor status` SHALL print, in this order:

1. `store: <path>`;
2. one daemon line: `daemon: running, heartbeat <age> ago, up <duration>` followed by `, site <site>` when `meta/site` is set, then `, dns <addresses>` when `meta/dns_listen` holds a JSON array of strings, and then `, metrics <addresses>` when `meta/metrics_listen` does, the addresses of each separated by single spaces in stored order; `daemon: stale, heartbeat <age> ago` when the heartbeat is older than 3 seconds; `daemon: stopped` when the heartbeat is `0`;
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

### Requirement: Status filtering and JSON output

`emfor status --service <name>` SHALL show only that service; when the store has no such service it SHALL print `service <name> not found` and exit 2. `emfor status --json` SHALL print one JSON object instead of text: `store` (path), `daemon` (`running`, `stale` or `stopped`), `heartbeat_ms` (the stored value), `heartbeat_age_ms` (null when stopped), `started_at_ms`, `site` (null when absent), `dns_listen` (the array of strings stored in `meta/dns_listen`, null when the key is absent or does not hold a JSON array of strings), `metrics_listen` (the array of strings stored in `meta/metrics_listen`, null when the key is absent or does not hold a JSON array of strings), `services`, an array of the service records exactly as stored, in name order, `history`, an object mapping each listed service name that has a `hist/` key to that key's value exactly as stored, and `flapping`, an array of `{"service":<name>,"backend":<id>,"changes":<n>}` for every flapping backend of the listed services (see "Flapping backends"), in service name order and then store order. `--json` and `--service` SHALL combine, and `--service` SHALL limit `history` and `flapping` the same way as `services`. Exit statuses SHALL be the same as for text output.

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

