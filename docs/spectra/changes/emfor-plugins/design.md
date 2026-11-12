## Context

The daemon checks backends through the `Checker` / `CheckerFactory` interface in crates/emfor-daemon/src/checker/mod.rs:
- `validate(options)` runs synchronously while the config is parsed.
- `check(target)` returns a future.
- `run_check` turns a timeout into `down("timeout")`.

`tcp` and `http` are the only factories, registered by `Registry::builtin()`. Config parsing (`config::parse`) runs before the tokio runtime exists. A reload re-parses the file on `SIGHUP` and applies it only if it is fully valid. `Supervisor` keeps a service's running checker when `ServiceConfig::check_changed` is false.

This change adds checks implemented outside emfor:
- `exec` runs a command per check.
- Plugins are long-lived child processes speaking line-delimited JSON.

Both keep foreign code out of the daemon process. Terms follow docs/spectra/LANGUAGE.md: 檢查器 (checker), 外掛 (plugin), 檢查結果 `up` / `down` / `error`.

`emfor-dns` is merged but not archived. It modifies the `service-config` requirements on validation and reload. To avoid two changes rewriting the same requirements, every plugin config rule lives in the new `checker-plugins` capability.

## Goals / Non-Goals

**Goals:**

- Reuse existing Nagios-style check scripts through `exec` with no wrapper.
- Let protocol checks (Redis, MySQL, ...) be written in any language as a plugin, with one process per plugin that can hold connections.
- Report a wrong `check_options` for a plugin at load time, like built-in checkers do.
- A crashed or stuck plugin never marks backends unhealthy; it produces `error`.
- A documented, versioned plugin protocol with example messages the daemon's tests check.

**Non-Goals:**

- In-process plugins (shared libraries, WASM).
- Plugin packaging, discovery, or distribution.
- Plugins for anything other than checks.
- Re-validating options after a plugin restart.
- A concurrency limit for `exec`.
- Shipping production plugins.

## Decisions

### Checker interface carries service and timeout

`Checker::check` takes a `CheckRequest { service: Arc<str>, target: SocketAddr, timeout: Duration }`. The `exec` checker needs the service name for `EMFOR_SERVICE`. Plugins need it, and the timeout, for the `check` message.

`Checker` gains `fn timeout_result(&self) -> CheckResult`, default `down("timeout")`. `run_check` returns `checker.timeout_result()` when the timeout fires. `tcp` and `http` keep the default; the plugin checker returns `error("plugin did not answer within timeout")`.

Alternative considered: pass the timeout only to plugins through their factory. Rejected, because the timeout is a per-service setting that can change on reload while the checker is kept.

### exec checker

`check_options` takes one key, `command`: a non-empty array of strings whose first element is an absolute path to an existing executable file. This is checked at validation, so a typo in the path is a config error. Any other key is an unknown key.

Each check runs the command:
- no shell, working directory `/`, stdin from `/dev/null`;
- the daemon's environment plus `EMFOR_IP`, `EMFOR_PORT`, `EMFOR_SERVICE`;
- in its own process group (`process_group(0)`).

The daemon waits for the process to exit, then reads what the command wrote to stdout (up to 4096 bytes, waiting at most 100ms more). It uses the first line, trimmed and cut to 200 characters. stderr is discarded.

| Outcome | Result |
| --- | --- |
| exit 0 | `up` |
| exit 1 or 2 | `down`; reason = first stdout line, or `exit code N` when stdout is empty |
| exit 3 or any other code | `error`; reason `exit code N: <first line>` |
| killed by a signal | `error`; reason `killed by signal N` |
| cannot start (not found, permission) | `error`; reason `cannot run <path>: <OS error>` |
| timeout | `down` with reason `timeout`; the whole process group gets `SIGKILL` |

A guard holds the command's process group id and sends `SIGKILL` to the group when dropped, so a check whose future is dropped on timeout leaves no processes behind; tokio reaps the killed process. After a normal exit the daemon kills the group once right away, which removes any background children the script left, and then disarms the guard, so a group id that may since have been reused is never signalled.

### Plugin registration in config

```toml
[plugins."redis"]
command = ["/usr/local/libexec/emfor-redis", "--verbose"]

[services."cache.svc.emfor"]
check = "redis"
backends = ["10.0.2.1:6379"]
[services."cache.svc.emfor".check_options]
password_file = "/etc/emfor/redis.pass"
```

