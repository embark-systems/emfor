# health-checking Specification

## Purpose

Defines how emfor decides whether each backend is healthy: the checker interface and its three result kinds, the built-in `tcp` and `http` checkers, how checks are scheduled, and the `rise` / `fall` state machine that turns check results into a health state.

## Requirements

### Requirement: Check results are up, down, or error

Every check SHALL produce exactly one result: `up` (the backend passed), `down` with a reason (the backend failed), or `error` with a reason (emfor could not perform the check because of a local problem). A check that does not finish within the service's `timeout` SHALL be recorded as `down` with reason `timeout`, except for checks performed by an external plugin, which SHALL be recorded as `error` with reason `plugin did not answer within timeout`.

#### Scenario: Timeout is a backend failure

- **WHEN** a backend accepts the TCP connection for an `http` check but sends no response within `timeout`
- **THEN** the check result is `down` with reason `timeout`

#### Scenario: Plugin timeout is not a backend failure

- **WHEN** a service uses a plugin that receives the `check` request but never answers, and the service's `timeout` passes
- **THEN** the check result is `error` with reason `plugin did not answer within timeout`

---
### Requirement: TCP checker

The `tcp` checker SHALL connect to the backend address. A refused or unreachable connection SHALL be `down`. A local failure to create the socket (such as too many open files) SHALL be `error`.

The `tcp` checker SHALL accept these optional `check_options` keys and no others:

- `send`: a string whose UTF-8 bytes the checker SHALL write to the connection once, right after connecting (after the TLS handshake when `tls = true`), without any further unescaping.
- `expect`: a string whose UTF-8 bytes SHALL appear, byte for byte and case-sensitively, somewhere in the bytes the backend sends.
- `tls`: a boolean, default `false`. With `true` the checker SHALL perform a TLS handshake right after connecting and SHALL NOT fall back to plaintext.
- `tls_verify`: a boolean, default `true`. With `false` the checker SHALL accept any server certificate and still encrypt.
- `server_name`: a host name or IP address, default the backend IP. The server certificate SHALL match it, and it SHALL be sent as SNI when it is a host name.
- `ca_file`: an absolute path of a PEM file. The server certificate SHALL be verified against the certificates in it instead of the system's CA certificates.

Validation SHALL fail when `send` or `expect` is not a string, when `send` is set without `expect` (error on `expect`: `required when send is set`), when `expect` is empty, or when `expect` is longer than 16384 bytes; when `tls` or `tls_verify` is not a boolean (`must be a boolean`); when `server_name` is not a valid host name or IP address (`not a valid host name or IP address`); when `ca_file` is not an absolute path (`not an absolute path`), does not exist (`file not found`) or holds no PEM certificate (`no certificate found`); and when `tls_verify`, `server_name` or `ca_file` is set without `tls = true` (`requires tls = true`). An empty `send` with an `expect` SHALL behave as `expect` alone.

With `tls = true`, a failed TLS handshake or certificate verification SHALL be `down` with a reason starting `tls: `. The handshake SHALL get at most 90% of the time left before the service's `timeout`; when the backend has not completed it by then, the result SHALL be `down` with reason `tls: no answer to the TLS handshake (does the port speak TLS?)`. When less than 100 ms would be left for the handshake, the service's `timeout` SHALL apply instead. When `ca_file` cannot be read during a check, or the host has no system CA certificates and `ca_file` is not set, the result SHALL be `error` with a reason starting `ca_file: ` or `tls: ` respectively.

