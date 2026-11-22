# 12. Operating emfor

[Contents](README.md) · Previous: [11. Peer groups](11-peer-groups.md) · Next: [13. Security model](13-security.md)

## Commands

| Command | What it does |
| --- | --- |
| `emfor server` | Runs the daemon in the foreground. |
| `emfor check-config [--show]` | Validates the config (and starts each plugin to validate its options); `--show` prints the merged config, each value followed by the file that set it (defaults are not added). |
| `emfor status [--service NAME] [--json] [--watch] [--history]` | Shows what the daemon has published in the store. |
| `emfor lookup SERVICE [--key KEY] [--explain] [--json] [--dns A\|AAAA]` | Looks up one backend as a client library would. |
| `emfor --version` | Prints `emfor` and the version number. |

Every command takes `--config PATH` (default `/etc/emfor/emfor.toml`, or
the one built in, [chapter 4](04-installing.md#the-default-config-path)).
`status` and `lookup` read only `[store] path` from it; `EMFOR_STORE` is not
used by them. They need the same read access to the store as an
application.

## Signals

| Signal | Effect |
| --- | --- |
| `SIGHUP` | Reload the config. Logs `config reloaded: 1 services, 2 backends`, or the errors and `reload rejected; keeping the previous config`. |
| `SIGTERM`, `SIGINT` | Stop: logs `shutting down`, sets the heartbeat to 0 so clients see a stale store at once, stops the plugins, exits 0. |

`systemctl reload emfor` runs `emfor check-config` first and sends `SIGHUP`
only if it passes. What a reload can change is in
[chapter 5](05-configuring.md#applying-a-change-reload).

## emfor status

```console
$ emfor status
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.4s ago, up 1.4s, dns 127.0.0.1:5300, metrics 127.0.0.1:5301

web.svc.emfor  round-robin  on_all_down=open
  127.0.0.1:8080  healthy    1.1s ago  2.2ms
  127.0.0.2:8080  unhealthy  1.0s ago  -      error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)
```

Each backend row has the address, the state, the time of the last check, the
smoothed check latency (`-` when there is none) and, after that, markers:

| Marker | Meaning |
| --- | --- |
| `ALL DOWN` (after the service) | No backend is healthy. |
| `drained` (as state), `(checks: healthy)` | `drain = true`; what the checks say. |
| `pool` | `fastest`: in the fast pool. |
| `backup` | `backup = true`. |
| `last check failed` | Still `healthy`, but the latest check failed. |
| `FLAPPING (5 changes in 10m)` | Five or more state changes in the last ten minutes. |
| `voted` | This host is not one of the backend's assigned checkers; its state can come from their votes. The checkers are listed only by `--json`. |
| `local-up` | A disagreement: down by votes, up here. |
| the reason | Why the latest check failed. |

The `daemon:` line says `running`, `stale, heartbeat 4.5s ago` or
`stopped`. After the clock of the host was set back, the heartbeat can be
ahead of now: the line then says `heartbeat 5.0s in the future`, and it is
`stale` when that is more than 3 seconds. With `[peers]` a `peers:` line follows
([chapter 11](11-peer-groups.md#how-the-votes-work)).

| Option | |
| --- | --- |
| `--service NAME` | Only that service (`service nope.svc.emfor not found`, exit 2). |
| `--history` | The last 20 state changes of each backend, newest first. |
| `--watch` | Refreshes every second, full screen. Keys: PgUp, PgDn or space, Up, Down, Home, End, `q`. |
| `--json` | Everything as one JSON object, for scripts. |

```console
$ emfor status --history
...
web.svc.emfor
  127.0.0.1:8080
    2026-11-21 17:31:25 unknown -> healthy
  127.0.0.2:8080
    2026-11-21 17:31:31 healthy -> unhealthy  error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)
    2026-11-21 17:31:25 unknown -> healthy
```

The JSON object has `store`, `daemon`, `heartbeat_ms`, `heartbeat_age_ms`
(null when stopped, negative when the heartbeat is ahead of now),
`started_at_ms`, `site`, `dns_listen`, `metrics_listen`, `services` (each with
`name`, `balance`, `on_all_down`, `slow_start_ms`, `all_down` and `backends`
with `id`, `ip`, `port`, `state`, `reason`, `checked_at`, `latency_us`,
`weight`, `site`, `backup`, `recovered_at`), `history`, `flapping`, `peers`
and `assigned_checkers` (the assigned checkers of each backend, in rank
order; `null` without a peer group or while the daemon is not running).

Exit status: 0 when the daemon runs and no shown service is all down; 1 when
the daemon is stale or stopped, or a shown service is `ALL DOWN`; 2 when the
config or the store cannot be read. So `emfor status >/dev/null` is a
health check of the host's emfor.

## emfor lookup

```console
$ emfor lookup web.svc.emfor
127.0.0.1:8080
$ emfor lookup web.svc.emfor --explain
```

`--explain` prints the store's freshness, the tier, every candidate, why each
other backend is out, the slow start steps and the balance rule, without
advancing any round-robin counter. `--key` passes an affinity key, and
`--json` prints `service`, `balance`, `on_all_down`, `family`, `via`,
`store` and `result`. When the clock of the host was set back, the heartbeat
can be later than now: the store line then says `heartbeat 1m00s in the
future`, and `store.heartbeat_age_ms` in the JSON is negative.

`--dns A` or `--dns AAAA` does not query the DNS server. It reads the same
store and explains what the daemon's DNS server would answer to that query,
always with the explanation. The daemon updates the store up to about a
second after it decides, so for that second the live DNS answer can differ;
the DNS server's rotation counters are in the daemon's memory and are not
shown.

| Exit | Meaning |
| --- | --- |
| 0 | An address was printed. |
| 1 | `ALL_DOWN`; with `--dns`, also a service with no backend of that family (which the DNS server answers with NOERROR and no records). |
| 2 | No such service, or a bad command line. |
| 3 | The store cannot be used (`UNAVAILABLE`, `INCOMPATIBLE`). |

## Metrics

With a `[metrics]` table, the daemon serves Prometheus text at
`http://127.0.0.1:5301/metrics` (another path gives 404). Keep it on
loopback or behind a firewall: it names every service and backend.

```toml
[metrics]
listen = ["127.0.0.1:5301"]
allow = ["127.0.0.0/8", "10.20.0.0/16"]   # optional
```

`allow` is a list of networks (`address/prefix`, IPv4 or IPv6, no repeats). With
it, a connection from any other source gets `403 Forbidden` and no metrics;
without it every source is served. Nothing is exempt: with `allow` set, list
`127.0.0.0/8` to read the metrics from the host itself. The daemon sees the
address of the direct peer, so behind NAT or a reverse proxy it is the
proxy's address that counts; `X-Forwarded-For` is not read. A change of
`allow` needs a restart, as `listen` does. A `listen` address that is not
loopback and has no `allow` gets a warning at start and after a reload.

| Metric | Labels | Meaning |
| --- | --- | --- |
| `emfor_build_info` | `version` | Always 1. |
| `emfor_start_time_seconds` | | When the daemon started. |
| `emfor_service_all_down` | `service` | 1 while no backend is healthy. |
| `emfor_backend_state` | `service`, `backend`, `state` | 1 for the backend's current state. |
| `emfor_backend_drain` | `service`, `backend` | 1 while drained. |
| `emfor_backend_check_latency_seconds` | `service`, `backend` | Smoothed check time, healthy backends only. |
| `emfor_backend_last_check_timestamp_seconds` | `service`, `backend` | |
| `emfor_backend_checks_total` | `service`, `backend`, `result` | Checks by result (`up`, `down`, `error`). |
| `emfor_backend_transitions_total` | `service`, `backend`, `to` | State changes. |
| `emfor_store_last_commit_timestamp_seconds` | | Should never be more than a second or two old. |
| `emfor_store_commit_failures_total` | | Should stay 0. |
| `emfor_store_used_bytes`, `emfor_store_map_size_bytes` | | Alert well before used reaches the map size. |
| `emfor_plugin_up`, `emfor_plugin_restarts_total` | `plugin` | |
| `emfor_dns_rate_limited_total` | `action` | `truncated`, `dropped`. |
| `emfor_peer_*` | | [Chapter 11](11-peer-groups.md#metrics). |

Ready-made alerting rules are in
[examples/prometheus/emfor-rules.yml](../../examples/prometheus/emfor-rules.yml)
([chapter 14](14-integrations.md#prometheus)).

## Logs

The daemon logs to standard error: the journal under systemd, syslog (tag
`emfor`) under the FreeBSD rc script. `[log] level` sets the level (`trace`,
`debug`, `info`, `warn`, `error`; default `info`); `RUST_LOG` overrides it for
one run. A start and a state change look like this:

```text
INFO emfor_daemon::cli: emfor started: 1 services, 2 backends, store /var/lib/emfor/state.mdb
INFO emfor_daemon::schedule: backend 127.0.0.2:8080 of web.svc.emfor changed from healthy to unhealthy (error sending request: ...) service=web.svc.emfor backend=127.0.0.2:8080 from=healthy to=unhealthy reason=...
```

| Level | What you see |
| --- | --- |
| `error` | Cannot start, invalid config on reload, store commit failures. |
| `warn` | Config warnings (file modes, passwords without TLS, DNS on a public address, mixed ports), a plugin that exited, a damaged store moved aside, dropped peer messages, an unreadable peer secret. |
| `info` | Start, reload and stop, every state change with its reason, plugin start and standard error, peers becoming live or not, peer notices and disagreements. |
| `debug` | Every check, ignored peer messages. |

## Exit codes of the daemon

| Status | Meaning | Restarting helps? |
| --- | --- | --- |
| 0 | Clean stop. | |
| 1 | Cannot start: an address in use (`cannot start metrics server: cannot listen on 127.0.0.1:5301 (tcp): Address already in use (os error 98)`), the store cannot be opened, or another daemon holds it. | No |
| 2 | Invalid config, or no command. | No |
| 70 | An internal failure while running (the state writer or a DNS listener stopped). | Yes |

The systemd unit does not restart on 1 and 2; FreeBSD's `daemon(8)` restarts
on every exit after 2 seconds.

## The store on disk

| File | Mode | |
| --- | --- | --- |
| `state.mdb` | 0640 (`[store] file_mode`) | The data. |
| `state.mdb-lock` | 0660 (`[store] lock_file_mode`) | LMDB's reader table; every reader writes to it. |
| `state.mdb.writer` | 0600 | Locked by the running daemon. |
| `state.mdb-peers` | 0600 | With `[peers]`: the secret record. |
| `state.mdb.corrupt` | | A damaged store that was moved aside. |

- **The daemon does not create the directory.** The packages and the unit
  (`StateDirectory=emfor`) do; for a manual setup, `install -d -o emfor -g
  emfor -m 0755 /var/lib/emfor`. Without it:
  `cannot open state store: /var/lib/emfor/state.mdb.writer: No such file or directory (os error 2)`.
- The daemon commits at least once a second and at every change, on its own
  thread. With `sync = false` (default) it does not wait for the disk, so a
  slow disk cannot make the heartbeat late. Nothing in the store must
  survive a crash: the daemon rewrites every service at start; only the
  history is kept.
- A damaged store found at start is moved to `state.mdb.corrupt`, the lock
  file is deleted, and the daemon starts with a new store. Running
  applications switch to the new file by themselves.
- The quick-start store, with one service, reported
  `emfor_store_used_bytes 32768`. Watch that metric against
  `emfor_store_map_size_bytes` (64 MiB by default): when the store fills up,
  commits fail and clients soon see a stale store. Raising `map_size` needs a
  restart.
- Never change the store in place (`cp` over it, `truncate`, `>`): every
  process that maps it can die with `SIGBUS`. To replace it, write a file
  next to it and `mv` it over.

### Keeping the store in memory

Since nothing in the store must outlive a reboot, it can live on a tmpfs
mounted on the store's directory itself, at least as large as `map_size`:

```text
tmpfs  /var/lib/emfor  tmpfs  size=64m,mode=0755,nosuid,nodev,noexec  0 0
```

Add `RequiresMountsFor=/var/lib/emfor` to the unit with `systemctl edit
emfor`. Do not use `RuntimeDirectory=` (`/run/emfor`): systemd deletes it at
every stop. The history is then lost at every reboot. This setup is taken
from the old manual, which reports a test in a container; it was not run for
this manual.

## Who can read the store

An application needs read access to `state.mdb` and **write** access to
`state.mdb-lock`. The daemon sets the modes at every start:

| Setup | How |
| --- | --- |
| Default | Files owned by `emfor:emfor`, modes 0640 and 0660. Add each application user to the group: `usermod -aG emfor www-data`, then restart the application (group membership is read at login). |
| Every user may read | `file_mode = "0644"`. The lock file must still be writable by every reader. |
| Every user may read, no group | `lock_file_mode = "0666"`. Any local user can then disturb other readers through the lock file; `emfor check-config` warns about it. |

Tested in a container: a user outside the group gets
`UNAVAILABLE: cannot open store /var/lib/emfor/state.mdb: Permission denied`;
after joining the group its lookups work.

The store directory must be searchable (`0755` is fine), and no user other
than the daemon's should be able to write to it.

## What applications see during a restart

A lookup never waits for the daemon; it always answers from the store. But
around a restart the store cannot tell good backends from bad ones for a
moment:

- **While the daemon is stopped**: after a clean stop the store is stale at
  once (heartbeat 0); after a kill, after `EMFOR_MAX_STALENESS` (3 s). An
  `open` service then returns any backend that is not drained, a `closed`
  one fails with `ALL_DOWN`.
- **Just after a start**: every backend is `unknown` until its first checks.
  Fast checks (every 200 ms) settle this within about half a second.

The old manual reports a measurement (Debian container, `tcp` checks,
immediate restart): the last wrong answer came about 0.25 s after the signal,
at most 0.4 s. It was not repeated for this manual.

So:

- Change the config with a **reload**, not a restart: a reload keeps the
  state of every backend whose check did not change.
- Install new packages first, then restart, rather than stop, install, start.
- Restart hosts one at a time; take a host out of its load balancer first if
  it uses `closed`, `failover` or backups.

## A rolling deploy with drain

To deploy backends one at a time without cutting running requests:

1. Set `drain = true` on the backend and reload. New lookups skip it.
2. Wait until its connections are idle. emfor does not see requests; this
   step is yours.
3. Stop, deploy and start the backend.
4. Wait until its checks say `healthy` again (they go on while drained).
5. Remove `drain` and reload.

Give the service its own `conf.d` file with one backend per line, because a
`backends` list is replaced as a whole by the file that sets it:

```toml
# /etc/emfor/conf.d/20-api.toml
[services."api.svc.emfor"]
check = "tcp"
backends = [
  { addr = "10.0.0.11:8080" },
  { addr = "10.0.0.12:8080" },
]
```

[examples/deploy/emfor-drain.sh](../../examples/deploy/emfor-drain.sh) does
steps 1, 2, 4 and 5 (it needs `jq`, and `ss` for `wait-idle`):

```sh
# emfor-drain.sh on /etc/emfor/conf.d/20-api.toml 10.0.0.11:8080        # on the emfor host
# emfor-drain.sh wait-idle 8080                                         # on 10.0.0.11
# deploy-and-restart-api
# emfor-drain.sh wait-ready 10.0.0.11:8080 120                          # on the emfor host
# emfor-drain.sh off /etc/emfor/conf.d/20-api.toml 10.0.0.11:8080
```

- Each backend line must be written exactly `{ addr = "10.0.0.11:8080" },`,
  with a space before `}`; the script refuses other lines and changes nothing.
- It runs `emfor check-config` as the daemon's user (`EMFOR_USER`, default
  `emfor`) and reloads with `systemctl reload emfor` (`EMFOR_RELOAD` for
  another command). It puts the old file back if the check fails.
- `wait-ready` returns only after a check newer than the command, and after
  the checks said `healthy` for SETTLE seconds in a row (default 10). SETTLE
  must exceed `fall × interval + timeout`.
- Drain is per host: with emfor on many application hosts, run `on` on each
  of them (or deploy the same `conf.d` file everywhere).
- Do one backend at a time: a service with every backend drained answers
  `ALL_DOWN`.

The CHANGELOG records that this script was run in a test container; it was
not run for this manual.

## Running more than one daemon on a host

Each store has one writer. A second daemon with the same store fails:
`cannot open state store: store is already in use by another emfor daemon: lock file /var/lib/emfor/state.mdb.writer is held`
(exit 1). For a second daemon (for tests), give it its own `[store] path`,
DNS and metrics addresses, and point its clients at it with `EMFOR_STORE`.