- **Plugin names** are 1–63 characters of `[a-z0-9_-]` and must not be `tcp`, `http`, or `exec`.
- **`command`** is required and validated like the `exec` command. `[plugins."<name>"]` takes no other keys.
- **Startup:** every declared plugin is started, including unused ones, so a broken plugin is caught when the config is applied.
- **Selection:** a service selects a plugin with `check = "<name>"`.

Parsing is split in two:
1. `config::parse_plugins(text)` reads only the `[plugins]` table, without starting anything.
2. After the plugins are started, `config::parse(text, &registry)` runs with a registry that has one `PluginFactory` per plugin.

`ServiceConfig` gains `checker_id`:
- for a built-in, the `check` name;
- for a plugin, `plugin:<name>:<command as JSON>`.

`check_changed` compares `checker_id`. Changing a plugin's `command` therefore resets the backend state of every service using it, and the supervisor takes the new checker.

### Plugin protocol version 1

The transport is one JSON object per line, UTF-8:
- daemon to plugin on stdin;
- plugin to daemon on stdout;
- free-form text on stderr.

Every object has a `type`. The plugin exits when stdin reaches end of file.

| Message | Daemon sends | Plugin answers |
| --- | --- | --- |
| handshake | `{"type":"hello","protocol":1}` | `{"type":"hello","protocol":1,"name":"redis","version":"0.1.0"}` |
| validate | `{"type":"validate","id":1,"options":{...}}` | `{"type":"validate","id":1,"errors":[{"key":"password_file","message":"file not found"}]}` (empty list = valid) |
| check | `{"type":"check","id":2,"service":"cache.svc.emfor","target":{"ip":"10.0.2.1","port":6379},"options":{...},"timeout_ms":900}` | `{"type":"check","id":2,"result":"down","reason":"PONG not received"}` |

Message rules:
- **`id`** is a u64, increasing per plugin process. Answers may come in any order.
- **`options`** is the service's `check_options` converted to JSON. TOML datetimes become RFC 3339 strings.
- **`result`** is `up`, `down`, or `error`. `reason` is optional; a `down` or `error` without one gets reason `no reason given`.
- **Error keys** in `validate` are relative to `check_options` and reported as `services."<svc>".check_options.<key>`.
- **`timeout_ms`** is the service `timeout` minus a margin of `min(100ms, timeout / 10)`, so the plugin can answer `down` before the daemon gives up.

Malformed input:
- A stdout line that is not valid JSON, or has an unknown `type`, is logged at `warn` and ignored.
- An answer whose `id` has no pending request (for example, it arrived too late) is ignored, logged at `debug`.
- An answer with an invalid `result` makes that check `error` with reason `invalid plugin response`.

The protocol is specified in docs/plugins/protocol-v1.md. docs/plugins/protocol-v1-examples.jsonl holds one example of each message. The daemon's unit tests parse every line of that file into the protocol types and serialize them back unchanged.

### Plugin process host

`PluginHost` owns one plugin process:
- a writer task fed by an mpsc channel;
- a reader task that routes answers to pending requests (`HashMap<u64, oneshot::Sender>`);
- a task that logs each stderr line at `info` as `plugin <name>: <line>`, with each line cut to 1000 characters.

The process runs with working directory `/`.

Start:
1. Spawn the process.
2. Send `hello` and wait up to 5s for a `hello` with `protocol: 1`.
3. On success, log `plugin <name> started (pid P, <plugin name> <version>)` at `info`.

Failure at start (spawn error, timeout, wrong protocol, or exit before the handshake) is an error naming the plugin. At config time it becomes a config error at `plugins."<name>".command`.

Exit:
- When the process exits or closes stdout, every pending request resolves to `error("plugin exited")`, and the exit status is logged at `warn`.
- The host restarts the process after a backoff of 1s, doubling each time, capped at 30s. The backoff resets to 1s once the process has stayed up for 30s.
- While the plugin is down or restarting, a check returns `error("plugin <name> unavailable")` at once.
- A failed handshake on restart counts as another exit.

Stop: close stdin, wait up to 2s, then `SIGKILL`.

### Validation through the plugin

`PluginFactory::validate` keeps the synchronous `CheckerFactory` signature. It sends `validate` and blocks on the answer for up to 5s, so config parsing that may call it runs on a blocking thread (`spawn_blocking`).

If one validation of a plugin times out, every later validation of that plugin in the same parse fails at once with the same error, so a hung plugin costs 5s and not 5s per service. A timeout is reported as `services."<svc>".check_options: plugin <name> did not answer validation within 5s`.

### Startup and reload with a plugin set

`cli` creates the tokio runtime before loading the config. `PluginSet`, owned by the run loop, maps plugin name to its running host and command.

