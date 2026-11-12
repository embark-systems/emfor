## Context

The repository has no code yet. emfor is a per-host daemon: every application host runs one copy, which health-checks backends and publishes results locally. Applications never send traffic through emfor. Two read paths will consume the published state in later changes: language clients that read the local LMDB file directly (`emfor-clients`) and a DNS server bound to 127.0.0.1 (`emfor-dns`). External checker plugins arrive in `emfor-plugins`.

This change builds the part all of those depend on: config loading, scheduling, the checker interface with the `tcp` and `http` checkers, the health state machine, and the LMDB writer. Terminology follows docs/spectra/LANGUAGE.md (service, backend, checker, check result, health state, heartbeat, all down).

Because clients in six languages will parse the LMDB file directly, the store format defined here is a cross-language contract and is versioned from day one.

## Goals / Non-Goals

**Goals:**

- A single `emfor` binary that runs from a TOML config, checks every backend, and keeps the LMDB store current.
- A checker interface that the `emfor-plugins` change can extend without changing the scheduler or state machine.
- LMDB store format version 1, readable by any LMDB binding (including PHP `dba`), with enough metadata for a reader to detect a dead daemon.
- Safe config reload: a bad config never replaces a good one.

**Non-Goals:**

- DNS server (change `emfor-dns`).
- External plugin protocol and the `exec` checker (change `emfor-plugins`). In this change an unknown `check` type is a validation error.
- Language clients, `emfor_lookup`, the `affinity_key` algorithm, and the shared cross-language test fixtures (change `emfor-clients`).
- Synchronizing state between emfor instances on different hosts.
- Per-backend overrides of check settings; weights; backends given as hostnames instead of IP literals.
- systemd unit files, packaging, and daemonizing (the process runs in the foreground; supervision is the service manager's job).

## Decisions

### Cargo workspace with a separate format crate

The repository becomes a Cargo workspace with `crates/emfor-daemon` (binary `emfor`) and `crates/emfor-format` (key names, JSON record structs, `SCHEMA_VERSION`). The format crate has no tokio or heed dependency so the future Rust client can depend on it without pulling in the daemon.

Alternative considered: one crate, extract the format later. Rejected because the extraction would touch every store test, and the split costs almost nothing now.

### TOML config with defaults and per-service overrides

Config is TOML. The parsed `toml::Table` is walked key by key by a validator that collects every violation with its TOML path, and any key not on the allowed list of its table is an error, so typos fail validation instead of being ignored. (serde with `deny_unknown_fields` was the first plan; it was dropped because serde stops at the first error, and the spec requires every violation to be reported.) Durations are strings with units parsed by `humantime` (`"500ms"`, `"2s"`). Service tables are keyed by the service hostname, which is normalized to lowercase with any trailing dot removed.

Alternative considered: YAML. Rejected: TOML is the Rust ecosystem default and has no implicit typing surprises.

### Checker trait with validate and check

```rust
trait Checker: Send + Sync + Debug {
    fn check(&self, target: SocketAddr) -> BoxFuture<'_, CheckResult>;
    /// Drop any per-backend resources; called when a reload removes a backend
    /// from a service whose checker is kept. Default: no-op.
    fn forget(&self, target: SocketAddr) {}
}
trait CheckerFactory: Send + Sync {
    fn kind(&self) -> &'static str;                       // "tcp", "http"
    // `options` is the `check_options` table (empty when absent). Each error
    // carries a key path relative to `check_options`; the config validator
    // prefixes it with `services."<name>".check_options.`.
    fn validate(&self, options: &toml::Table) -> Result<Arc<dyn Checker>, Vec<OptionError>>;
}
enum CheckResult { Up, Down(String), Error(String) }
```

A registry maps the `check` string to a factory. `validate` both checks options and builds the checker, so a config that validates is guaranteed to be runnable. The scheduler wraps every `check` future in `tokio::time::timeout`; checkers do not need to enforce the timeout themselves, and a timed-out check is `Down("timeout")`.

`Error` means the failure is on the emfor side, not the backend: for `tcp` and `http` this is limited to local resource errors when creating the socket or client (for example `EMFILE`). The plugin change will use `Error` for plugin crashes.

Alternative considered: a plain enum of built-in checks. Rejected because the plugin change needs an open set.

### Health state machine with rise and fall

Each backend has a health state in {`unknown`, `healthy`, `unhealthy`} and two counters.

- `Up` increments the success counter and resets the failure counter; when the success counter reaches `rise`, the state becomes `healthy`.
- `Down` increments the failure counter and resets the success counter; when the failure counter reaches `fall`, the state becomes `unhealthy`.
- `Error` changes neither counter and does not change the state. If a backend has received only `Error` results for longer than `fall × interval`, its state becomes `unknown` and both counters reset.
- Every backend starts as `unknown` with both counters at zero.

### One task per backend for check scheduling

Each backend gets its own tokio task that loops: wait, run one check, feed the result to the state machine. Because a backend's checks run sequentially in one loop, two checks on the same backend never overlap. The first check runs after a random delay in `[0, interval)` so that many hosts starting together do not check in lockstep. Each later wait is `interval` adjusted by a uniformly random offset of up to `±jitter` (a percentage of `interval`).

Alternative considered: a central timer wheel. Rejected as unnecessary for the expected scale (an estimate of at most a few thousand backends per host; not measured).

### Single writer task for the LMDB store

All store writes go through one writer task fed by a channel. It commits immediately when any backend's health state changes, and once per second (the heartbeat tick) it commits the heartbeat plus every service whose `checked_at` values changed. One write transaction per commit.

The store opens with `MDB_NOSUBDIR` (one data file plus `<path>-lock`), uses only the unnamed default database, and stores UTF-8 keys with JSON values. This is what PHP `dba`'s lmdb handler can read, and the other planned bindings can read it too.

Alternative considered: named sub-databases per service. Rejected because PHP `dba` can only read the default database.

### Store record format version 1

Keys:

| Key | Value |
| --- | --- |
| `meta/schema_version` | `"1"` |
| `meta/heartbeat` | Unix time in milliseconds as a decimal string; `"0"` after a clean shutdown |
| `meta/started_at` | Unix time in milliseconds when the daemon started |
| `svc/<service name>` | JSON service record, below |

Service record:

```json
{
  "name": "api.svc.emfor",
  "on_all_down": "open",
  "all_down": false,
  "backends": [
    {
      "id": "10.0.0.11:8080",
      "ip": "10.0.0.11",
      "port": 8080,
      "state": "healthy",
      "checked_at": 1762905600123,
      "reason": null
    }
  ]
}
```

`backends` keeps config order. `id` is the backend string exactly as written in config after normalization (IPv6 as `[addr]:port`). `checked_at` is the Unix-ms end time of the last completed check, or `null` before the first check. `reason` is the message of the last `Down` or `Error`, or `null` after an `Up`. `all_down` is `true` when no backend in the service is `healthy` (so it is `true` right after startup until backends reach `rise`).

### Store lifecycle on start, reload, and shutdown

- Start: acquire the writer lock, open the store, write `meta/schema_version` and `meta/started_at`, delete every `svc/*` key, write a fresh record for each configured service with all backends `unknown`, then start checking. State from a previous run is never reused.
- Reload: services removed from config are deleted from the store. New services and new backends start `unknown`. A service whose `check` type or `check_options` changed has all its backends reset to `unknown`. Backends whose check settings are unchanged keep their state and counters even if `interval`, `timeout`, `rise`, `fall`, or `jitter` changed.
- Clean shutdown (`SIGTERM` or `SIGINT`): stop check tasks, write `meta/heartbeat = "0"` so readers immediately treat the state as stale, close the store, exit 0.

### Writer lock against a second daemon

Before opening the store the daemon takes an exclusive `flock` on `<store path>.writer`. If the lock is held, it exits with status 1 and an error that names the lock file. LMDB itself allows only one write transaction at a time but would let two daemons overwrite each other's records alternately.

### HTTP checker uses reqwest with rustls

The `http` checker sends one `GET` per check using reqwest built with rustls (no OpenSSL dependency), always connects to the backend IP and port, and sets the `Host` header from `host_header` when given. When `host_header` names a domain, the request URL uses that name and the client resolves it to the backend address, so TLS sends the right SNI and verifies the certificate against the name; when `host_header` is an IP literal, the URL uses the backend address and only the `Host` header carries the value. The checker keeps one client per backend and releases it through `forget` when a reload removes that backend. Redirects are not followed; a redirect status is judged against `expect_status` like any other status.

### Reload and shutdown via unix signals

`SIGHUP` triggers reload; `SIGTERM` and `SIGINT` trigger clean shutdown. Signals are handled with `tokio::signal::unix`. Logs go to stderr through `tracing`, at the level from `[log] level` (default `info`), overridable by `RUST_LOG`. A reload applies a changed `[log] level` (unless `RUST_LOG` is set). On reload a changed `[store]` table is reported together with every other validation error.

## Implementation Contract

**Command line**

- `emfor [--config <path>]` runs the daemon in the foreground. Default config path `/etc/emfor/emfor.toml`.
- `emfor --check-config [--config <path>]` validates and exits: status 0 and prints `config ok: <n> services, <m> backends` on success; status 2 and prints every validation error (one per line, prefixed with the TOML path such as `services."api.svc.emfor".timeout`) on failure. It never opens the store.
- Exit status of the daemon: 0 after clean shutdown; 1 on startup failure (store cannot be opened, writer lock held); 2 on invalid config at startup.

**Config file shape**

```toml
[store]
path = "/var/lib/emfor/state.mdb"    # default
map_size = "64MiB"                   # default
file_mode = "0644"                   # default, data file
lock_file_mode = "0666"              # default, <path>-lock; readers must be able to write it

[log]
level = "info"                       # default

[defaults]
interval = "2s"
timeout = "1s"
rise = 2
fall = 3
jitter = "10%"
on_all_down = "open"                 # "open" | "closed"

[services."api.svc.emfor"]
check = "http"                       # "tcp" | "http"
backends = ["10.0.0.11:8080", "[2001:db8::1]:8080"]
interval = "1s"                      # optional override of any [defaults] key

[services."api.svc.emfor".check_options]
path = "/healthz"                    # default "/"
scheme = "http"                      # "http" | "https", default "http"
host_header = "api.internal"         # optional
expect_status = ["200-299"]          # default ["200-399"]; items are codes or "lo-hi" ranges
tls_verify = true                    # default true, https only
```

The `[defaults]` values shown are the built-in defaults used when `[defaults]` or a key in it is absent. `tcp` accepts no `check_options` keys.

**Validation rules** (all violations are reported together, not just the first)

- `interval` ≥ 100ms; `timeout` > 0 and `timeout` ≤ `interval` (equal is allowed: a backend's checks run sequentially in one loop, so they cannot overlap); `rise` ≥ 1; `fall` ≥ 1; `jitter` between `0%` and `50%`.
- Service name: 1–253 characters, labels of `[a-z0-9-]` 1–63 characters not starting or ending with `-`, after lowercase normalization. Two names that normalize to the same value are a duplicate error.
- `backends`: non-empty; each entry is `IPv4:port` or `[IPv6]:port` with port 1–65535; duplicates within a service are an error.
- `check` must name a registered checker; `check_options` must pass that checker's `validate`.
- Unknown keys anywhere are errors.
- On reload, a changed `[store]` table is an error ("store settings require restart").

**Failure modes**

- Reload with an invalid config: the old config keeps running; each validation error is logged at `error` level; nothing in the store changes.
- LMDB write fails (for example map full): logged at `error`; the writer retries on the next tick; because the heartbeat is not updated, readers see the state go stale after their staleness threshold.
- A backend check that exceeds `timeout` is recorded as `Down("timeout")`.

**Acceptance criteria**

- `cargo test --workspace` passes, including integration tests that start the daemon against local TCP and HTTP test servers and read the LMDB file with heed.
- A backend that goes from accepting to refusing connections becomes `unhealthy` in the store after `fall` checks, and back to `healthy` after `rise` checks.
- After `kill -TERM`, `meta/heartbeat` reads `"0"`.
- Starting a second daemon with the same `[store] path` exits 1 while the first runs.
- `emfor --check-config` on examples/emfor.toml exits 0.

**Scope boundaries**

In scope: everything above. Out of scope: anything listed under Non-Goals, including DNS, plugins, `exec`, clients, and the cross-language fixtures.

## Risks / Trade-offs

- [The store format becomes a contract for six client languages before any client exists] → Version it with `meta/schema_version`, keep the record shape small, and let `emfor-clients` add golden fixtures generated from this writer.
- [Default `lock_file_mode` 0666 lets any local user register as a reader, which could fill the reader table] → Make it configurable; document tightening it to a shared group (0664 plus a group) in examples/emfor.toml.
- [Every host checks every backend, so backend check load grows with host count] → Random first-check delay and jitter spread the load; per-check cost of `tcp` and `http` is small. Not measured; revisit if a deployment reports check load.
- [Rewriting a whole service record once per second when `checked_at` changes] → Records are small (estimated well under 4 KiB for a 20-backend service; not measured); measure write volume in the integration test if it becomes a concern.
- [PHP `dba` compatibility of the store is assumed, not yet verified] → `emfor-clients` verifies it first and can fall back to FFI; the store layout here (single file, default database) is chosen to maximize the chance it works.

## Migration Plan

New project; nothing to migrate. Rollback is stopping the daemon; readers will see the heartbeat go to `"0"`.

## Open Questions

None blocking this change. Whether DNS needs AAAA and SRV records is decided in `emfor-dns`.
