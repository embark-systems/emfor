# 8. Operations

[Manual](README.md) · Previous: [7. DNS](07-dns.md) · Next: [9. Troubleshooting](09-troubleshooting.md)

## Commands

| Command | What it does |
| --- | --- |
| `emfor server` | Runs the daemon in the foreground. |
| `emfor check-config` | Validates the config and exits 0 (valid) or 2 (invalid). |
| `emfor status` | Shows what the running daemon has published. |
| `emfor --help` | Lists the commands and the default config path. |

`--config <path>` goes before or after the command. `emfor` with no
command only prints its usage and exits 2; it never starts a daemon.

## Signals

| Signal | Effect |
| --- | --- |
| `SIGHUP` | Reload the config. Applied only when the whole file is valid; otherwise the errors are logged and the old config keeps running. See [what a reload changes](03-configuration.md#what-a-reload-changes). |
| `SIGTERM`, `SIGINT` | Stop: checks end, the heartbeat is set to 0 (clients see a stale store at once), plugins are stopped, exit 0. A reload or start that is still loading the config is abandoned. |

Use your service manager rather than signals: `systemctl reload emfor` and
`service emfor reload` check the config first and do not signal the daemon
when it is invalid.

## emfor status

```console
$ emfor status
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.4s ago, up 3h12m, site tpe1, dns 127.0.0.1:5300, metrics 127.0.0.1:5301

api.svc.emfor  round-robin  on_all_down=open
  10.0.0.11:8080  healthy    0.8s ago  1.2ms
  10.0.0.12:8080  unhealthy  0.9s ago  -      connect: Connection refused
```

`emfor status` reads only the `[store]` table of the config and opens the
store read-only, like a client; any user who can read the store can run
it.

- **daemon**: `running` while the heartbeat is at most 3 seconds old,
  `stale` when it is older (the daemon hangs or died), `stopped` after a
  clean shutdown. With `stale` or `stopped`, the services shown are only
  what the daemon wrote last.
- **service**: `balance`, `on_all_down`, and `ALL DOWN` when no backend is
  healthy.
- **backend**: state, time since the last check, smoothed latency, `pool`
  for the fast backends of a `fastest` service, `FLAPPING (<n> changes in
  10m)` for a backend that changed between healthy and unhealthy 5 or more
  times in 10 minutes, and the reason of the last failed check.

| Option | Effect |
| --- | --- |
| `--service <name>` | Show one service. |
| `--json` | One JSON object, for scripts. |
| `--watch` | Redraw every second, highlight changes, and list the last 10 changes. The `store:` and `daemon:` lines stay at the top; scroll a long list with PgUp/PgDn, Up/Down and Home/End. Lines wider than the terminal are cut. `q` or Ctrl-C ends it. |
| `--history` | List each backend's last 20 state changes (kept across restarts). |

| Exit status | When |
| --- | --- |
| 0 | The daemon is running and every shown service has a healthy backend. |
| 1 | The daemon is stale or stopped, or a shown service is `ALL DOWN`. |
| 2 | The config or store cannot be read, the store has another format version, or `--service` names no service. |

One unhealthy backend does not change the status, so `emfor status` (or
`emfor status --service <name>`) works as a monitoring check from cron or
a Nagios-style system as it is. More in
[docs/deploy/daemon.md](../deploy/daemon.md#emfor-status).

## Metrics

With a `[metrics]` table the daemon serves Prometheus metrics at
`http://127.0.0.1:5301/metrics`. The endpoint has no authentication and
no TLS; keep it on loopback or firewall it.

| Metric | Meaning |
| --- | --- |
| `emfor_backend_state{service,backend,state}` | 1 for the current state |
| `emfor_backend_check_latency_seconds` | Smoothed check latency (healthy backends) |
| `emfor_backend_last_check_timestamp_seconds` | When the last check finished |
| `emfor_backend_in_pool` | 1 in the pool of a `fastest` service |
| `emfor_service_all_down` | 1 when no backend is healthy |
| `emfor_backend_checks_total{result}` | Checks by result: `up`, `down`, `error` |
| `emfor_backend_transitions_total{to}` | State changes by new state |
| `emfor_store_commit_failures_total` | Failed store commits |
| `emfor_store_last_commit_timestamp_seconds` | Last successful commit; should be under a few seconds old |
| `emfor_store_used_bytes`, `emfor_store_map_size_bytes` | Store use against `[store] map_size` |
| `emfor_plugin_up`, `emfor_plugin_restarts_total` | Plugin health |
| `emfor_peer_up{peer}` | With `[peers]`: 1 while that member is live |
| `emfor_peer_messages_rejected_total{reason}` | With `[peers]`: dropped messages by reason: `malformed`, `auth` (wrong secret), `sender` (not a member, or a spoofed address), `site`, `stale` (clocks more than 5 seconds apart), `replay` |
| `emfor_peer_notifications_total{direction}` | With `[peers]`: failure reports `sent` (one per member) and `received` |
| `emfor_build_info{version}`, `emfor_start_time_seconds` | The daemon |

[examples/prometheus/emfor-rules.yml](../../examples/prometheus/emfor-rules.yml)
has alerting rules: the endpoint is down, a service is `ALL DOWN`, store
commits fail, a backend is unhealthy for 10 minutes, flaps, mostly errors,
or is no longer checked. The full list of metrics and rules is in
[docs/deploy/daemon.md](../deploy/daemon.md#metrics).

### Peer groups

Members of a peer group need UDP between each other on their `[peers]
listen` port (5302 in the examples) and clocks within 5 seconds of each
other (NTP). If `emfor_peer_up` stays 0 for a member, look at
`emfor_peer_messages_rejected_total` on both sides: `auth` means the
secrets differ, `stale` that the clocks drift, `sender` that `members` or
`listen` do not match, `site` that the hosts have different `site` values.

## Logs

The daemon logs to stderr: the journal under systemd, syslog (tag `emfor`)
under the FreeBSD rc.d script. Set the level with `[log] level`, or for
one run with `RUST_LOG` (for example `RUST_LOG=debug`). Colors are used
only when stderr is a terminal and `NO_COLOR` is not set.

| Level | What you see |
| --- | --- |
| `error` | Store commit failures, rejected reloads |
| `warn` | Plugin exits, damaged store replaced, services outside the DNS zone or with mixed ports, unparsable plugin output, dropped peer messages (one line per source and reason a minute), an unreadable `[peers] secret_file` |
| `info` | Start and stop, every backend state change with its reason, plugin stderr lines, peer members becoming live or not live, `peer … reports backend … down (…); checking now` |
| `debug` | The duration of every passed check, peer reports that were ignored and why |

## Exit codes

| Status | Meaning | Retry helps? |
| --- | --- | --- |
| 0 | Clean stop (`SIGTERM` / `SIGINT`) | — |
| 1 | Cannot start: a listen address is in use, the store cannot be opened, or another emfor holds the store (`<store>.writer`) | No; fix the cause |
| 2 | Invalid config, or no command given | No; fix the config |
| 70 | The state writer stopped (a bug) | Yes; the systemd unit restarts it |

## The store on disk

The daemon commits to the store at least once a second and on every state
change. By default (`[store] sync = false`) a commit does not wait for the
disk, so a stalled disk cannot make the heartbeat late and the store look
stale to every application on the host. Nothing in the store needs to
survive a host crash: the daemon rewrites every service at start. Only the
state history (`emfor status --history`) is kept across restarts.

After a power loss the store may be damaged. The daemon then logs a
warning, moves it to `<store>.corrupt`, and starts with a new store;
running applications follow the new file by themselves. Set `sync = true`
to flush every commit, at the risk described above.

Commits run on their own thread, so a slow disk does not delay DNS
answers, metrics or checks. Watch `emfor_store_used_bytes` against
`map_size`: when the store is full, commits fail and clients soon see a
stale store.

## Who can use the store

Applications read the data file and register their reads in the lock file
`<store>-lock`, so every application user needs read access to the store
and write access to the lock file. The defaults (`file_mode = "0644"`,
`lock_file_mode = "0666"`) let every local user do this, and also let any
local user who writes the lock file stall the daemon's commits or disturb
lookups. Keep the defaults only on hosts where every local user is trusted.
Otherwise, give your applications a group:

1. Create a group, for example `emfor-clients`, and add each application
   user to it.
2. Make it the daemon's group. Under systemd: `systemctl edit emfor`, add
   `Group=emfor-clients` and set `StateDirectoryMode=0750`. Under FreeBSD:
   `chgrp emfor-clients /var/lib/emfor && chmod 0750 /var/lib/emfor`.
3. In `[store]`, set `file_mode = "0640"` and `lock_file_mode = "0660"`.
4. `chgrp emfor-clients` the existing store and lock files, and restart the
   daemon (it sets the modes at every start).

## Running more than one daemon

Each store has one writer: the daemon locks `<store>.writer`, and a second
daemon with the same `[store] path` exits with status 1. To run two
daemons on one host (for testing), give them different store paths, DNS
ports and metrics ports.

## Upgrading

Install the daemon and every client library from the same release, then
restart the daemon. Applications keep running; they follow the restarted
daemon's store without a restart of their own.

**From 0.1.x to 0.2.0**: the daemon is now started with a command. Change
your service definition before restarting:

| 0.1.x | 0.2.0 |
| --- | --- |
| `emfor --config X` | `emfor server --config X` |
| `emfor --check-config --config X` | `emfor check-config --config X` |

The store format and the plugin protocol are unchanged. Replace copies of
the 0.1.x systemd unit and rc.d script with the new examples. See
[CHANGELOG.md](../../CHANGELOG.md) for everything that changed.
