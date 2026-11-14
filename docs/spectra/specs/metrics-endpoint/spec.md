# metrics-endpoint Specification

## Purpose

The daemon can serve its backend health state and check counters over HTTP in the Prometheus text format, so existing monitoring systems can scrape every host, alert on failures, and graph failure and transition rates over time.

## Requirements

### Requirement: Metrics endpoint enabled by the metrics table

The daemon SHALL serve metrics only when the config has a `[metrics]` table. It SHALL listen on TCP on every address in `[metrics] listen` (default `["127.0.0.1:5301"]`). If any address cannot be bound at startup, the daemon SHALL exit with status 1 and an error naming that address. Without a `[metrics]` table the daemon SHALL open no metrics socket. The metrics server SHALL start before the daemon logs `emfor started`, and SHALL log `metrics on <addresses>` at `info` level once bound.

#### Scenario: No metrics table

- **WHEN** the config has no `[metrics]` table
- **THEN** the daemon opens no metrics socket and logs no `metrics on` line

#### Scenario: Default listen address

- **WHEN** the config has an empty `[metrics]` table
- **THEN** `GET /metrics` on 127.0.0.1:5301 returns status 200

#### Scenario: Address in use

- **WHEN** another process already listens on TCP 127.0.0.1:5301 and the daemon starts with an empty `[metrics]` table
- **THEN** the daemon exits with status 1 and its error message contains `127.0.0.1:5301`


<!-- @trace
source: emfor-metrics
updated: 2026-11-13
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/config.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/dns.rs
  - crates/emfor-daemon/src/schedule.rs
  - crates/emfor-daemon/src/metrics.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/metrics.rs
-->

---
### Requirement: Metrics HTTP behavior

The metrics server SHALL speak HTTP/1.1 and close the connection after every response, sending `Connection: close` and a `Content-Length`. For a request line `GET /metrics HTTP/1.x` (with or without a query string) it SHALL answer status 200 with `Content-Type: text/plain; version=0.0.4; charset=utf-8` and the metrics body, except that until the daemon has applied its first service layout after start a `GET /metrics` SHALL get status 503 with no metrics, so a scrape never records a start time of 0 or a missing service list. Any other path SHALL get status 404; any other method on `/metrics` SHALL get status 405 with `Allow: GET`; a request whose head is not valid HTTP/1.x, or whose head (request line and headers) exceeds 8192 bytes, SHALL get status 400. A connection that has not sent a complete request head within 5 seconds SHALL be closed without a response. At most 16 connections SHALL be served at once; further connections SHALL be closed without a response until one finishes. The request body, if any, SHALL be ignored.

#### Scenario: Requests and statuses

- **WHEN** clients send requests to the metrics server
- **THEN** each gets the status for its request line

##### Example: request outcomes

| Request | Status |
| --- | --- |
| `GET /metrics HTTP/1.1` | 200 |
| `GET /metrics?x=1 HTTP/1.0` | 200 |
| `GET / HTTP/1.1` | 404 |
| `POST /metrics HTTP/1.1` | 405, `Allow: GET` |
| `HELLO` | 400 |
| a head of 9000 bytes | 400 |

#### Scenario: Not ready yet

- **WHEN** a client sends `GET /metrics HTTP/1.1` after the metrics server is bound but before the daemon has applied its first service layout
- **THEN** the response has status 503 and no `emfor_` series, and once the layout is applied the same request gets status 200

#### Scenario: Slow client

- **WHEN** a client connects and sends nothing for 5 seconds
- **THEN** the server closes the connection without a response and keeps answering other clients


<!-- @trace
source: emfor-metrics
updated: 2026-11-13
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/config.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/dns.rs
  - crates/emfor-daemon/src/schedule.rs
  - crates/emfor-daemon/src/metrics.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/metrics.rs
-->

---
### Requirement: Backend and service metrics

