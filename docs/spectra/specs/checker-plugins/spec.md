# checker-plugins Specification

## Purpose

Defines external plugins: checkers that run as long-lived child processes outside the emfor daemon and talk to it over a versioned, line-delimited JSON protocol on stdin and stdout. Covers how plugins are registered in config, the protocol, process supervision, and how plugins take part in config validation and reload.

## Requirements

### Requirement: Plugin registration in config

A plugin SHALL be declared as `[plugins."<name>"]` with a required `command`: a non-empty array of strings whose first element is an absolute path to an existing executable file and no element of which contains a NUL character (`must not contain NUL`). `[plugins."<name>"]` SHALL accept the optional key `env_pass`, an array of strings naming the variables of the daemon's environment that the plugin process also gets (see "Plugin environment"), each a variable name of `[A-Za-z_][A-Za-z0-9_]*` (`invalid variable name "<name>"`) and none twice (`duplicate variable name "<name>"`); an `env_pass` that is not an array of strings SHALL be an error `must be an array of strings`. `[plugins."<name>"]` SHALL accept no other keys. Plugin names SHALL be 1 to 63 characters of `a-z`, `0-9`, `_`, `-` and SHALL NOT be `tcp`, `http`, or `exec`. A service SHALL use a plugin by setting `check` to the plugin's name. Every declared plugin SHALL be started when a config is applied, whether or not a service uses it.

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
| `[plugins."redis"]` with a valid `command` and `env_pass = ["http_proxy"]` | ok |
| `[plugins."redis"]` with a valid `command` and `env_pass = ["A B"]` | error at `plugins.redis.env_pass`: invalid variable name "A B" |
| `[plugins."redis"]` with a valid `command` and `env_pass = ["HOME", "HOME"]` | error at `plugins.redis.env_pass`: duplicate variable name "HOME" |
| `[plugins."redis"]` with a valid `command` and `env_pass = "HOME"` | error at `plugins.redis.env_pass`: must be an array of strings |


<!-- @trace
source: emfor-child-environment
updated: 2026-11-21
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/checker/exec.rs
  - crates/emfor-daemon/src/child.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/plugin/host.rs
  - crates/emfor-daemon/src/plugin/mod.rs
  - crates/emfor-daemon/src/plugin/set.rs
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual.zh_TW/04-health-checks.md
  - docs/manual.zh_TW/08-operations.md
  - docs/manual/05-configuring.md
  - docs/manual/06-health-checks.md
  - docs/manual/12-operating.md
tests:
  - crates/emfor-daemon/tests/peers.rs
  - crates/emfor-daemon/tests/plugin_host.rs
  - crates/emfor-daemon/tests/plugins.rs
-->

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

A plugin answer SHALL become the check result given by its `result` and `reason`; a `down` or `error` answer without `reason` SHALL use reason `no reason given`. An answer with any other `result` value SHALL make the check `error` with reason `invalid plugin response`. A stdout line that is not valid JSON or has an unknown `type` SHALL be logged at `warn` and ignored. An answer whose `id` matches no pending request SHALL be ignored. Each line the plugin writes to stderr SHALL be logged at `info` with the plugin name. Stderr lines and invalid stdout lines SHALL each be rate limited per plugin process: up to 100 lines in a burst, refilled at 10 lines a second. A line over the limit SHALL NOT be logged; the number of such lines SHALL be logged at `warn` as `plugin <name>: suppressed <n> stderr lines` (or `invalid stdout lines`) right before the next line of that stream that is logged, and when the stream ends. (100 lines leave room for a startup message or a stack trace; 10 a second keeps an occasional line per check, while a plugin writing in a loop cannot fill the log.) A plugin built on the `emfor-plugin` crate SHALL send the first 512 characters of a reason, followed by `…` when the reason was longer (so at most 513 characters), so the answer line stays within the 65536-byte line limit whatever text a backend returns; the daemon's reason normalization (spec health-checking) then keeps at most 512 characters, the first 511 followed by `…`. Such a plugin SHALL answer a check request whose target `ip` is not an IP address `error` with reason `invalid target ip "<ip>"` (the text of `ip` in double quotes), and a check whose task ended without an outcome and without a panic (it was cancelled) `error` with reason `check was cancelled`. Such a plugin SHALL answer a check whose code panics `error` with reason `check panicked: <message>` (or `check panicked` when the panic carries no text), and SHALL keep serving other requests.

#### Scenario: Plugin reports down

