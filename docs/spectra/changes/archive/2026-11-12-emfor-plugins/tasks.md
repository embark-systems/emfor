## 1. Checker interface

- [x] 1.1 Change `Checker::check` to take `CheckRequest { service, target, timeout }` and add `Checker::timeout_result` (default `down("timeout")`), per "Checker interface carries service and timeout". `run_check` in crates/emfor-daemon/src/checker/mod.rs returns `timeout_result()` on timeout. Migrate `tcp`, `http`, and the scheduler's call in crates/emfor-daemon/src/schedule.rs. This delivers the built-in half of "Check results are up, down, or error". Verify: the existing `cargo test --workspace` suite passes unchanged in behavior, plus a new unit test in `checker/mod.rs` where a checker overriding `timeout_result` with `error(...)` gets that result from `run_check`.

## 2. exec checker

- [x] 2.1 [after: 1.1] Add the built-in `exec` checker in crates/emfor-daemon/src/checker/exec.rs, per "exec checker", delivering the "Exec checker" requirement. It covers the `command` validation, the environment variables, the process group with a kill-on-drop guard, the exit status mapping, and the stdout reason. Register it in `Registry::builtin()`. Verify with unit tests that write shell scripts to a temp dir:
  - one test per row of the "exit status mapping" example;
  - a test that the environment variables are set;
  - a test where a forked child is gone after a timeout (checked by pid);
  - the "Invalid command option" scenario.

## 3. Plugin protocol and process

- [x] 3.1 Define the protocol version 1 message types (serde) in crates/emfor-daemon/src/plugin/protocol.rs, per "Plugin protocol version 1". This includes TOML-to-JSON option conversion, with datetimes as RFC 3339 strings, and the `timeout_ms` margin. Write docs/plugins/protocol-v1.md, the plugin author's guide: transport, messages, rules, exit on EOF, concurrency advice. Write docs/plugins/protocol-v1-examples.jsonl with one line per message kind. Verify with unit tests:
  - every line of the examples file parses into the protocol types and serializes back to equal JSON;
  - every row of the "timeout_ms from the service timeout" example.
- [x] 3.2 [after: 3.1] Add the `emfor-test-plugin` bin target (crates/emfor-daemon/tests/support/test_plugin.rs, declared in crates/emfor-daemon/Cargo.toml). It speaks protocol 1 and is driven by its options and environment:
  - `mode` = `up`, `down`, `error`, `bogus` (an invalid `result`), `hang` (never answers), `crash` (exits), or `slow_first` (delays its first answer, so answers come out of order);
  - `validate` rejects unknown keys and bad `mode` values;
  - environment switches: answer the handshake with protocol 2, never answer the handshake, or exit at start;
  - it writes `test plugin started` to stderr at start.

  Verify by a unit-level test that pipes a handshake, a validate, and a check through the binary and checks the answers.
- [x] 3.3 [after: 3.2] Implement `PluginHost` in crates/emfor-daemon/src/plugin/host.rs, per "Plugin process host", delivering "Plugin process supervision" and the logging and malformed-input rules of "Plugin check results":
  - spawn and handshake with a 5s limit;
  - id-based routing of answers;
  - stderr lines logged at `info`;
  - on exit: a `warn` log, pending requests failed with `plugin exited`, and restart with 1s/2s/4s…30s backoff that resets after 30s of uptime;
  - `unavailable` errors while the plugin is down;
  - stop: close stdin, then kill after 2s.

  Verify with tokio tests using `emfor-test-plugin`:
  - handshake ok;
  - wrong protocol and no-hello failures;
  - out-of-order answers;
  - crash, then pending `plugin exited`, then restart;
  - the backoff delays from the "restart delays" example, using a paused clock where possible;
  - stop leaves no process.
- [x] 3.4 [after: 1.1, 3.3] Implement `PluginChecker` and `PluginFactory` in crates/emfor-daemon/src/plugin/mod.rs, per "Validation through the plugin", delivering "Plugin check results" and the plugin half of "Check results are up, down, or error":
  - checks send `check` requests and map the answers (missing reason becomes `no reason given`, an invalid result becomes `invalid plugin response`);
  - `timeout_result` is `error("plugin did not answer within timeout")`;
  - `validate` blocks up to 5s and maps errors to keys. After one timeout, later validations of the same plugin fail at once.

  Verify with tests using `emfor-test-plugin` in modes `up`, `down`, `bogus`, and `hang` (through `run_check`), plus validation errors and the sticky-timeout behavior (the second validation returns well under 5s).

## 4. Config, startup, and reload

- [x] 4.1 [after: 2.1] Add the `[plugins]` table to crates/emfor-daemon/src/config.rs, per "Plugin registration in config":
  - `parse_plugins(text)` returns the plugin specs and errors;
  - name rules and reserved names;
  - `command` validation shared with `exec`;
  - the unknown-key rule and the top-level `plugins` key;
  - `ServiceConfig::checker_id`, with `check_changed` comparing it.

  This delivers "Plugin registration in config". Verify with config unit tests covering every row of the "plugin declaration validation" example, and a `check_changed` test where only the plugin `command` differs.
- [x] 4.2 [after: 3.4, 4.1] Load the config inside the tokio runtime with a `PluginSet` in crates/emfor-daemon/src/cli.rs, per "Startup and reload with a plugin set", delivering "Plugin validation of check options":
  - `prepare` reuses or starts hosts, then the registry is built, then `parse` runs on a blocking thread, then all errors are merged;
  - `commit` / `rollback`;
  - `--check-config` stops the plugins before exiting;
  - shutdown stops all plugins.

  Verify with end-to-end tests in crates/emfor-daemon/tests/plugins.rs that run the `emfor` binary:
  - a service using `emfor-test-plugin` in mode `up` becomes `healthy` in the LMDB record;
  - the "Plugin rejects an option" scenario via `--check-config` (exit 2, message);
  - the "Check config with a broken plugin" scenario (exit 2, no plugin process left, checked with `pgrep -f`).
- [x] 4.3 [after: 4.2] Apply the plugin reload rules, per "Plugins on reload": keep unchanged plugins, start new or changed ones before validation, stop removed or replaced ones on commit, stop only new ones on rollback, and reset services whose plugin `command` changed. This delivers "Plugins on reload". Verify with end-to-end tests:
  - "Unchanged plugin survives reload" (same pid, from the `plugin ... started (pid P` log line);
  - "Rejected reload with a new plugin" (its process gone, previous checks continue);
  - a `command` change that restarts the plugin and resets the backend to `unknown`.
- [x] 4.4 [after: 4.2] Add end-to-end tests of runtime plugin behavior, delivering the runtime scenarios of "Plugin process supervision", "Plugin check results", and "Check results are up, down, or error":
  - "Crash and recovery": kill the plugin pid; the backend stays `healthy` and has an `error` reason during the outage; the `test plugin started` log line appears twice;
  - "Plugin timeout is not a backend failure", in mode `hang`;
  - "Plugin stderr is logged".

  Verify: these tests pass in `cargo test --workspace`.

## 5. Examples and docs

- [x] 5.1 [after: 4.2] Add commented `exec` and `[plugins]` examples to examples/emfor.toml. They stay commented because the referenced executables do not exist on a fresh host, so the shipped example remains loadable. The examples point to docs/plugins/protocol-v1.md. Update docs/spectra/LANGUAGE.md if the `外掛` / `檢查器` entries need the `exec` / `[plugins.*]` wording adjusted. Verify: the existing `example_config_is_valid` test still passes, and a content review confirms both examples appear and match the config rules in `specs/checker-plugins/spec.md`.