The body SHALL be the Prometheus text exposition format 0.0.4, with one `# HELP` and one `# TYPE` line before each metric family, reflecting the daemon's live state at the time of the request. Services SHALL appear in name order and backends in config order, labelled `service="<name>"` and `backend="<backend id>"`. Label values SHALL escape `\` as `\\`, `"` as `\"` and newline as `\n`. For each configured backend:

- `emfor_backend_state` (gauge) SHALL have three series with `state="unknown"`, `state="healthy"` and `state="unhealthy"`; the backend's current state has value 1 and the other two 0.
- `emfor_backend_check_latency_seconds` (gauge) SHALL be the backend's smoothed check latency in seconds, present only when the store record's `latency_us` is not null.
- `emfor_backend_last_check_timestamp_seconds` (gauge) SHALL be the Unix time in seconds, with millisecond precision, when the last check finished, present only after the first check.
- `emfor_backend_in_pool` (gauge) SHALL be 1 for members of a `fastest` service's pool and 0 for its other backends, and absent for backends of other services.

For each configured service, `emfor_service_all_down` (gauge) SHALL be 1 when no backend is healthy, otherwise 0. A service or backend removed by a reload SHALL not appear in any later response.

#### Scenario: One healthy and one failing backend

- **WHEN** `api.svc.emfor` has backend `10.0.0.11:8080` healthy with latency 1200 µs, last checked at 1762905600123, and backend `10.0.0.12:8080` unhealthy
- **THEN** the body contains `emfor_backend_state{service="api.svc.emfor",backend="10.0.0.11:8080",state="healthy"} 1`, `emfor_backend_state{service="api.svc.emfor",backend="10.0.0.12:8080",state="healthy"} 0`, `emfor_backend_check_latency_seconds{service="api.svc.emfor",backend="10.0.0.11:8080"} 0.0012`, `emfor_backend_last_check_timestamp_seconds{service="api.svc.emfor",backend="10.0.0.11:8080"} 1762905600.123` and `emfor_service_all_down{service="api.svc.emfor"} 0`, and no latency series for `10.0.0.12:8080`

#### Scenario: Removed service disappears

- **WHEN** service `old.svc.emfor` is removed from the config and the daemon reloads
- **THEN** the next response contains no series with `service="old.svc.emfor"`


<!-- @trace
source: emfor-metrics
updated: 2026-11-13
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/config.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/dns.rs
  - crates/emfor-daemon/src/schedule.rs
  - crates/emfor-daemon/src/metrics.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/metrics.rs
-->

---
### Requirement: Check and transition counters

For each configured backend the daemon SHALL count, from the time the backend first appears in the running daemon, every check result it applies, and every own check that a peer group leaves unapplied (spec peer-group "Region verdict and combined result"), as `emfor_backend_checks_total` (counter) with `result="up"`, `result="down"` or `result="error"`, and every health state transition as `emfor_backend_transitions_total` (counter) with `to="unknown"`, `to="healthy"` or `to="unhealthy"`. All three `result` series and all three `to` series SHALL be present from the start with value 0. Counters SHALL keep counting across reloads while the backend stays configured, including when the service's `check` or `check_options` change; a backend removed by a reload SHALL lose its counters, and if added again later it SHALL start from 0. Results from check tasks replaced by a reload SHALL NOT be counted.

#### Scenario: Counting results and transitions

- **WHEN** backend `10.0.0.11:8080` of `api.svc.emfor` with `rise = 2` passes two checks after start, then fails three checks with `fall = 3`
- **THEN** its `emfor_backend_checks_total` series are `up` 2, `down` 3, `error` 0, and its `emfor_backend_transitions_total` series are `healthy` 1, `unhealthy` 1, `unknown` 0

#### Scenario: Counters reset when a backend is removed and added back

- **WHEN** a backend with `emfor_backend_checks_total{result="up"}` 40 is removed by a reload and added back by a later reload
- **THEN** its `emfor_backend_checks_total{result="up"}` reads 0 right after the second reload


