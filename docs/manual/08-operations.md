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
| 2 | The config or store cannot be read, the store has another format version, `--service` names no service, or the output cannot be written. |

A pipe closed early (`emfor status | head -1`) does not change the exit
status. `--watch` opens the store again when the file is replaced (as the
daemon does with a damaged store at start).

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
| `emfor_peer_messages_rejected_total{reason}` | With `[peers]`: dropped messages by reason: `malformed`, `auth` (wrong secret, or a message that does not decrypt), `sender` (not a member, or a spoofed address), `site`, `stale` (clocks more than 5 seconds apart), `replay` |
| `emfor_peer_notifications_total{direction}` | With `[peers]`: failure reports `sent` (one per member) and `received` |
| `emfor_peer_results_total{direction}` | With `[peers]`: check results of assigned checkers `sent` (one per member) and `received` |
| `emfor_peer_disagreements` | With `[peers]`: backends the group votes down while this host's own check passes |
| `emfor_peer_checks_skipped_total` | With `[peers]`: checks this host did not make because two assigned checkers agreed |
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
`malformed` on a member that runs 0.3.0 means it cannot read the
encrypted messages of upgraded members: set `encrypt = false` on those
until it is upgraded (see
[`[peers]`](03-configuration.md#peers)). A warning `peer <address> sends
plaintext messages (EMP1); set encrypt = true there` means that member
still sends readable messages; they are accepted.

Reasons tell where a `down` came from: `peers 2/3: <reason>` means two of
the three assigned checkers saw the backend down (the reason is theirs),
`local: <reason>` that only this host sees it down while the vote did not
say down, and a reason without a prefix that there was no vote (fewer than
three live members, or no recent result). `emfor status` shows the group
under the daemon line:

```
peers: 1/2 live, not live 10.0.0.7:5302
peers: check definition of api.svc.emfor differs at 10.0.0.6:5302
```

and marks backends with `local-up` when the group votes them down while
this host's own check passes. Those are worth a look: this host reaches a
backend the others do not (often a network partition or a firewall rule on
the other hosts), and `emfor_peer_disagreements` counts them. A "check
definition differs" line means that member's `check`, `check_options` or
`timeout` for the service differ from this host's, so its results are not
counted; make the configs match. The same data is in the store key
`meta/peers` and in `emfor status --json` as `peers`. Its `services_off`
lists the services this host leaves out of the group (`peers = false`);
their backends are never `voted` and have no `assigned_checkers`. Each
entry of `members` has its own `services_off`: the services of this host
that the member announces it leaves out. That member is not counted as
an assigned checker of those services here, and `emfor status` leaves it
out the same way. The text output shows this as a line per member after
the `peers:` line, listing only the services shown:

```
peers: 10.0.0.6:5302 leaves out api.svc.emfor db.svc.emfor
```

In a group of four or more, `emfor_peer_checks_skipped_total` grows on
hosts that are not an assigned checker of some backends: they let the
votes decide most check times and check those backends themselves about
every third `interval`. `emfor status` marks those backends `voted` and
ends the `peers:` line with how many of the shown backends this host
checks every time:

```
peers: 4/4 live, checks 12/40 backends here
```

`emfor status --json` lists each backend's assigned checkers under
`assigned_checkers`, in rank order:

```json
"assigned_checkers": [
  {"service": "api.svc.emfor", "backend": "10.0.0.11:8080",
   "checkers": ["10.0.0.5:5302", "10.0.0.8:5302", "10.0.0.6:5302"]}
]
```

It is `null` while the daemon is not running or `meta/peers` is missing.
status works these out from the live members in
`meta/peers`, which the daemon updates every second, so right after a
member comes or goes they can be up to a second behind. `voted` only says
that this host is not an assigned checker; whether a given check time was
decided by votes depends on the votes at that moment.
`emfor_backend_checks_total` still counts every check time, decided by
votes or not. A backend's latency is measured only
by this host's own checks, so on such a host it changes about every third
`interval`.

## Logs

The daemon logs to stderr: the journal under systemd, syslog (tag `emfor`)
under the FreeBSD rc.d script. Set the level with `[log] level`, or for
one run with `RUST_LOG` (for example `RUST_LOG=debug`). Colors are used
only when stderr is a terminal and `NO_COLOR` is not set.

| Level | What you see |
| --- | --- |
| `error` | Store commit failures, rejected reloads |
| `warn` | Plugin exits, damaged store replaced, services outside the DNS zone or with mixed ports, unparsable plugin output, dropped peer messages (one line per source and reason a minute), a member sending plaintext (`EMP1`) messages while `encrypt = true` (one line per member a minute), an unreadable `[peers] secret_file` |
| `info` | Start and stop, every backend state change with its reason, plugin stderr lines, peer members becoming live or not live, `peer … reports backend … down (…); checking now`, `backend … of … is down at 2/3 peers but up here` |
| `debug` | The duration of every passed check, peer reports that were ignored and why, results from a member with another check definition |

## Exit codes

| Status | Meaning | Retry helps? |
| --- | --- | --- |
| 0 | Clean stop (`SIGTERM` / `SIGINT`) | — |
| 1 | Cannot start: a listen address is in use, the store cannot be opened, or another emfor holds the store (`<store>.writer`) | No; fix the cause |
| 2 | Invalid config, or no command given | No; fix the config |
| 70 | The state writer or a DNS listener stopped while running | Yes; the systemd unit restarts it |

## The store on disk

The daemon commits to the store at least once a second and on every state
change. By default (`[store] sync = false`) a commit does not wait for the
disk, so a stalled disk cannot make the heartbeat late and the store look
stale to every application on the host. Nothing in the store needs to
survive a host crash: the daemon rewrites every service at start. Only the
state history (`emfor status --history`) is kept across restarts.

After a power loss the store may be damaged. The daemon then logs a
warning, moves it to `<store>.corrupt`, and starts with a new store;
running applications follow the new file by themselves. The only thing
lost is the state history: the last 20 state changes of each backend
that `emfor status --history` shows. Every state change is also in the
daemon's log (`changed from healthy to unhealthy` and the like), so look
there for what happened before the crash. Set `sync = true` to flush
every commit, at the risk described above.

Commits run on their own thread, so a slow disk does not delay DNS
answers, metrics or checks. Watch `emfor_store_used_bytes` against
`map_size`: when the store is full, commits fail and clients soon see a
stale store.

### Keeping the store in memory

Since nothing in the store has to outlive a reboot, it can live on a
tmpfs. Commits, about one a second, then never reach a disk, a stalled
disk cannot delay them even with `sync = true`, and a power loss leaves
no damaged file behind: the daemon simply starts with a new store. The
cost is the state history, which is then lost at every reboot, not only
after a crash (a restart of the daemon alone keeps it).

Mount the tmpfs on the store's directory itself (`/var/lib/emfor`, or
`/var/db/emfor` with the FreeBSD port), so that neither `[store] path`
nor `EMFOR_STORE` changes. Make it at least
as large as `[store] map_size` (`64MiB` by default); only the pages in
use take memory, a few tens of KiB for a few services.

- systemd: add to `/etc/fstab`

  ```
  tmpfs  /var/lib/emfor  tmpfs  size=64m,mode=0755,nosuid,nodev,noexec  0 0
  ```

  and, with `systemctl edit emfor`, `RequiresMountsFor=/var/lib/emfor`
  under `[Unit]`, so the daemon starts only once it is mounted.
  `StateDirectory=emfor` gives the directory to the `emfor` user at
  every start.
- FreeBSD: add to `/etc/fstab`

  ```
  tmpfs  /var/db/emfor  tmpfs  rw,size=64m,mode=0755,uid=<uid of emfor>  0 0
  ```

  with the number `id -u emfor` prints, so the directory belongs to
  `emfor` after every boot.

Do not use systemd's `RuntimeDirectory=` (`/run/emfor`) for the store: it
deletes the directory whenever the daemon stops, which loses the history
at every restart and leaves running applications on a deleted file.

This was checked in a Debian container with a tmpfs on `/var/lib/emfor`:
the store takes 32 KiB for one service, `emfor status` reads it, and the
history survives a restart of the daemon. The systemd and FreeBSD
settings above have not been run on a host.

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
   `Group=emfor-clients` and set `StateDirectoryMode=0750`. Under FreeBSD
   with the port:
   `chgrp emfor-clients /var/db/emfor && chmod 0750 /var/db/emfor`.
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