- **WHEN** a plugin answers a check with `{"type":"check","id":3,"result":"down","reason":"PONG not received"}`
- **THEN** the check result is `down` with reason `PONG not received`

#### Scenario: Plugin stderr is logged

- **WHEN** a plugin writes `connection pool exhausted` to stderr
- **THEN** the daemon log has an `info` line containing the plugin name and `connection pool exhausted`

#### Scenario: Reason from a plugin crate is cut

- **WHEN** a plugin built on the `emfor-plugin` crate answers a check `down` with a reason of 70000 characters
- **THEN** the answer line is shorter than 65536 bytes and its reason is the first 512 characters followed by `…`, 513 characters in all

#### Scenario: Target ip is not an address

- **WHEN** a plugin built on the `emfor-plugin` crate receives a check request whose target `ip` is `nope`
- **THEN** it answers `error` with reason `invalid target ip "nope"`

#### Scenario: Plugin log lines are rate limited

- **WHEN** plugin `p` writes 1000 lines to stderr at once and then exits
- **THEN** the daemon logs the first 100 of them and one `warn` line `plugin p: suppressed 900 stderr lines`


<!-- @trace
source: emfor-spec-code-alignment
updated: 2026-11-22
code:
  - docs/release-signing.md
  - crates/emfor-check/src/options.rs
  - crates/emfor-plugin/src/lib.rs
  - docs/casa-assessment.md
tests:
  - crates/emfor-daemon/tests/plugin_host.rs
  - crates/emfor-daemon/tests/support/test_plugin.rs
  - crates/emfor-daemon/tests/plugins.rs
-->

---
### Requirement: Plugin process supervision

When a plugin process exits or closes its stdout, or stops answering, the daemon SHALL log its exit status at `warn`, resolve every pending request of that plugin to `error` with reason `plugin exited`, and restart the plugin after a delay that starts at 1 second, doubles after each consecutive failure, is capped at 30 seconds, and resets to 1 second once the plugin has stayed up for 30 seconds. A failed handshake on restart SHALL count as a failure. While a plugin is not running, its checks SHALL return `error` with reason `plugin <name> unavailable` without waiting for the timeout. The daemon SHALL hold at most 1024 requests waiting to be written to a plugin's stdin; while that many wait (the plugin does not read its stdin), a further check SHALL return `error` with reason `plugin <name> busy: it is not reading its requests` without waiting for the timeout.

A running plugin SHALL count as having stopped answering when the callers of 3 requests in a row (`check` or `validate`) gave up waiting for their answers, with no answer to any request arriving in between; any answer, including one to a request nobody waits for any more, SHALL reset the count. The daemon SHALL then log at `warn` that plugin `<name>` stopped answering, kill the process, and handle it as a plugin that exited. A plugin that answers every `check` within `timeout_ms` is never killed this way.

#### Scenario: Plugin that stops reading its stdin

- **WHEN** a plugin stops reading its stdin while its checks keep coming and their callers keep waiting
- **THEN** once 1024 requests wait to be written, further checks of that plugin are `error` with reason `plugin <name> busy: it is not reading its requests` at once, and the daemon's memory for them stops growing

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

On `SIGHUP` reload, a plugin whose `command` and `env_pass` are unchanged SHALL keep its running process. A plugin that is new or whose `command` or `env_pass` changed SHALL be started before the new config is validated. If the reload is applied, plugins that were removed or replaced SHALL be stopped, and services whose plugin `command` or `env_pass` changed SHALL restart their backends from `unknown`. If the reload is rejected, only the plugin processes started for that reload SHALL be stopped, and the previous plugins and checks SHALL keep running unchanged. If a kept plugin is not running when the reload is validated (it is waiting to restart), a service whose plugin and `check_options` are the same as in the applied config SHALL be accepted without a `validate` request, and any other `check_options` for that plugin SHALL be a config error at `services."<service>".check_options` with message `plugin <name> unavailable`. Stopping a plugin SHALL close its stdin and SHALL kill it if it has not exited within 2 seconds. On daemon shutdown every plugin SHALL be stopped. Each plugin SHALL be started in a process group of its own, and whenever a plugin process exits, is stopped or is killed (including when it stopped answering), the daemon SHALL kill every process left in that group. The daemon SHALL also end the tasks that read that process's stdout and stderr and write to its stdin: after the group was killed, it SHALL give the stderr and stdout readers 100 milliseconds to read what is left in the pipes (so the last lines a plugin wrote before it exited are still logged), and SHALL then end all three, even when a process that left the group still holds a pipe open.