Loading a config, whether at start, on reload, or for `--check-config`:
1. Read the file.
2. Call `parse_plugins`.
3. Call `PluginSet::prepare(specs)`. Running hosts whose command is unchanged are reused; the others are started.
4. Build the registry.
5. Run `parse` on a blocking thread.
6. Collect all errors: plugin start errors, parse errors, and the existing `[store]` and `[dns] listen` restart rules.

A reused host may be down, waiting to restart after a crash. Its factory then accepts, without asking the plugin, the `check_options` that the applied config already uses with the same plugin and command, since they were validated when that config was applied. Other options for that plugin fail with `plugin <name> unavailable`. This keeps an unrelated reload from being rejected because a plugin happens to be restarting.

If there are no errors, `commit` stops hosts that are no longer declared or were replaced. Otherwise `rollback` stops only the hosts started by this attempt, and the previous config keeps running unchanged.

`--check-config` loads the same way and then stops every plugin before exiting.

On shutdown the daemon stops all plugins after stopping the checks.

## Implementation Contract

**Behavior**

- A service with `check = "exec"` and `command = ["/path/check_x"]`:
  - is `up` when the script exits 0;
  - is `down` with the script's first stdout line when it exits 2;
  - gets `error` when it exits 3, without changing the backend state.
- A script that sleeps past `timeout` is killed together with its children, and the result is `down` with reason `timeout`.
- With `[plugins."fake"]` pointing at a working plugin, a service using `check = "fake"` gets the plugin's results. Killing the plugin process yields `error` for that service's checks and a restart within the backoff, after which results return.
- A plugin that never answers a check yields `error("plugin did not answer within timeout")`. A backend that was `healthy` stays `healthy` until the existing all-errors rule (`fall × interval`) makes it `unknown`.
- `--check-config` with a plugin that rejects an option prints `services."<svc>".check_options.<key>: <message>` and exits 2.
- A reload whose plugin fails to start is rejected with an error at `plugins."<name>".command`; the previous plugins and checks keep running.

**Interfaces and data shapes**

- `CheckRequest { service, target, timeout }`, `Checker::timeout_result`.
- Config: `[plugins."<name>"] command = [...]`; `check = "exec"` with `check_options.command = [...]`.
- Protocol messages exactly as in "Plugin protocol version 1" and docs/plugins/protocol-v1.md.
- Environment for `exec`: `EMFOR_IP`, `EMFOR_PORT`, `EMFOR_SERVICE`.

**Failure modes**

- Plugin start or handshake failure: a config error at `plugins."<name>".command`, reported together with every other error. Exit 2 at startup (config error), rejection on reload.
- Plugin crash at runtime: `error` results, a `warn` log line with the exit status, and a restart with backoff. It never produces `down`.
- Malformed plugin output: logged; the affected check ends in `error` or times out to `error`.

**Acceptance criteria**

- `cargo test --workspace` passes, including:
  - unit tests for `exec`, with scripts written to a temp dir, covering each row of the outcome table;
  - protocol round-trip tests over docs/plugins/protocol-v1-examples.jsonl;
  - `PluginHost` tests with the `emfor-test-plugin` binary: handshake, out-of-order answers, crash and restart, hang, wrong protocol;
  - end-to-end tests that start the `emfor` binary with plugin configs for startup, reload, `--check-config`, crash, and timeout.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass.

**Scope boundaries**

In scope: everything above, plus docs and the example config. Out of scope: the Non-Goals, any change to DNS, balance, the LMDB format, or the health state machine.

## Risks / Trade-offs

- **Config loading starts processes**, so `--check-config` is no longer free of side effects. → Plugins are stopped before it exits, and this is documented in the example config and the protocol doc.
- **A plugin that hangs during validation slows a reload by up to 5s.** → It is capped per plugin, not per service, as described under "Validation through the plugin".
- **`exec` forks once per check per backend.** With many backends and short intervals this costs CPU (not measured). → Documented: use a plugin for frequent checks. A concurrency limit is a non-goal for now.
- **A plugin is shared by all its services**, so one slow check can delay others if the plugin is single-threaded. → The protocol allows out-of-order answers, and the doc tells plugin authors to handle checks concurrently.
- **The `emfor-test-plugin` bin target is built and installed with the daemon crate.** → Its name makes the purpose clear. Moving it to a separate workspace crate is possible later if packaging needs it.

## Migration Plan

Additive. Configs without `[plugins]` and without `check = "exec"` behave as before. The checker interface change is internal.

## Open Questions

None.
