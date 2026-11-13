# Building emfor and checking what it publishes

## Commands

The `emfor` executable takes a subcommand; `--config <path>` goes before or after it:

| Command | What it does |
| --- | --- |
| `emfor server` | Runs the daemon in the foreground until `SIGTERM` or `SIGINT`; `SIGHUP` reloads the config. |
| `emfor check-config` | Validates the config, starting and stopping any plugins it declares; prints `config ok: ...` and exits 0, or prints each error and exits 2. |
| `emfor status` | Shows what the running daemon publishes (see [emfor status](#emfor-status)). |

`emfor` with no subcommand only prints this list and exits 2: it never starts a daemon. Before 0.2.0, `emfor` without a subcommand started the daemon and `--check-config` checked the config; `--check-config` now only prints `--check-config was replaced by "emfor check-config"` and exits 2.

## Building with an install prefix

The daemon looks for its config at `/etc/emfor/emfor.toml` unless `--config` says otherwise. To install under another prefix, set `EMFOR_PREFIX` when building; the default config path becomes `<prefix>/etc/emfor/emfor.toml`:

```sh
EMFOR_PREFIX=/usr/local cargo build --release --bin emfor
install -m 0755 target/release/emfor /usr/local/sbin/emfor
```

| `EMFOR_PREFIX` | Default config path |
| --- | --- |
| not set | `/etc/emfor/emfor.toml` |
| `/usr` | `/etc/emfor/emfor.toml` |
| `/usr/local` | `/usr/local/etc/emfor/emfor.toml` |
| `/opt/emfor` | `/opt/emfor/etc/emfor/emfor.toml` |

- The prefix must be an absolute path; anything else fails the build.
- `cargo install --root <dir>` only decides where the binary goes. Cargo does not pass it to the build, so set `EMFOR_PREFIX` as well.

With `cargo install`, name each crate: the repository root is a workspace, not a package, so `cargo install --path .` fails with "found a virtual manifest". From an unpacked source release or a checkout:

```sh
EMFOR_PREFIX=/usr/local cargo install --locked --path crates/emfor-daemon --root /usr/local
cargo install --locked --path crates/emfor-check --root /usr/local
```

This installs `emfor` and `emfor-check` into `/usr/local/bin`; nothing else in the crates is installed. Packaging systems that run `cargo install` per crate (for example `CARGO_INSTALL_PATH` in FreeBSD ports) take the same two paths, `crates/emfor-daemon` and `crates/emfor-check`, with `EMFOR_PREFIX` set to the package prefix in the build environment; [examples/freebsd/](../../examples/freebsd/) is an example FreeBSD port that does this.
- `emfor --help` shows the path that was built in, after `--config`.
- Only the config path changes. The store stays at the `[store] path` of the config (default `/var/lib/emfor/state.mdb`), and clients keep their own default; see the client READMEs.

## Running under systemd

[`examples/systemd/emfor.service`](../../examples/systemd/emfor.service) runs the daemon as user `emfor` with the binary at `/usr/local/sbin/emfor` and the config at `/etc/emfor/emfor.toml`:

```sh
cargo build --release --bin emfor
install -m 0755 target/release/emfor /usr/local/sbin/emfor
useradd --system --no-create-home --shell /usr/sbin/nologin emfor
install -m 0644 examples/systemd/emfor.service /etc/systemd/system/emfor.service
systemctl daemon-reload
systemctl enable --now emfor
```

Build without `EMFOR_PREFIX` (or with `/usr`) so that `emfor status` and `emfor check-config` find `/etc/emfor/emfor.toml` without `--config`; the unit passes `--config` itself.

- **Store directory**: `StateDirectory=emfor` creates `/var/lib/emfor`, owned by `emfor` with mode 0755, for the default `[store] path`. Applications that use an emfor client need to reach it; `[store] file_mode` and `lock_file_mode` decide who can read the store. Do not switch to `DynamicUser=`: its directory sits under `/var/lib/private` (mode 0700), where clients cannot reach it.
- **Reload**: `systemctl reload emfor` runs `emfor check-config` first and fails without signalling the daemon if the config is invalid; otherwise it sends SIGHUP. `[store]`, `[dns] listen` and `[metrics] listen` changes need `systemctl restart emfor`.
- **Restarts**: the unit restarts the daemon after a crash, but not after exit status 1 (a listen address in use, the store held by another emfor) or 2 (invalid config), which a retry does not fix.
- **Logs**: the daemon logs to stderr, which goes to the journal (`journalctl -u emfor`), without color escape sequences.
- **Hardening**: the file system is read-only except `/var/lib/emfor`, there is no access to home directories or devices, and no capabilities. Plugins run under the same limits. A `[store] path` elsewhere needs `ReadWritePaths=<its directory>`; a plugin under a home directory needs `ProtectHome=` relaxed. For DNS on port 53, set `CapabilityBoundingSet=CAP_NET_BIND_SERVICE` and `AmbientCapabilities=CAP_NET_BIND_SERVICE`.

Checked with systemd 257 on Debian 13 (arm64), in a container running systemd as PID 1:

- `systemd-analyze verify` prints no warnings, and `systemd-analyze security` rates the unit 1.7 (OK).
- Under the hardening, the daemon checks an HTTP backend and answers on DNS and `/metrics`; a second, unprivileged user reads the store with `emfor status`.
- `systemctl reload` applies a valid change; with an invalid config it fails and the daemon gets no SIGHUP (same PID, no `reload rejected` in its log).
- `systemctl restart` and `stop` work; after `kill -KILL` the daemon is restarted, and when the metrics port is taken it stops with status 1 and is not restarted.
- The journal holds plain log lines: the daemon colors its log only when stderr is a terminal and `NO_COLOR` is not set.

## emfor status

`emfor status` shows what the running daemon has published in the state store. It reads only the `[store]` table of the config (so it still works while the rest of the file has an error), opens the store read-only like a client, and never changes it.

```console
$ emfor status
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.4s ago, up 3h12m, site tpe1, dns 127.0.0.1:5300, metrics 127.0.0.1:5301

api.svc.emfor  round-robin  on_all_down=open
  10.0.0.11:8080  healthy    0.8s ago  1.2ms
  10.0.0.12:8080  unhealthy  0.9s ago  -      connect: Connection refused

db.svc.emfor  failover  on_all_down=open  ALL DOWN
  10.0.3.1:5432   unhealthy  1.1s ago  -      auth: 28P01: password authentication failed for user "emfor"
```

- **daemon line**: `running` while the heartbeat is at most 3 seconds old; `stale` when it is older (the daemon hangs or is gone without a clean shutdown); `stopped` after a clean shutdown. A running daemon adds `dns` and its listen addresses when the config has a `[dns]` table, then `metrics` and its addresses when it has a `[metrics]` table, the addresses separated by spaces, as the daemon wrote them to `meta/dns_listen` and `meta/metrics_listen` at start (so a changed config that has not been applied by a restart does not show). The services shown are what the daemon last wrote; with a stale or stopped daemon, do not trust them.
- **service line**: the `balance`, `on_all_down`, and `ALL DOWN` when no backend is healthy.
- **backend lines**: health state after `rise` / `fall`, time since the last check finished (`never` before the first), the smoothed check latency (`-` when the backend is not healthy), `pool` for the fast backends of a `fastest` service, `FLAPPING (<n> changes in 10m)` for a flapping backend (see [State history](#state-history)), and the message of the last failed check. The message is cleared by the next passing check, which can happen before the backend counts as healthy again (`rise`).

On a terminal the states are colored; set `NO_COLOR` to turn that off. Piped output has no escape sequences.

### Options

| Option | Effect |
| --- | --- |
| `--config <path>` | Config file to take `[store] path` from |
| `--service <name>` | Show one service |
| `--json` | One JSON object: `store`, `daemon` (`running`, `stale`, `stopped`), `heartbeat_ms`, `heartbeat_age_ms`, `started_at_ms`, `site`, `dns_listen` and `metrics_listen` (the `[dns]` and `[metrics]` listen addresses, or `null`), `services` (the store's service records as written), `history` (service name → its `hist/` value as written) and `flapping` (`[{"service", "backend", "changes"}]`) |
| `--watch` | Redraw every second until Ctrl-C; needs a terminal, not with `--json` |
| `--history` | List each backend's recorded state changes, newest first; not with `--json` or `--watch` |

### Exit status

| Status | When |
| --- | --- |
| 0 | The daemon is running and every shown service has a healthy backend |
| 1 | The daemon is stale or stopped, or a shown service is `ALL DOWN` |
| 2 | The config or store cannot be read, the store has another format version, or `--service` names no service |

A single unhealthy or flapping backend does not change the status: failing over is the daemon's job. This makes `emfor status` usable as a monitoring check as it is, for example from cron or a Nagios-style plugin; add `--service` to alert on one service only.

### Watching

`emfor status --watch` redraws every second. When a backend's state or failure message changes, or a service turns `ALL DOWN` or back, that row is shown in reverse video for one frame. Below the services it lists the last 10 changes, newest first: backend state changes from the stored [state history](#state-history), so changes from before `--watch` started are shown too, and daemon state changes it has seen since it started. Changes from an earlier day carry the date:

```
recent changes:
  14:03:12 api.svc.emfor 10.0.0.12:8080 healthy -> unhealthy
  14:01:40 daemon stopped -> running
  2026-11-12 23:50:00 api.svc.emfor 10.0.0.12:8080 unhealthy -> healthy
```

### State history

For each service the daemon keeps a `hist/<service>` key next to `svc/<service>`, holding the last 20 state changes of each backend that a check caused, written in the same transaction as the record. Resets to `unknown` (at start, or when `check` / `check_options` change on reload) are not recorded. The history survives restarts; at start and on reload the daemon drops the history of services and backends no longer in the config, and a value it cannot parse is logged as a warning and dropped. Clients and the DNS server do not read these keys. Each entry is about 90 bytes, so a service with 10 backends uses at most about 18 KB.

```console
$ emfor status --history --service api.svc.emfor
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.4s ago, up 3h12m, site tpe1

api.svc.emfor
  10.0.0.11:8080
    no changes recorded
  10.0.0.12:8080
    2026-11-13 14:03:12 healthy -> unhealthy  connect: Connection refused
    2026-11-13 14:02:50 unknown -> healthy
```

A backend with 5 or more changes between `healthy` and `unhealthy` in the last 10 minutes is flapping: `emfor status` marks its line with `FLAPPING (<n> changes in 10m)` and `--json` lists it under `flapping`. This is for display only; it changes neither the exit status nor what the daemon does.

## Metrics

With a `[metrics]` table the daemon serves Prometheus metrics at `http://<listen>/metrics`. Without the table it opens no metrics socket.

```toml
[metrics]
listen = ["127.0.0.1:5301"]  # the default; several addresses allowed
```

At start the daemon logs `metrics on <addresses>`. If an address cannot be bound it exits with status 1 and names the address. Changing `listen` requires a restart: a reload with a different `listen` is rejected and the old config stays.

The server answers only `GET /metrics` (a query string is ignored): other paths get 404, other methods 405, and a malformed request or a request head over 8192 bytes 400. Right after start, until the daemon has set up its services, `GET /metrics` gets 503, so Prometheus records one failed scrape instead of a start time of 0 and no services. A client that has not sent its request within 5 seconds is disconnected; at most 16 connections are served at once. Every response closes the connection. The values come from the daemon's memory, the same state the DNS server answers from, not from the store.

### Metrics served

| Metric | Type | Labels | Value |
| --- | --- | --- | --- |
| `emfor_backend_state` | gauge | `service`, `backend`, `state` | 1 for the current state (`unknown`, `healthy`, `unhealthy`), 0 for the other two |
| `emfor_backend_check_latency_seconds` | gauge | `service`, `backend` | Smoothed check latency; only while the backend has one (healthy) |
| `emfor_backend_last_check_timestamp_seconds` | gauge | `service`, `backend` | Unix time the last check finished; after the first check |
| `emfor_backend_in_pool` | gauge | `service`, `backend` | 1 in the pool of a `fastest` service, else 0; other services have none |
| `emfor_service_all_down` | gauge | `service` | 1 when no backend is healthy |
| `emfor_backend_checks_total` | counter | `service`, `backend`, `result` | Checks by result: `up`, `down`, `error` |
| `emfor_backend_transitions_total` | counter | `service`, `backend`, `to` | State changes by new state: `unknown`, `healthy`, `unhealthy` |
| `emfor_build_info` | gauge | `version` | Always 1 |
| `emfor_start_time_seconds` | gauge | | Daemon start time, Unix seconds |
| `emfor_store_commit_failures_total` | counter | | Failed store commits since start |

The two counters start at 0 when a backend first appears in the running daemon, keep counting across reloads (also when `check` or `check_options` change), and start again from 0 if the backend is removed and added back. They restart with the daemon; Prometheus handles that as a counter reset.

Services are listed by name and backends in config order. Measured with one service of 10 unreachable backends: 104 series and 10.6 KB per scrape, that is 10 series per backend plus 4 for the daemon and the service (the test `series_for_ten_backends` in `crates/emfor-daemon/tests/metrics.rs` keeps it so). A backend has no last-check series before its first check, a healthy backend adds the latency series, and a backend of a `fastest` service adds `in_pool`, so a backend has 9 to 12 series.

### Scraping

```yaml
scrape_configs:
  - job_name: emfor
    scrape_interval: 15s
    static_configs:
      - targets: ["127.0.0.1:5301"]
```

The default address is loopback only, for a Prometheus or agent on the same host. To scrape from elsewhere, listen on an address the scraper can reach and allow only the scraper's address in the host firewall: the endpoint has no authentication and no TLS, and it shows every service name and backend address.

### Alerting rules

[`examples/prometheus/emfor-rules.yml`](../../examples/prometheus/emfor-rules.yml) has alerting rules for a scrape job named `emfor`:

| Alert | Fires when | Severity |
| --- | --- | --- |
| `EmforMetricsDown` | The endpoint cannot be scraped for 2 minutes | critical |
| `EmforServiceAllDown` | A service is `ALL DOWN` for 1 minute | critical |
| `EmforStoreCommitFailing` | A store commit failed in the last 5 minutes; clients soon see a stale heartbeat | critical |
| `EmforBackendUnhealthy` | A backend is unhealthy for 10 minutes | warning |
| `EmforBackendFlapping` | 5 or more changes between `healthy` and `unhealthy` in 10 minutes, like the `FLAPPING` mark of `emfor status` | warning |
| `EmforBackendCheckErrors` | More than half of a backend's checks in 10 minutes end in `error` (the check could not decide), for 10 minutes | warning |
| `EmforBackendNotChecked` | A backend's last check is over 5 minutes old, for 5 minutes | warning |

The durations are starting points; set them from the services' `interval`, `rise` and `fall`. `EmforBackendNotChecked` fires on services whose `interval` is close to 5 minutes, so raise its threshold if you have any. [`emfor-rules.test.yml`](../../examples/prometheus/emfor-rules.test.yml) next to it holds unit tests for the rules. After you change the rules, run both commands below; they were last checked with promtool 3.5.0:

```sh
promtool check rules examples/prometheus/emfor-rules.yml
promtool test rules examples/prometheus/emfor-rules.test.yml
```
