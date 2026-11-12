## Why

Applications need to know which backend of a service is healthy without routing their traffic through a proxy. emfor is a per-host daemon that runs health checks and publishes results locally; this first change builds the daemon core that every later piece (DNS, plugins, language clients) reads from or plugs into.

## What Changes

- New Rust daemon binary `emfor` that loads a TOML config, health-checks every configured backend on a schedule, and publishes the resulting health state into a local LMDB file.
- TOML configuration with a global `[defaults]` table and per-service overrides for `interval`, `timeout`, `rise`, `fall`, `jitter`, and `on_all_down`; per-check settings live in `check_options` and are validated by the checker that owns them.
- `emfor --check-config` validates a config file and exits; `SIGHUP` reloads the config and applies it only when the whole new config validates.
- A checker interface (`validate` / `check`) returning `up`, `down`, or `error`, with two built-in checkers: `tcp` (connect) and `http` (HTTP/HTTPS status code).
- A per-backend health state machine using haproxy-style `rise` / `fall` thresholds, starting from `unknown` on every daemon start.
- LMDB state store format version 1: single-file environment, default database only, UTF-8 keys and JSON values, `meta/schema_version`, a once-per-second `meta/heartbeat`, per-backend `checked_at`, and a per-service `all_down` flag.
- Single-writer protection so two daemons cannot write the same store.

## Non-Goals (optional)

Scope exclusions are recorded in design.md under Goals / Non-Goals.

## Capabilities

### New Capabilities

- `service-config`: Config file format, defaults and per-service overrides, validation rules, `--check-config`, and `SIGHUP` reload semantics.
- `health-checking`: Checker interface and result kinds, built-in `tcp` and `http` checkers, check scheduling with jitter and timeout, and the `rise` / `fall` health state machine.
- `state-store`: LMDB file layout, key names, JSON record format version 1, heartbeat, `all_down` computation, startup / reload / shutdown behavior of the store, and single-writer protection.

### Modified Capabilities

(none)

## Impact

- New Cargo workspace at the repository root: Cargo.toml, crates/emfor-daemon (binary `emfor`), crates/emfor-format (store key names, record types, schema version; reused later by the Rust client).
- New dependencies: tokio, serde, toml, humantime-serde, heed (LMDB), reqwest with rustls, tracing, tracing-subscriber, clap, rand, thiserror.
- New example config at examples/emfor.toml.
- No existing code, APIs, or specs are affected; the repository has no code yet.
