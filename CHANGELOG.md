# Changelog

## 0.1.0 — 2026-11-13

The first release. The daemon, `emfor-check` and every client come from
this one version; `libemfor` and the clients built on it must be upgraded
together with the daemon (they share its LMDB build).

### Daemon

- Health checks: `tcp` with optional `send` / `expect` and TLS,
  `http` / `https`, `exec` for Nagios-style scripts, and long-running
  plugins speaking [protocol v1](docs/plugins/protocol-v1.md). Per-service
  `interval`, `timeout`, `rise`, `fall` and `jitter`, with `[defaults]`.
- State store: a local LMDB file, record format version 1, with a
  heartbeat so readers notice a daemon that stopped. Backend state,
  latency, the last failure reason and the last 20 state changes per
  backend.
- Balance methods: `round-robin`, `sticky` (rendezvous hashing on an
  affinity key), `failover`, `weighted`, `locality` (same `site` first) and
  `fastest` (backends within a latency margin of the quickest).
  `on_all_down = "open"` or `"closed"` decides what an all-down service
  returns.
- DNS server (optional `[dns]`): A, AAAA and SRV answers for its own zone
  (default `svc.emfor`), meant to sit behind the host's resolver; a warning
  when a service's backends use different ports.
- `/metrics` (optional `[metrics]`): Prometheus metrics for the daemon,
  every service and backend.
- `emfor status`: the daemon's state, every service and backend, recent
  state changes and flapping backends; `--service`, `--json` and `--watch`.
- `SIGHUP` reloads the config when it is fully valid; `--check-config`
  checks a config without running.
- `EMFOR_PREFIX` at build time sets the default config path
  (`/etc/emfor/emfor.toml` without it).

### emfor-check

- Plugins for redis, MySQL / MariaDB, PostgreSQL and LDAP, with TLS.
- The `emfor-plugin` crate for writing plugins in Rust.

### Clients

- One implementation in Rust (`emfor-client`), also built with cargo-c as
  the C library `libemfor`.
- Go, Python, Perl and PHP clients wrapping `libemfor`.
- Every client returns `ip:port` or one of the error codes `UNAVAILABLE`,
  `INCOMPATIBLE`, `NOT_FOUND` and `ALL_DOWN`, checked against shared
  conformance data (`tests/conformance/v1`). Configured only by
  `EMFOR_STORE` and `EMFOR_MAX_STALENESS`; safe across threads and `fork`.
- A lookup takes about 1 µs; see [docs/performance.md](docs/performance.md).

### Deployment

- Guides for the daemon, systemd, metrics and alerting
  ([docs/deploy/daemon.md](docs/deploy/daemon.md)), for forwarding the DNS
  zone from dnsmasq, unbound, systemd-resolved and macOS
  ([docs/deploy/dns-resolver.md](docs/deploy/dns-resolver.md)), and for
  `emfor-check` ([docs/deploy/plugins.md](docs/deploy/plugins.md)).
- Examples: a fully commented config, a systemd unit (run under systemd
  257), a FreeBSD rc.d script, and Prometheus alerting rules with promtool
  tests.

### Known limits

- FreeBSD is a deployment platform but has not been tested on a host yet;
  the CI job that covers it is run by hand and has not passed yet.
- Lookup throughput across threads levels off at about two million a
  second on Linux; see [docs/performance.md](docs/performance.md).
