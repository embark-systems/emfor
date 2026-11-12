## Why

The daemon only knows how to check backends with the built-in `tcp` and `http` checkers. Real services need protocol-level checks: a Redis `PING`, a MySQL login, a replication-lag query, or an existing Nagios/Icinga script that operations already trusts. Building every protocol into emfor is not sustainable. This change adds two ways to check backends with code outside emfor, without loading that code into the daemon process.

## What Changes

- **`exec` checker (built-in).** Each check runs a command once.
  - `check_options.command` is an array, run without a shell.
  - The backend is passed in the environment variables `EMFOR_IP`, `EMFOR_PORT`, and `EMFOR_SERVICE`.
  - Exit codes follow the Nagios convention: 0 is `up`; 1 or 2 is `down`, with the first line of stdout as the reason; 3 or any other code, death by signal, or failure to start is `error`.
  - On timeout, the whole process group is killed and the result is `down` with reason `timeout`.
- **External plugins.** A plugin is registered under `[plugins."<name>"]` with a `command`, and services use it as `check = "<name>"`.
  - Each plugin runs as one long-lived child process, shared by every service that uses it. It exchanges line-delimited JSON over stdin and stdout.
  - The protocol, version 1, has three messages: `hello` (handshake with protocol version), `validate` (checks a service's `check_options`), and `check` (carries the target, service, options, and `timeout_ms`). Requests carry an `id`, and answers may arrive in any order.
  - The plugin's stderr is logged by the daemon.
  - A plugin that exits is restarted after a backoff that starts at 1s and doubles up to 30s. While the plugin is unavailable, its checks are `error`.
  - A plugin that does not answer a check within the timeout yields `error`, not `down`. A stuck plugin must not mark every backend unhealthy.
- **Validation and reload.**
  - Startup, `SIGHUP` reload, and `--check-config` start the plugins and have each plugin validate its services' `check_options`.
  - A plugin that fails to start, fails the handshake, or does not answer validation within 5s is a config error, reported together with all other errors.
  - On reload, plugins whose `command` is unchanged keep running. Plugins with a new `command` are started fresh, and services using them restart from `unknown`. Plugins that were removed are stopped.
- **Checker interface.**
  - A checker now receives the service name and the timeout with each check.
  - A checker can choose the result recorded when its check exceeds the timeout. Built-ins keep `down`; plugins use `error`.
- **Docs.**
  - `docs/plugins/protocol-v1.md` specifies the plugin protocol, with example messages in `docs/plugins/protocol-v1-examples.jsonl`. The daemon's tests check those examples.
  - `examples/emfor.toml` shows `exec` and a plugin.

## Non-Goals

- Loading plugins into the daemon process (shared libraries via `dlopen`, WASM). Subprocesses isolate plugin crashes and allow any language.
- A plugin package format, plugin discovery, or a registry of third-party plugins.
- Plugins that are not checkers (for example, custom balance methods or custom DNS answers).
- Re-validating `check_options` after a plugin restart. Options were validated when the config was applied.
- Limiting how many `exec` processes run at once.
- Shipping production plugins (Redis, MySQL, ...). The repository ships only a test plugin used by the test suite.

## Capabilities

### New Capabilities

- `checker-plugins`: Registering external plugins in config, protocol version 1 (handshake, validate, check), process supervision and restart, stderr logging, validation at startup and reload, and how plugins change on reload.

### Modified Capabilities

- `health-checking`: The timeout rule allows a checker to record `error` instead of `down` (used by plugins). Adds the built-in `exec` checker.

## Impact

- Affected specs: `checker-plugins` (new), `health-checking` (modified). `service-config` is not modified. Plugin config rules live in `checker-plugins`, so this change does not touch the requirements that the unarchived `emfor-dns` change modifies.
- Affected code:
  - crates/emfor-daemon/src/checker/mod.rs: checker interface and timeout result.
  - crates/emfor-daemon/src/checker/tcp.rs and crates/emfor-daemon/src/checker/http.rs: adapt to the new interface.
  - crates/emfor-daemon/src/checker/exec.rs (new).
  - crates/emfor-daemon/src/plugin/ (new): protocol, process host, plugin checker.
  - crates/emfor-daemon/src/config.rs: `[plugins]` table, plugin identity in `check_changed`.
  - crates/emfor-daemon/src/schedule.rs: passes the service and timeout.
  - crates/emfor-daemon/src/supervisor.rs.
  - crates/emfor-daemon/src/cli.rs: config loaded inside the async runtime; plugin set kept across reloads.
- New test binary: crates/emfor-daemon/tests/support/test_plugin.rs, built as the `emfor-test-plugin` bin target.
- New docs: docs/plugins/protocol-v1.md, docs/plugins/protocol-v1-examples.jsonl. Updated: examples/emfor.toml, docs/spectra/LANGUAGE.md (if wording needs updating).
- Dependencies: no new crates expected. Uses tokio process, serde_json, and nix (signals and process groups), which the daemon already uses.
