# health-checking Specification

## Purpose

Defines how emfor decides whether each backend is healthy: the checker interface and its three result kinds, the built-in `tcp` and `http` checkers, how checks are scheduled, and the `rise` / `fall` state machine that turns check results into a health state.

## Requirements

### Requirement: Check results are up, down, or error

Every check SHALL produce exactly one result: `up` (the backend passed), `down` with a reason (the backend failed), or `error` with a reason (emfor could not perform the check because of a local problem). A check that does not finish within the service's `timeout` SHALL be recorded as `down` with reason `timeout`.

#### Scenario: Timeout is a backend failure

- **WHEN** a backend accepts the TCP connection for an `http` check but sends no response within `timeout`
- **THEN** the check result is `down` with reason `timeout`


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
### Requirement: TCP checker

The `tcp` checker SHALL connect to the backend address. A completed connection SHALL be `up` and the connection SHALL be closed immediately. A refused or unreachable connection SHALL be `down`. A local failure to create the socket (such as too many open files) SHALL be `error`. The `tcp` checker SHALL accept no `check_options` keys.

#### Scenario: Listening backend

- **WHEN** a `tcp` check runs against a backend with a listening socket
- **THEN** the result is `up`

#### Scenario: Closed port

- **WHEN** a `tcp` check runs against a port with no listener
- **THEN** the result is `down` with a reason describing the refused connection

#### Scenario: Unexpected option

- **WHEN** a `tcp` service sets `check_options.path = "/"`
- **THEN** validation fails with an unknown key error for `check_options.path`


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
### Requirement: HTTP checker

The `http` checker SHALL send one `GET` request per check to the backend IP and port using `scheme` (`http` or `https`, default `http`) and `path` (default `/`), with the `Host` header set to `host_header` when configured. It SHALL NOT follow redirects. The result SHALL be `up` when the response status matches any entry of `expect_status` (default `["200-399"]`; each entry is a status code or an inclusive `lo-hi` range) and `down` otherwise, with the status code in the reason. For `https`, certificate verification SHALL be on unless `tls_verify = false`.

#### Scenario: Status inside expected range

- **WHEN** an `http` check with default options receives status 204
- **THEN** the result is `up`

##### Example: status matching

| `expect_status` | Response status | Result |
| --- | --- | --- |
| default `["200-399"]` | 200 | up |
| default `["200-399"]` | 302 | up (redirect not followed) |
| default `["200-399"]` | 503 | down, reason contains 503 |
| `["200", "204"]` | 204 | up |
| `["200-299"]` | 301 | down, reason contains 301 |

#### Scenario: Invalid expect_status

- **WHEN** a service sets `expect_status = ["299-200"]`
- **THEN** validation fails because the range lower bound exceeds the upper bound

#### Scenario: Host header

- **WHEN** a service sets `host_header = "api.internal"`
- **THEN** each check request carries `Host: api.internal`


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
### Requirement: Check scheduling with jitter and no overlap

The daemon SHALL check every backend of every service repeatedly. The first check of a backend SHALL start after a random delay in `[0, interval)`. Each following check SHALL start after the previous check finishes plus `interval` adjusted by a uniformly random offset within `±jitter` of `interval`. Two checks of the same backend SHALL NOT run at the same time.

#### Scenario: Spacing between checks

- **WHEN** a backend has `interval = "2s"` and `jitter = "10%"`
- **THEN** the gap between the end of one check and the start of the next is between 1.8s and 2.2s

#### Scenario: Zero jitter

- **WHEN** a backend has `interval = "1s"` and `jitter = "0%"`
- **THEN** the gap between the end of one check and the start of the next is 1s


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
### Requirement: Health state machine with rise and fall

Each backend SHALL have a health state of `unknown`, `healthy`, or `unhealthy`, a consecutive-success counter, and a consecutive-failure counter, all starting at `unknown`, 0, 0 when the daemon starts. An `up` result SHALL increment the success counter and reset the failure counter; reaching `rise` SHALL set the state to `healthy`. A `down` result SHALL increment the failure counter and reset the success counter; reaching `fall` SHALL set the state to `unhealthy`. An `error` result SHALL change neither counter nor the state, except that when a backend has received only `error` results for longer than `fall × interval`, its state SHALL become `unknown` and both counters SHALL reset to 0.

#### Scenario: Backend becomes healthy

- **WHEN** a backend in state `unknown` with `rise = 2` gets two `up` results in a row
- **THEN** its state is `healthy`

##### Example: state transitions with rise = 2, fall = 3

| Results in order | Final state |
| --- | --- |
| up | unknown |
| up, up | healthy |
| up, up, down, down | healthy |
| up, up, down, down, down | unhealthy |
| up, up, down, down, up, down, down | healthy (up reset the failure counter) |
| down, down, down | unhealthy |
| down, down, down, up, up | healthy |
| up, up, error, error | healthy |

#### Scenario: Persistent errors make the state unknown

- **WHEN** a `healthy` backend with `interval = "2s"` and `fall = 3` receives only `error` results for more than 6s
- **THEN** its state becomes `unknown` and both counters are 0


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
### Requirement: Logging of state changes

Every health state change SHALL be logged at `info` level with the service name, backend id, old state, new state, and the reason from the result that caused it.

#### Scenario: Transition is logged

- **WHEN** backend `10.0.0.11:8080` of `api.svc.emfor` changes from `healthy` to `unhealthy` after `down` results with reason `connection refused`
- **THEN** an `info` log line contains `api.svc.emfor`, `10.0.0.11:8080`, `healthy`, `unhealthy`, and `connection refused`

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