#### Scenario: Unchanged plugin survives reload

- **WHEN** a reload changes only a service's `interval` and the service uses plugin `redis`
- **THEN** the `redis` plugin process keeps the same process id

#### Scenario: Changed env_pass restarts the plugin

- **WHEN** a reload changes only `[plugins."redis"] env_pass` from `[]` to `["HOME"]`
- **THEN** the `redis` plugin is started again with the new environment, and services using it restart their backends from `unknown`

#### Scenario: Processes a plugin started are stopped with it

- **WHEN** plugin `p` started a background process and is then stopped, crashes, or is killed because it stopped answering
- **THEN** that background process is killed too

#### Scenario: Reload while a plugin is restarting

- **WHEN** plugin `redis` has exited and is waiting to restart, and a reload changes only the `interval` of a service using `redis`
- **THEN** the reload is applied without a `validate` request to `redis`

#### Scenario: Rejected reload with a new plugin

- **WHEN** a reload adds plugin `mysql` and also contains an invalid `timeout`
- **THEN** the reload is rejected, the `mysql` process started for it is stopped, and the previous config keeps running

#### Scenario: Last stderr lines of an exiting plugin are logged

- **WHEN** plugin `p` writes `fatal: out of memory` to stderr and exits at once
- **THEN** the daemon logs `plugin p: fatal: out of memory` before it restarts the plugin

#### Scenario: Pipe held by a process outside the group

- **WHEN** plugin `p` started a background process that left the plugin's process group and keeps the plugin's stderr open, and `p` is restarted after it exited
- **THEN** the tasks that served the old process end within 1 second of the exit, and the number of live tasks does not grow with each restart


<!-- @trace
source: emfor-child-environment
updated: 2026-11-21
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/checker/exec.rs
  - crates/emfor-daemon/src/child.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/plugin/host.rs
  - crates/emfor-daemon/src/plugin/mod.rs
  - crates/emfor-daemon/src/plugin/set.rs
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual.zh_TW/04-health-checks.md
  - docs/manual.zh_TW/08-operations.md
  - docs/manual/05-configuring.md
  - docs/manual/06-health-checks.md
  - docs/manual/12-operating.md
tests:
  - crates/emfor-daemon/tests/peers.rs
  - crates/emfor-daemon/tests/plugin_host.rs
  - crates/emfor-daemon/tests/plugins.rs
-->

---
### Requirement: Plugin output line limit

The daemon SHALL read at most 65536 bytes of any single line a plugin writes, on stdout and on stderr, and SHALL NOT buffer more than that for one line. When a stdout line exceeds 65536 bytes before its line feed, the daemon SHALL log at `warn` that plugin `<name>` wrote a line longer than 65536 bytes and stop reading its stdout; the plugin SHALL then be handled as a plugin that closed its stdout (it is stopped, pending requests become `error` with reason `plugin exited`, and it is restarted after the backoff delay). When a stderr line exceeds 65536 bytes, the daemon SHALL read its first 65536 bytes, discard the rest of that line up to its line feed and log the line as it logs any stderr line; the plugin keeps running. The daemon SHALL log at most the first 1000 characters of any stderr line, whatever its length. A stdout line that is not a valid message SHALL be logged (at `warn`, see Plugin check results) with at most its first 200 characters, and a first stdout line that is not a valid `hello` SHALL be quoted in the handshake failure with at most its first 200 characters.

#### Scenario: Oversized answer line

- **WHEN** plugin `p` writes 70000 bytes to stdout without a line feed
- **THEN** the daemon logs that plugin `p` wrote a line longer than 65536 bytes, its pending checks are `error` with reason `plugin exited`, and the plugin is started again after the restart delay

#### Scenario: Oversized stderr line

- **WHEN** plugin `p` writes 70000 bytes to stderr followed by a line feed, then answers a check `up`
- **THEN** the daemon logs one stderr line of plugin `p` holding its first 1000 characters and no more, the plugin keeps running, and the check result is `up`

#### Scenario: Long invalid stdout line

- **WHEN** plugin `p` writes a line of 300 characters that is not JSON to stdout
- **THEN** the daemon logs at `warn` that it is ignoring an invalid line of plugin `p`, quoting its first 200 characters and no more

#### Scenario: Long handshake answer

- **WHEN** the first stdout line of plugin `p` is 300 characters long and is not a valid `hello`
- **THEN** the plugin is refused with an error that quotes the first 200 characters of that line and no more