Without `expect`, a completed connection (with `tls = true`, a completed handshake) SHALL be `up` and the connection SHALL be closed immediately (with `tls = true`, after sending a TLS close_notify). With `expect`, the checker SHALL write `send` when set, then read from the connection and keep the received bytes, up to 16384 bytes. It SHALL answer `up` and close the connection as soon as the received bytes contain `expect`, including when `expect` spans several reads. It SHALL answer `down` with reason `expect: got "<excerpt>"` when the backend closes the connection, or 16384 bytes have been received, without `expect` appearing. The excerpt SHALL be the first 64 received bytes, escaped so that printable ASCII other than `\`, `'` and `"` stays as is; tab, carriage return and line feed become `\t`, `\r` and `\n`; `\`, `'` and `"` are preceded by `\`; and every other byte becomes `\x` followed by two lowercase hex digits. A failure while writing SHALL be `down` with a reason starting `send: `; a failure while reading, other than the backend closing the connection, SHALL be `down` with a reason starting `expect: `. A check that has not finished within the service's `timeout` SHALL be `down` with reason `timeout`, as for every checker.

#### Scenario: Listening backend

- **WHEN** a `tcp` check without `check_options` runs against a backend with a listening socket
- **THEN** the result is `up`

#### Scenario: Closed port

- **WHEN** a `tcp` check runs against a port with no listener
- **THEN** the result is `down` with a reason describing the refused connection

#### Scenario: Unexpected option

- **WHEN** a `tcp` service sets `check_options.path = "/"`
- **THEN** validation fails with an unknown key error for `check_options.path`

#### Scenario: Banner matches

- **WHEN** a `tcp` service sets only `expect` and the backend sends a greeting containing it
- **THEN** the checker sends nothing and the result is `up`

##### Example: SMTP greeting

- **GIVEN** `check_options = { expect = "220" }`
- **WHEN** the backend sends `220 mail.example ESMTP\r\n` after accepting the connection
- **THEN** the result is `up`

#### Scenario: Request and response match

- **WHEN** a `tcp` service sets `send` and `expect` and the backend answers the request with a response containing `expect`
- **THEN** the backend received exactly the bytes of `send` and the result is `up`

##### Example: memcached stats

- **GIVEN** `check_options = { send = "stats\r\n", expect = "STAT pid" }`
- **WHEN** the backend receives `stats\r\n` (the 7 bytes `73 74 61 74 73 0d 0a`) and answers `STAT pid 4242\r\n`
- **THEN** the result is `up`

#### Scenario: Expected text split across reads

- **WHEN** the backend sends the expected text in two separate writes that arrive in separate reads
- **THEN** the result is `up`

##### Example: split greeting

- **GIVEN** `check_options = { expect = "+PONG" }`
- **WHEN** the backend writes `+PO`, waits 50 ms, then writes `NG\r\n`
- **THEN** the result is `up`

#### Scenario: Backend closes without the expected text

- **WHEN** the backend closes the connection before `expect` appears in what it sent
- **THEN** the result is `down` with reason `expect: got "<excerpt>"`

##### Example: excerpts

| Backend sends, then closes | Reason |
| --- | --- |
| `ERROR\r\n` | `expect: got "ERROR\r\n"` |
| nothing | `expect: got ""` |
| `say "hi"\x00` | `expect: got "say \"hi\"\x00"` |
| 100 bytes of `a` | `expect: got "` followed by 64 `a` and `"` |

#### Scenario: Too much data without the expected text

- **WHEN** the backend sends 16384 bytes without `expect` among them and keeps the connection open
- **THEN** the result is `down` with a reason starting `expect: got "` without waiting for `timeout`

#### Scenario: Silent backend

- **WHEN** a `tcp` service sets `expect`, and the backend accepts the connection but sends nothing until the service's `timeout` passes
- **THEN** the result is `down` with reason `timeout`

#### Scenario: Invalid send and expect options

- **WHEN** a `tcp` service sets `send` or `expect` to a value that breaks the rules above
- **THEN** validation fails with an error on the named key

##### Example: validation errors

| `check_options` | Error key | Message |
| --- | --- | --- |
| `{ send = "PING\r\n" }` | `expect` | `required when send is set` |
| `{ expect = "" }` | `expect` | `must not be empty` |
| `{ expect = 220 }` | `expect` | `must be a string` |
| `{ send = ["a"], expect = "b" }` | `send` | `must be a string` |
| `expect` of 16385 bytes | `expect` | `must be at most 16384 bytes` |

#### Scenario: TLS handshake succeeds

