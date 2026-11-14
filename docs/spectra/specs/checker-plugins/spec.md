# checker-plugins Specification

## Purpose

Defines external plugins: checkers that run as long-lived child processes outside the emfor daemon and talk to it over a versioned, line-delimited JSON protocol on stdin and stdout. Covers how plugins are registered in config, the protocol, process supervision, and how plugins take part in config validation and reload.

## Requirements

### Requirement: Plugin registration in config

A plugin SHALL be declared as `[plugins."<name>"]` with a required `command`: a non-empty array of strings whose first element is an absolute path to an existing executable file. `[plugins."<name>"]` SHALL accept no other keys. Plugin names SHALL be 1 to 63 characters of `a-z`, `0-9`, `_`, `-` and SHALL NOT be `tcp`, `http`, or `exec`. A service SHALL use a plugin by setting `check` to the plugin's name. Every declared plugin SHALL be started when a config is applied, whether or not a service uses it.

#### Scenario: Service uses a plugin

- **WHEN** the config declares `[plugins."redis"]` with a valid `command` and a service sets `check = "redis"`
- **THEN** that service's backends are checked by the `redis` plugin

##### Example: plugin declaration validation

| Declaration | Result |
| --- | --- |
| `[plugins."redis"]` `command = ["/usr/local/libexec/emfor-redis"]` (file exists, executable) | ok |
| `[plugins."redis"]` without `command` | error at `plugins.redis.command`: missing required key |
| `[plugins."redis"]` `command = []` | error at `plugins.redis.command`: must not be empty |
| `[plugins."redis"]` `command = ["emfor-redis"]` | error at `plugins.redis.command`: not an absolute path |
| `[plugins."http"]` with a valid `command` | error at `plugins.http`: name is reserved for a built-in checker |
| `[plugins."Redis"]` with a valid `command` | error at `plugins.Redis`: invalid plugin name |
| `[plugins."redis"]` with `args = ["-v"]` | error at `plugins.redis.args`: unknown key |

---
### Requirement: Plugin protocol version 1

The daemon and a plugin SHALL exchange one JSON object per line, UTF-8, daemon to plugin on the plugin's stdin and plugin to daemon on its stdout. On start the daemon SHALL send `{"type":"hello","protocol":1}` and the plugin SHALL answer with `{"type":"hello","protocol":1,"name":...,"version":...}` within 5 seconds; any other answer, no answer, or a different `protocol` SHALL be a plugin start failure. A `validate` request SHALL carry `id` and `options`, and its answer SHALL carry the same `id` and an `errors` list of `{key, message}` (empty when valid). A `check` request SHALL carry `id`, `service`, `target` (`ip`, `port`), `options`, and `timeout_ms`, and its answer SHALL carry the same `id`, `result` (`up`, `down`, or `error`), and an optional `reason`. Answers MAY arrive in any order. `timeout_ms` SHALL be the service `timeout` minus `min(100ms, timeout / 10)`. A plugin SHALL exit when its stdin reaches end of file.

#### Scenario: Out-of-order answers

- **WHEN** the daemon sends check requests with ids 7 and 8 to a plugin and the plugin answers 8 before 7
- **THEN** each answer is applied to the backend of its own request

#### Scenario: Wrong protocol version

- **WHEN** a plugin answers the handshake with `{"type":"hello","protocol":2,"name":"x","version":"1"}`
- **THEN** the plugin fails to start with an error naming the plugin and both protocol versions

##### Example: timeout_ms from the service timeout

| Service `timeout` | `timeout_ms` sent |
| --- | --- |
| `1s` | 900 |
| `5s` | 4900 |
| `200ms` | 180 |

---
### Requirement: Plugin check results

A plugin answer SHALL become the check result given by its `result` and `reason`; a `down` or `error` answer without `reason` SHALL use reason `no reason given`. An answer with any other `result` value SHALL make the check `error` with reason `invalid plugin response`. A stdout line that is not valid JSON or has an unknown `type` SHALL be logged at `warn` and ignored. An answer whose `id` matches no pending request SHALL be ignored. Each line the plugin writes to stderr SHALL be logged at `info` with the plugin name.

#### Scenario: Plugin reports down

- **WHEN** a plugin answers a check with `{"type":"check","id":3,"result":"down","reason":"PONG not received"}`
- **THEN** the check result is `down` with reason `PONG not received`

#### Scenario: Plugin stderr is logged

- **WHEN** a plugin writes `connection pool exhausted` to stderr
- **THEN** the daemon log has an `info` line containing the plugin name and `connection pool exhausted`

---
### Requirement: Plugin process supervision

When a plugin process exits or closes its stdout, or stops answering, the daemon SHALL log its exit status at `warn`, resolve every pending request of that plugin to `error` with reason `plugin exited`, and restart the plugin after a delay that starts at 1 second, doubles after each consecutive failure, is capped at 30 seconds, and resets to 1 second once the plugin has stayed up for 30 seconds. A failed handshake on restart SHALL count as a failure. While a plugin is not running, its checks SHALL return `error` with reason `plugin <name> unavailable` without waiting for the timeout.

A running plugin SHALL count as having stopped answering when the callers of 3 requests in a row (`check` or `validate`) gave up waiting for their answers, with no answer to any request arriving in between; any answer, including one to a request nobody waits for any more, SHALL reset the count. The daemon SHALL then log at `warn` that plugin `<name>` stopped answering, kill the process, and handle it as a plugin that exited. A plugin that answers every `check` within `timeout_ms` is never killed this way.

#### Scenario: Plugin stops answering

- **WHEN** the process of plugin `sip` is running but answers no request, and three checks in a row of its backends end with `plugin did not answer within timeout`
- **THEN** the daemon logs that plugin `sip` stopped answering, kills it, and starts it again after the restart delay, and later checks get the restarted plugin's answers