<!-- @trace
source: emfor-metrics
updated: 2026-11-13
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/config.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/dns.rs
  - crates/emfor-daemon/src/schedule.rs
  - crates/emfor-daemon/src/metrics.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/metrics.rs
-->

---
### Requirement: Daemon metrics

The body SHALL also contain `emfor_build_info{version="<emfor version>"} 1` (gauge), `emfor_start_time_seconds` (gauge) with the daemon start time in Unix seconds, and `emfor_store_commit_failures_total` (counter), the number of failed state store commits since the daemon started. After the first successful commit it SHALL contain `emfor_store_last_commit_timestamp_seconds` (gauge), the Unix time of the last successful commit, `emfor_store_used_bytes` (gauge), the bytes of the store up to its last used page, and `emfor_store_map_size_bytes` (gauge), the map size. For each plugin of the applied config it SHALL contain `emfor_plugin_up{plugin="<name>"}` (gauge), 1 while the plugin process runs and has completed the handshake and 0 otherwise, and `emfor_plugin_restarts_total{plugin="<name>"}` (counter), the number of times the plugin was started again after it exited. When `[peers]` is configured it SHALL contain `emfor_peer_up{peer="<ip:port>"}` (gauge) for every member other than this daemon, 1 while the member is live and 0 otherwise; `emfor_peer_messages_rejected_total{reason="<reason>"}` (counter) for each of the reasons `malformed`, `auth`, `sender`, `site`, `stale` and `replay`, starting at 0; and `emfor_peer_notifications_total{direction="sent"}` and `{direction="received"}` (counters), the `down` messages sent (one per receiving member) and the `down` messages accepted. With `[peers]` it SHALL also contain `emfor_peer_results_total{direction="sent"}` and `{direction="received"}` (counters), the `result` messages sent (one per receiving member) and the `result` messages accepted, `emfor_peer_disagreements` (gauge), the number of backends that are currently a disagreement (spec peer-group), and `emfor_peer_checks_skipped_total` (counter), the number of scheduled checks this daemon did not make because the votes of the backend's assigned checkers decided the result (spec peer-group, "Split checks"), starting at 0. Without `[peers]` it SHALL contain no series whose name starts with `emfor_peer_`.

#### Scenario: Plugin restart counted

- **WHEN** a plugin exits during a check and is started again
- **THEN** `emfor_plugin_restarts_total{plugin="<name>"}` increases by 1

#### Scenario: Store commit failure counted

- **WHEN** one commit to the state store fails
- **THEN** `emfor_store_commit_failures_total` increases by 1

#### Scenario: Peer metrics

- **WHEN** a daemon with `members = ["127.0.0.1:5302", "127.0.0.1:5303"]` and `listen = "127.0.0.1:5302"` has accepted a heartbeat from 127.0.0.1:5303 within the last 3 seconds and has dropped one message with a wrong tag
- **THEN** the body contains `emfor_peer_up{peer="127.0.0.1:5303"} 1` and `emfor_peer_messages_rejected_total{reason="auth"} 1`

#### Scenario: Peer result metrics

- **WHEN** a daemon in a peer group of three is an assigned checker of one backend, has made 4 `up` checks of it, and has one disagreement
- **THEN** the body contains `emfor_peer_results_total{direction="sent"} 8` and `emfor_peer_disagreements 1`

#### Scenario: Skipped checks counted

- **WHEN** a daemon in a peer group of four is not an assigned checker of its only backend, every assigned checker's fresh vote is `up`, and 10 scheduled check times have passed starting with an own check
- **THEN** the body contains `emfor_peer_checks_skipped_total 8`

<!-- @trace
source: emfor-peers-split-checks
updated: 2026-11-13
code:
  - crates/emfor-daemon/src/schedule.rs
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/peers/
  - docs/manual/01-concepts.md
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/quorum.rs
  - crates/emfor-daemon/src/metrics.rs
  - CHANGELOG.md
  - docs/manual/03-configuration.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->