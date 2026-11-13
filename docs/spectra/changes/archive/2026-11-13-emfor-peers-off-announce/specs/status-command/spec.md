## MODIFIED Requirements

### Requirement: Status text output

`emfor status` SHALL print, in this order:

1. `store: <path>`;
2. one daemon line: `daemon: running, heartbeat <age> ago, up <duration>` followed by `, site <site>` when `meta/site` is set, then `, dns <addresses>` when `meta/dns_listen` holds a JSON array of strings, and then `, metrics <addresses>` when `meta/metrics_listen` does, the addresses of each separated by single spaces in stored order; `daemon: stale, heartbeat <age> ago` when the heartbeat is older than 3 seconds; `daemon: stopped` when the heartbeat is `0`;
3. when the daemon is running and `meta/peers` holds a JSON object of the shape defined in spec state-store, a line `peers: <live>/<members> live`, where `<members>` is the number of entries in `members` and `<live>` the number of them with `live` true, followed by `, not live <addresses>` with the addresses of the members that are not live separated by single spaces in stored order when there are any; followed by `, checks <n>/<m> backends here` when at least one shown backend is voted (see below), where `<m>` is the number of shown backends and `<n>` the number of them that are not voted; then, for each shown service with entries in `digest_mismatch`, a line `peers: check definition of <service> differs at <members>` with that service's members separated by single spaces in stored order;
4. for each service in name order, a line `<name>  <balance>  on_all_down=<open|closed>` followed by `  ALL DOWN` when `all_down` is true;
5. under each service, one line per backend in store order with the backend `id`, its health state (`healthy`, `unhealthy` or `unknown`), the time since `checked_at` as `<age> ago` (`never` when absent), the check latency (`-` when absent), the marker `pool` for members of a `fastest` service's pool, the marker `local-up` when the daemon is running and `meta/peers` lists the backend in `disagreements`, the marker `voted` when the backend is voted, and the last failure `reason` when present.

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