<!-- @trace
source: emfor-spec-code-alignment
updated: 2026-11-22
code:
  - docs/release-signing.md
  - crates/emfor-check/src/options.rs
  - crates/emfor-plugin/src/lib.rs
  - docs/casa-assessment.md
tests:
  - crates/emfor-daemon/tests/plugin_host.rs
  - crates/emfor-daemon/tests/support/test_plugin.rs
  - crates/emfor-daemon/tests/plugins.rs
-->

---
### Requirement: Cancelled requests do not count as unanswered

A request SHALL count towards a plugin having stopped answering only when its caller gave up waiting after the time it was willing to wait (for a `check` the check's timeout, for a `validate` the validation timeout). A request that is dropped earlier, because the task that made it was cancelled by a reload or by shutdown, SHALL NOT count.

#### Scenario: Reload cancels checks in flight

- **WHEN** four checks of one plugin are waiting for their answers and the daemon cancels their tasks long before their timeout
- **THEN** the plugin is not killed or restarted, and it answers the next request

#### Scenario: Callers give up after the timeout

- **WHEN** three requests in a row get no answer and their callers give up when the timeout passes
- **THEN** the plugin is killed and restarted as before

---
### Requirement: Bounded validation answer

A plugin library SHALL answer `validate` with at most 12 errors, each key and message cut to its first 512 characters followed by `…`, and SHALL drop errors from the end until the JSON line is at most 48 KiB (keeping at least one), so that the answer stays under the 64 KiB line limit of the daemon even when every character takes six bytes in JSON.

#### Scenario: Many long errors

- **WHEN** a plugin finds 100 errors whose messages are 2000 characters long
- **THEN** the answer has 12 errors whose keys and messages are 512 characters followed by `…`

#### Scenario: Control characters

- **WHEN** the messages are control characters, which take six bytes each in JSON
- **THEN** the answer line is under 64 KiB and has at least one error

---
### Requirement: Plugin environment

A plugin process SHALL be started with an environment that contains only: the variables `PATH`, `LANG`, `TZ` and every variable whose name starts with `LC_`, with the values they have in the daemon's environment (a variable the daemon does not have is not set); and the variables named in the plugin's `env_pass`, with the values they have in the daemon's environment (a name the daemon does not have is not set and is not an error). No other variable of the daemon's environment SHALL reach the plugin, and emfor SHALL NOT set `EMFOR_IP`, `EMFOR_PORT` or `EMFOR_SERVICE` for it. The same environment SHALL be used each time the plugin is restarted after it exited.

#### Scenario: Variable of the daemon is not passed to a plugin

- **GIVEN** the daemon's environment has `SECRET_TOKEN=abc` and `HOME=/root`
- **WHEN** a plugin without `env_pass` starts and prints its environment to stderr
- **THEN** the output has neither `SECRET_TOKEN` nor `HOME`

#### Scenario: Locale, time zone and PATH are passed to a plugin

- **GIVEN** the daemon's environment has `PATH=/usr/bin:/bin`, `LANG=C.UTF-8`, `LC_ALL=C` and `TZ=UTC`
- **WHEN** a plugin starts and prints its environment to stderr
- **THEN** the output has `PATH=/usr/bin:/bin`, `LANG=C.UTF-8`, `LC_ALL=C` and `TZ=UTC`

#### Scenario: Variable named in env_pass reaches a plugin, also after a restart

- **GIVEN** the daemon's environment has `http_proxy=http://proxy:3128`, and `[plugins."redis"] env_pass = ["http_proxy"]`
- **WHEN** the plugin starts, exits and is restarted
- **THEN** both processes see `http_proxy=http://proxy:3128`

<!-- @trace
source: emfor-child-environment
updated: 2026-11-21
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/checker/exec.rs
  - crates/emfor-daemon/src/child.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/plugin/host.rs
  - crates/emfor-daemon/src/plugin/mod.rs
  - crates/emfor-daemon/src/plugin/set.rs
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual.zh_TW/04-health-checks.md
  - docs/manual.zh_TW/08-operations.md
  - docs/manual/05-configuring.md
  - docs/manual/06-health-checks.md
  - docs/manual/12-operating.md
tests:
  - crates/emfor-daemon/tests/peers.rs
  - crates/emfor-daemon/tests/plugin_host.rs
  - crates/emfor-daemon/tests/plugins.rs
-->