- **WHEN** a `tcp` service sets `tls = true`, `ca_file` holds the CA that signed the backend certificate and `server_name` matches it
- **THEN** the result is `up`

#### Scenario: TLS with expected text

- **WHEN** a `tcp` service sets `tls = true` and `expect`, and the backend sends a greeting containing `expect` over TLS after the handshake
- **THEN** the result is `up`

##### Example: TLS greeting

- **GIVEN** `check_options = { tls = true, server_name = "db.test", ca_file = "/etc/emfor/ca.pem", expect = "+OK" }` and a certificate for `db.test` signed by that CA
- **WHEN** the backend completes the handshake and sends `+OK ready\r\n`
- **THEN** the result is `up`

#### Scenario: TLS name mismatch

- **WHEN** `tls = true`, `ca_file` holds the CA that signed the backend certificate, and `server_name` does not match the certificate
- **THEN** the result is `down` with a reason starting `tls: `

#### Scenario: TLS verification turned off

- **WHEN** `tls = true`, `tls_verify = false` and the backend certificate names another host and is signed by an unknown CA
- **THEN** the result is `up`

#### Scenario: TLS to a plaintext port

- **WHEN** `tls = true`, the service's `timeout` is 1 s, and the backend accepts the connection but never answers the TLS handshake
- **THEN** the result is `down` with reason `tls: no answer to the TLS handshake (does the port speak TLS?)` before the timeout passes

#### Scenario: CA file removed after validation

- **WHEN** `ca_file` is deleted after the config was applied
- **THEN** checks of that service are `error` with a reason starting `ca_file: `

#### Scenario: Invalid TLS options

- **WHEN** a `tcp` service sets a TLS option that breaks the rules above
- **THEN** validation fails with an error on the named key

##### Example: TLS validation errors

| `check_options` | Error key | Message |
| --- | --- | --- |
| `{ tls = "yes" }` | `tls` | `must be a boolean` |
| `{ server_name = "db.test" }` | `server_name` | `requires tls = true` |
| `{ tls = false, tls_verify = false }` | `tls_verify` | `requires tls = true` |
| `{ tls = true, server_name = "not a name" }` | `server_name` | `not a valid host name or IP address` |
| `{ tls = true, ca_file = "ca.pem" }` | `ca_file` | `not an absolute path` |
| `{ tls = true, ca_file = "/etc/emfor/missing.pem" }` | `ca_file` | `file not found` |
| `{ tls = true, ca_file = "/etc/emfor/not-pem.txt" }` | `ca_file` | `no certificate found` |


<!-- @trace
source: emfor-tcp-tls
updated: 2026-11-12
code:
  - crates/emfor-check/src/net.rs
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-tls/Cargo.toml
  - examples/emfor.toml
  - crates/emfor-check/Cargo.toml
  - crates/emfor-check/src/options.rs
  - crates/emfor-daemon/Cargo.toml
  - crates/emfor-tls/src/lib.rs
  - crates/emfor-tls/src/tests.rs
  - crates/emfor-tls/src/testing.rs
  - Cargo.toml
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

The daemon SHALL check every backend of every service repeatedly. Let `fast` be the smaller of `interval` and 200ms. The first check of a backend SHALL start after a random delay in `[0, fast)`. When a check finishes and the backend's health state is `unknown` and that check's result was `up` or `down`, the next check SHALL start `fast` after the previous check finishes, without jitter. Otherwise each following check SHALL start after the previous check finishes plus `interval` adjusted by a uniformly random offset within `±jitter` of `interval`. Two checks of the same backend SHALL NOT run at the same time.

#### Scenario: Spacing between checks

- **WHEN** a `healthy` backend has `interval = "2s"` and `jitter = "10%"`
- **THEN** the gap between the end of one check and the start of the next is between 1.8s and 2.2s

#### Scenario: Zero jitter

- **WHEN** a `healthy` backend has `interval = "1s"` and `jitter = "0%"`
- **THEN** the gap between the end of one check and the start of the next is 1s