##### Example: unanswered requests

| Requests whose callers gave up, in order | Answers in between | Plugin |
| --- | --- | --- |
| 3 | none | killed and restarted |
| 2, then an answer, then 2 | one | keeps running |
| 1 (an answer arrived after the timeout) | the late answer | keeps running |

#### Scenario: Crash and recovery

- **WHEN** the process of plugin `redis` is killed while its backends are `healthy`
- **THEN** checks of those backends are `error` until the plugin is restarted, the backends stay `healthy` during that time, and checks return to the plugin's results after the restart

##### Example: restart delays

| Consecutive failures | Delay before next start |
| --- | --- |
| 1 | 1s |
| 2 | 2s |
| 3 | 4s |
| 6 | 30s (capped) |
| any, after the plugin stayed up 30s | 1s |

---
### Requirement: Plugin validation of check options

When a config is loaded at startup, on `SIGHUP` reload, or by `emfor check-config`, the daemon SHALL start the declared plugins and send each service's `check_options` to its plugin in a `validate` request before applying the config. Every error the plugin returns SHALL be a config error at `services."<service>".check_options.<key>`. A plugin that fails to start or to complete the handshake SHALL be a config error at `plugins."<name>".command`. A plugin that does not answer a `validate` request within 5 seconds SHALL be a config error at `services."<service>".check_options`, and every later `validate` request to that plugin in the same load SHALL fail with the same error without waiting. These errors SHALL be reported together with all other validation errors. `emfor check-config` SHALL stop every plugin it started before exiting.

#### Scenario: Plugin rejects an option

- **WHEN** a plugin answers `validate` for service `cache.svc.emfor` with `{"errors":[{"key":"password_file","message":"file not found"}]}`
- **THEN** config validation fails with `services."cache.svc.emfor".check_options.password_file: file not found`

#### Scenario: Check config with a broken plugin

- **WHEN** `emfor check-config` runs with a config whose plugin exits before answering the handshake
- **THEN** it prints an error at `plugins."<name>".command`, exits with status 2, and leaves no plugin process running


<!-- @trace
source: emfor-server-subcommand
updated: 2026-11-13
code:
  - README.md
  - docs/deploy/plugins.md
  - examples/systemd/emfor.service
  - tests/spike/run.sh
  - docs/plugins/protocol-v1.md
  - examples/freebsd/Makefile
  - examples/freebsd/README.md
  - examples/freebsd/files/emfor.in
  - CHANGELOG.md
  - tests/plugins/inside.sh
  - README.zh-TW.md
  - docs/deploy/daemon.md
  - tests/resolver/resolved.sh
  - examples/emfor.toml
  - tests/resolver/inside.sh
  - crates/emfor-daemon/src/cli.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - crates/emfor-daemon/tests/metrics.rs
  - crates/emfor-daemon/tests/daemon.rs
  - crates/emfor-daemon/tests/plugins.rs
-->

---
### Requirement: Plugins on reload

On `SIGHUP` reload, a plugin whose `command` is unchanged SHALL keep its running process. A plugin that is new or whose `command` changed SHALL be started before the new config is validated. If the reload is applied, plugins that were removed or replaced SHALL be stopped, and services whose plugin `command` changed SHALL restart their backends from `unknown`. If the reload is rejected, only the plugin processes started for that reload SHALL be stopped, and the previous plugins and checks SHALL keep running unchanged. If a kept plugin is not running when the reload is validated (it is waiting to restart), a service whose plugin and `check_options` are the same as in the applied config SHALL be accepted without a `validate` request, and any other `check_options` for that plugin SHALL be a config error at `services."<service>".check_options` with message `plugin <name> unavailable`. Stopping a plugin SHALL close its stdin and SHALL kill it if it has not exited within 2 seconds. On daemon shutdown every plugin SHALL be stopped.

#### Scenario: Unchanged plugin survives reload

- **WHEN** a reload changes only a service's `interval` and the service uses plugin `redis`
- **THEN** the `redis` plugin process keeps the same process id

#### Scenario: Reload while a plugin is restarting

- **WHEN** plugin `redis` has exited and is waiting to restart, and a reload changes only the `interval` of a service using `redis`
- **THEN** the reload is applied without a `validate` request to `redis`

#### Scenario: Rejected reload with a new plugin

- **WHEN** a reload adds plugin `mysql` and also contains an invalid `timeout`
- **THEN** the reload is rejected, the `mysql` process started for it is stopped, and the previous config keeps running

---
### Requirement: Plugin output line limit

The daemon SHALL read at most 65536 bytes of any single line a plugin writes, on stdout and on stderr, and SHALL NOT buffer more than that for one line. When a stdout line exceeds 65536 bytes before its line feed, the daemon SHALL log at `warn` that plugin `<name>` wrote a line longer than 65536 bytes and stop reading its stdout; the plugin SHALL then be handled as a plugin that closed its stdout (it is stopped, pending requests become `error` with reason `plugin exited`, and it is restarted after the backoff delay). When a stderr line exceeds 65536 bytes, the daemon SHALL log the line's first 65536 bytes as it logs any stderr line and discard the rest of that line up to its line feed; the plugin keeps running.

#### Scenario: Oversized answer line

- **WHEN** plugin `p` writes 70000 bytes to stdout without a line feed
- **THEN** the daemon logs that plugin `p` wrote a line longer than 65536 bytes, its pending checks are `error` with reason `plugin exited`, and the plugin is started again after the restart delay

#### Scenario: Oversized stderr line

- **WHEN** plugin `p` writes 70000 bytes to stderr followed by a line feed, then answers a check `up`
- **THEN** the daemon logs one stderr line of plugin `p`, the plugin keeps running, and the check result is `up`

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