#### Scenario: Fast checks until the state is known

- **WHEN** the daemon starts with a backend that passes every check, `interval = "2s"`, `rise = 2` and `jitter = "0%"`
- **THEN** the backend is `healthy` within 1 second of the start

##### Example: wait before the next check

| `interval` | state after the check | result of the check | wait |
| --- | --- | --- | --- |
| `2s` | unknown | up | 200ms |
| `2s` | unknown | down | 200ms |
| `2s` | unknown | error | 2s ± jitter |
| `2s` | healthy | up | 2s ± jitter |
| `2s` | unhealthy | down | 2s ± jitter |
| `100ms` | unknown | up | 100ms |


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

---
### Requirement: Exec checker

The `exec` checker SHALL run the command given in `check_options.command` once per check, without a shell, with the backend passed in the environment variables `EMFOR_IP`, `EMFOR_PORT`, and `EMFOR_SERVICE`. The result SHALL follow the exit status: 0 is `up`; 1 or 2 is `down` with the first line of standard output as the reason (or `exit code N` when there is none); any other exit code is `error`; death by a signal is `error`; failure to start the command is `error`. When the check exceeds `timeout`, the command's whole process group SHALL be killed and the result SHALL be `down` with reason `timeout`. `check_options.command` SHALL be required, SHALL be a non-empty array of strings whose first element is an absolute path to an existing executable file, and SHALL be the only accepted key.

#### Scenario: Script reports critical

- **WHEN** an `exec` check runs a script that prints `replication lag 120s` and exits 2
- **THEN** the result is `down` with reason `replication lag 120s`

##### Example: exit status mapping

| Command behavior | Result | Reason |
| --- | --- | --- |
| exits 0 | up | none |
| prints `slow`, exits 1 | down | `slow` |
| prints nothing, exits 2 | down | `exit code 2` |
| prints `unknown state`, exits 3 | error | `exit code 3: unknown state` |
| killed by signal 9 | error | `killed by signal 9` |
| sleeps longer than `timeout` | down | `timeout` |

#### Scenario: Backend passed in the environment

- **WHEN** an `exec` check for service `db.svc.emfor` runs against backend `10.0.1.5:5432`
- **THEN** the command sees `EMFOR_IP=10.0.1.5`, `EMFOR_PORT=5432`, and `EMFOR_SERVICE=db.svc.emfor`

#### Scenario: Timeout kills child processes

- **WHEN** an `exec` command starts a background child and both sleep longer than `timeout`
- **THEN** the result is `down` with reason `timeout` and neither process is still running afterwards

#### Scenario: Invalid command option

- **WHEN** a service sets `check = "exec"` with `check_options.command = ["check_pg"]`
- **THEN** validation fails at `check_options.command` because the first element is not an absolute path to an existing executable

---
### Requirement: Reasons are bounded and printable

Every check result's reason, from every checker and plugin, SHALL be normalized before it reaches the health state, the store, the logs of state changes, or the metrics: tab, carriage return and line feed SHALL become the two characters `\t`, `\r` and `\n`; every other character in Unicode category Cc (U+0000–U+001F, U+007F, U+0080–U+009F) SHALL become `\u{` followed by its code point in lowercase hex without leading zeros and `}`; then a reason longer than 512 characters SHALL be cut to its first 511 characters followed by `…`. All other characters, including `\`, SHALL stay as they are.

#### Scenario: Escape sequence from a backend

- **WHEN** an `exec` check prints `bad\x1b[2Jstate` (with a real ESC byte) and exits 1
- **THEN** the backend's reason in the store is `bad\u{1b}[2Jstate`

##### Example: normalization

| Reason produced by the check | Stored reason |
| --- | --- |
| `connection refused` | `connection refused` |
| `a<TAB>b` | `a\tb` |
| `x<ESC>y` | `x\u{1b}y` |
| `x<U+0085>y` | `x\u{85}y` |
| 600 × `a` | 511 × `a` followed by `…` |
| 512 × `a` | 512 × `a` |

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