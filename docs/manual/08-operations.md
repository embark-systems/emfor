# 8. Operations

[Manual](README.md) · Previous: [7b. DNS recipes](07-dns-recipes.md) · Next: [9. Troubleshooting](09-troubleshooting.md)

## Commands

| Command | What it does |
| --- | --- |
| `emfor server` | Runs the daemon in the foreground. |
| `emfor check-config [--show]` | Validates the config and exits 0 (valid) or 2 (invalid). With `--show`, prints the merged config and the file that set each value ([chapter 3](03-configuration.md#checking-a-config)). |
| `emfor status` | Shows what the running daemon has published. |
| `emfor lookup <service>` | Prints the backend a client would get, and with `--explain` why ([chapter 6](06-clients.md#from-the-shell-emfor-lookup)). |
| `emfor --help` | Lists the commands and the default config path. |

`--config <path>` goes before or after the command. `emfor` with no
command only prints its usage and exits 2; it never starts a daemon.

## Signals

| Signal | Effect |
| --- | --- |
| `SIGHUP` | Reload the config (the main file and the files of `include_dir`). Applied only when the whole config is valid; otherwise the errors are logged and the old config keeps running. See [what a reload changes](03-configuration.md#what-a-reload-changes). |
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

`emfor status` reads only the `[store]` table of the config ([chapter 3](03-configuration.md#store)) and opens the
store read-only, like a client. Opening the store registers a reader in its
lock file, so the user needs read access to the store and write access to
the lock file ([who can use the store](#who-can-use-the-store)); with the
lock file at mode 0444, `emfor status` stops with `cannot open store …:
Permission denied`.

- **daemon**: `running` while the heartbeat is at most 3 seconds old,
  `stale` when it is older (the daemon hangs or died), `stopped` after a
  clean shutdown. With `stale` or `stopped`, the services shown are only
  what the daemon wrote last.
- **service**: `balance` and `on_all_down` ([chapter 5](05-backend-selection.md#balance-methods)), and `ALL DOWN` when no backend is
  healthy.
- **backend**: state, time since the last check, smoothed latency, then
  these markers, in this order, when they apply: `pool` for the fast
  backends of a `fastest` service; `backup` for a backup backend
  ([chapter 5](05-backend-selection.md#backup-backends)); `local-up` and
  `voted` in a peer group (see [Peer groups](#peer-groups)); `FLAPPING
  (<n> changes in 10m)` for a backend that changed between healthy and
  unhealthy 5 or more times in 10 minutes; `last check failed` for a backend
  that is still `healthy` but whose last check failed (it turns unhealthy
  after `fall` failed checks in a row; until then lookups usually skip it,
  see [a backend whose checks start
  failing](05-backend-selection.md#a-backend-whose-checks-start-failing),
  but not when every healthy backend is failing); and the reason of the last
  failed check. A backend drained in the config
  ([chapter 5](05-backend-selection.md#draining-a-backend)) shows `drained`
  in place of its state, and `(checks: <state>)`, the state the checks give,
  as the last marker, after `backup`, `local-up`, `voted` and `FLAPPING`; it
  has no `last check failed` and no reason of the last failed check. A row with every marker and a short reason is about 100
  columns wide (97 in a group of ten hosts with 40 backends); `--watch` cuts
  lines wider than the terminal.

```
  10.0.1.3:5432   healthy  0.8s ago  1.2ms  voted  last check failed  connect: Connection refused
  10.0.1.40:5432  healthy  0.8s ago  1.2ms  backup  voted
  10.0.0.13:8080  drained  0.8s ago  1.2ms  (checks: healthy)
  127.0.0.1:9     drained  0.4s ago  -      (checks: unhealthy)
```

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
| `emfor_backend_drain` | 1 for a backend drained in the config, else 0; `emfor_backend_state` keeps the state the checks give |
| `emfor_service_all_down` | 1 when no backend is healthy |
| `emfor_backend_checks_total{result}` | Checks by result: `up`, `down`, `error` |
| `emfor_backend_transitions_total{to}` | State changes by new state |
| `emfor_store_commit_failures_total` | Failed store commits |
| `emfor_store_last_commit_timestamp_seconds` | Last successful commit; should be under a few seconds old |
| `emfor_store_used_bytes`, `emfor_store_map_size_bytes` | Store use against `[store] map_size` |
| `emfor_plugin_up`, `emfor_plugin_restarts_total` | Plugin health |
| `emfor_dns_rate_limited_total{action}` | With `[dns]`: UDP answers over `rate_limit`, by what was sent instead: `truncated` or `dropped` ([chapter 7](07-dns.md#rate-limit)) |
| `emfor_peer_up{peer}` | With `[peers]`: 1 while that member is live |
| `emfor_peer_messages_rejected_total{reason}` | With `[peers]`: dropped messages by reason: `malformed`, `auth` (wrong secret, or a message that does not decrypt), `sender` (not a member, or a spoofed address), `site`, `stale` (clocks more than 5 seconds apart), `replay`, `plaintext` (an unencrypted message while `accept_plaintext` is `false`, which is the default with `encrypt = true`) |
| `emfor_peer_notifications_total{direction}` | With `[peers]`: failure reports `sent` (one per member) and `received` |
| `emfor_peer_results_total{direction}` | With `[peers]`: check results of assigned checkers `sent` (one per member) and `received` |
| `emfor_peer_disagreements` | With `[peers]`: backends the group votes down while this host's own check passes |
| `emfor_peer_checks_skipped_total` | With `[peers]`: checks this host did not make because two assigned checkers agreed |
| `emfor_peer_queue_dropped_total` | With `[peers]`: `down` and `result` messages not sent because the peer task fell more than 4096 messages behind (logged as `the peer task is behind`); should stay 0 |
| `emfor_build_info{version}`, `emfor_start_time_seconds` | The daemon |

[examples/prometheus/emfor-rules.yml](../../examples/prometheus/emfor-rules.yml)
has alerting rules: the endpoint is down, a service is `ALL DOWN`, store
commits fail, a backend is unhealthy for 10 minutes, flaps, mostly errors,
or is no longer checked. [docs/deploy/daemon.md](../deploy/daemon.md#metrics) has the metrics with
their types and labels.

### Peer groups

Members of a peer group need UDP between each other on their [`[peers]`](03-configuration.md#peers)
`listen` port (5302 in the examples) and clocks within 5 seconds of each
other (NTP). If `emfor_peer_up` stays 0 for a member, look at
`emfor_peer_messages_rejected_total` on both sides: `auth` means the
secrets differ, `stale` that the clocks drift, `sender` that `members` or
`listen` do not match, `site` that the hosts have different `site` values.
`malformed` on a member that runs 0.3.0 means it cannot read the
encrypted messages of upgraded members: set `encrypt = false` on those
until it is upgraded (see
[`[peers]`](03-configuration.md#peers)). A warning `peer <address> sends
plaintext messages (EMP1); set encrypt = true there` means that member
still sends readable messages. They are accepted only with
`accept_plaintext = true`; by default a daemon with `encrypt = true`
drops them, they count as `plaintext`, and that member is not live here.
The log then has no such warning for that member; look for
`dropped peer message from <address> (plaintext)` instead (once a minute for
each source). Set `accept_plaintext = true` on this host until that member
sends encrypted messages.

Reasons tell where a `down` came from: `peers 2/3: <reason>` means two of
the three assigned checkers saw the backend down (the reason is theirs),
`local: <reason>` that only this host sees it down while the vote did not
say down, and a reason without a prefix that there was no vote (fewer than
three live members, or no recent result). `emfor status` shows the group
under the daemon line:

```
peers: 2/3 live, not live 10.0.0.7:5302
peers: check definition of api.svc.emfor differs at 10.0.0.6:5302
```

and marks backends with `local-up` when the group votes them down while
this host's own check passes. Those are worth a look: this host reaches a
backend the others do not (often a network partition or a firewall rule on
the other hosts), and `emfor_peer_disagreements` counts them. To see which members decide
a backend, read its `assigned_checkers` in `emfor status --json` (below).
A "check
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

When this host and the live members that have not left a service out number
fewer than three, the group cannot vote for it: no backend of it is voted
and every reason is this host's own, without a `peers d/f:` prefix. If that
is so for every shown service that takes part in the group, the `peers:`
line ends with `too few live members to vote`:

```
peers: 1/2 live, not live 10.0.0.7:5302, too few live members to vote
```

If some services can vote and others cannot, the line says nothing; a
backend with no `checkers` in `assigned_checkers` (below) is decided by
this host alone.

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
`interval`. The `<age> ago` of a voted backend is the time its result was
applied, whoever decided it, so it can be recent while this host last
connected to the backend up to about three `interval`s ago.

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
| 2 | Invalid config, or no command given | No; run `emfor check-config` first (with the binary you are about to start) and fix the config |
| 70 | The state writer or a DNS listener stopped while running | Yes; the systemd unit restarts it |

## The store on disk

The daemon commits to the store at least once a second and on every state
change. By default ([`[store] sync = false`](03-configuration.md#store)) a commit does not wait for the
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

Never change the store file in place: no `cp` over it, no `truncate`, no
shell redirection (`> state.mdb`). The daemon and every application read
it through a memory map, and a file that becomes shorter while they have
it open ends each of those processes with `SIGBUS`. To put another file
in its place, write it next to the store and `mv` it over the store path:
running applications then follow the new file. A store that is already
shorter than LMDB records when it is opened, as after a power loss, is
handled like other damage: the daemon moves it to `<store>.corrupt`,
lookups fail with `UNAVAILABLE` (the message says `truncated`) until
there is a good store, and `emfor status` exits 2. LMDB itself can leave
a store a few pages shorter than it records (pages it freed again before
writing them); the daemon extends the file after every commit, so this
only matters for a store an older emfor wrote: the first start after the
upgrade may move it aside, losing only the state history.

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

## Rolling deploy with drain

To deploy to the backends of a service one at a time without cutting the
requests that are running, take the backend out of the choice with
`drain = true` ([chapter 5](05-backend-selection.md#draining-a-backend)),
wait until its requests are over, stop, deploy and start it, wait until its
checks pass, and put it back. emfor does the first and the last step and
shows the checks in between; the waiting is yours, because emfor is not in
the data path and does not see a request.

A `conf.d` file cannot switch one backend of a service that another file
defines: a list is replaced as a whole, so the file would have to repeat the
whole `backends` list. Give the service its own file with one backend per
line, and change that line:

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
the steps for one backend. Before you start:

- No package installs the script. Copy it to the emfor host (for `on`, `off`
  and `wait-ready`) and to the backend host (for `wait-idle`).
- Run it as root on the emfor host: `emfor check-config` run as root accepts
  only conf.d files that root owns, and the script reloads emfor.
- It needs `jq`, and `ss` (Linux, iproute2) for `wait-idle`.
- The reload command is `systemctl reload emfor`. Set `EMFOR_RELOAD` for
  another one (FreeBSD: `service emfor reload`, with the files in
  `/usr/local/etc/emfor/conf.d`).
- It calls `emfor check-config` and `emfor status` without `--config`, so
  they use the config path built into emfor. For another path set
  `EMFOR_CONFIG=/path/to/emfor.toml`.
- Each backend must be one line, written exactly `{ addr = "10.0.0.11:8080" },`
  with a space before the `}`. For a line it cannot change the script stops
  and leaves the file alone. It writes the new line to a copy, moves it
  over the file, runs `check-config`, and puts the old file back if the check
  refuses; only then does it reload.
- It finds the backend in `emfor status --json` by address, in the first
  service (in name order) that has that address. If two services share an
  address, set `EMFOR_SERVICE=<name>`.

```sh
# On the host that runs emfor: add drain = true to the line of the backend,
# run emfor check-config, reload, and wait until `emfor status` shows it drained.
emfor-drain.sh on /etc/emfor/conf.d/20-api.toml 10.0.0.11:8080

# On 10.0.0.11: wait until no connection to port 8080 has moved data for 3 seconds.
emfor-drain.sh wait-idle 8080
deploy-and-restart-api

# On the emfor host again: wait until the checks of the backend have said
# healthy for 10 seconds, then remove drain and reload.
emfor-drain.sh wait-ready 10.0.0.11:8080 120
emfor-drain.sh off /etc/emfor/conf.d/20-api.toml 10.0.0.11:8080
```

What `wait-ready` guarantees. A backend that has just stopped keeps the state
`healthy` until `fall` checks have failed (with the defaults, `interval` 2
seconds and `fall` 3, about 6 seconds), and a drained backend hides the
result of its last check (the store shows only the state of the checks). So
`wait-ready ADDR [SECONDS [SETTLE]]` does not return at the first `healthy`:
it returns when a check newer than the command has finished and the checks
have said `healthy` for SETTLE seconds in a row (default 10), and for a
backend that is not drained also when the last check passed. SETTLE must be
longer than `fall` x `interval` plus `timeout` of the service; raise it if
you changed those. It does not count `rise` and does not look at what the
application does with a request: a backend that passes a `tcp` check and
still answers 500 is ready for it.

Do the backends one at a time, and look at `emfor status` between them (its
exit status is 1 when a service is `ALL DOWN`): a service whose backends are
all drained answers nothing. `drain` is a setting of one host's config
([chapter 5](05-backend-selection.md#draining-a-backend)), so when emfor runs
on every application host, run the first and the last step on each of them.
A HAProxy that has the emfor servers of several hosts as name servers needs
them to agree on `drain` too, and is better off with one local name server
([HAProxy in chapter 7](07-dns-recipes.md#using-it-with-haproxy)).

What the waiting is for, as `tests/deploy/run.sh` saw it with nginx 1.30.6
and 1.31.8 taking its upstream from emfor's SRV answers
([chapter 7](07-dns-recipes.md#using-it-with-nginx)):

- A request that is running when the backend is drained finishes (a 1200
  byte answer, sent over about 6 seconds, arrived whole). Stopping the
  backend without a drain cut the same request after 146 bytes.
- New requests stop reaching a drained backend within a few seconds (nginx
  asks again every second). For a client that reads the store it is the
  next lookup; a client that asks DNS may cache the answer for its TTL.
- nginx retried a request that a stopped backend had refused, so a stop
  without a drain lost no new request in the test (0 of 64 to 96 POSTs).
  The drain matters for the requests that are running, and for clients that
  do not retry; the second was not tried.
- Connections that a proxy keeps open for the next request stay open after
  the drain, so "no connection to the port" does not come: nginx 1.30
  keeps idle connections to an upstream by default, and 1 or 2 were still
  open when the wait was over. `wait-idle` therefore waits for a quiet
  time, 3 seconds by default (a third argument, in milliseconds), read from
  `ss -i`. A request that stays silent for longer than that, a long poll or
  an event stream, looks idle to it: raise the time, or count the requests
  in the application.
- `wait-ready` with the default `interval` and `fall`: the test stops a
  drained backend and looks at `wait-ready` at once; the checks still said
  `healthy` then, and `wait-ready` returned only after the backend had been
  started again and its checks had passed for 10 seconds (26 seconds after
  the stop, the backend being started 12 seconds after it). The rest of the
  test config has `interval = "200ms"` and `fall = 1`, with which a stopped
  backend shows `unhealthy` within a fraction of a second; those timings hide the
  problem, so the case runs with the defaults. The test also checks that the
  script leaves a file alone when it cannot change the line, and puts the old
  file back when `check-config` refuses the new one.
- `off` puts the backend back at once, without counting `rise` again, and
  without slow start; a backend that was restarted while it was drained
  ramps from the moment its checks passed, so after a short deploy it may
  still be inside `slow_start`
  ([chapter 5](05-backend-selection.md#slow-start)).

Not tried: HAProxy, which also retries, and its own sessions; HTTPS;
clients that keep their own connection pool to the backends (a drain does not
close a pooled connection); several emfor hosts. The script and the test ran
on Linux only. An earlier version of the script (before `wait-ready` settled
and before `EMFOR_CONFIG`, `EMFOR_SERVICE` and the restore of the old file)
passed on FreeBSD 15.1 (nginx 1.30.6 and `jq` from the packages) when run
by hand with `wait-idle` replaced by a fixed 8-second wait; the present
script was not run there. `wait-idle` needs `ss`, a Linux tool, and stops
with a message elsewhere: wait a fixed time, at least the longest request
and the time a proxy needs to ask again, or count the requests in the
application. `tests/deploy/inside.sh` itself calls `ss` and writes
`/etc/nginx/nginx.conf`, so a FreeBSD run needs edits to it. The Integrations
workflow runs the test on Linux every week and when the test or the script
changes.

## Watching emfor from Nagios, Icinga or Zabbix

Two small scripts connect emfor to a monitoring system that speaks the Nagios
plugin convention (exit status 0 OK, 1 WARNING, 2 CRITICAL, 3 UNKNOWN, a first
line of text, perfdata after a `|`). They are examples to copy, in
[examples/nagios/](../../examples/nagios/check_emfor.sh).

**Watching emfor: `check_emfor.sh`.** It reads `emfor status --json` (it needs
`jq`) and gives the Nagios status of it:

```console
$ check_emfor.sh -s web.svc.emfor
EMFOR WARNING - web.svc.emfor: 2 of 3 healthy (127.0.0.1:7002 WARNING - slow on 7002) | 'web.svc.emfor_healthy'=2;3;1;0;3
$ echo $?
1
```

- CRITICAL: the daemon is stale or stopped, or a service has fewer than `-c`
  healthy backends (default 1: all down, or all drained).
- WARNING: fewer than `-w` are healthy (default: every backend that is not
  drained).
- UNKNOWN: `emfor` or `jq` cannot be run, the store cannot be read, or the
  service does not exist.
- A drained backend is left out of the counts (it is out on purpose) and is
  named in the text. `-s` picks a service (default every service), `-C` the
  config that names the store.
- The user the monitoring runs as must be able to read the store and write
  its lock file ([who can use the store](#who-can-use-the-store)); with
  the recommended `lock_file_mode = "0660"` that user must be in the group.
  With NRPE or SSH the script runs on the host that has the store.

**Using a Nagios plugin as a check: `emfor-exec.sh`.** emfor's [`exec`
check](04-health-checks.md#exec) starts a program without a shell and passes
the backend in the environment (`EMFOR_IP`, `EMFOR_PORT`), so a plugin cannot
be given `-H $EMFOR_IP` in the config. `emfor-exec.sh` does that and runs the
plugin:

```toml
[services."web.svc.emfor"]
check = "exec"
backends = ["10.0.0.11:80", "10.0.0.12:80"]
interval = "10s"
timeout = "5s"
check_options = { command = ["/usr/local/libexec/emfor-exec.sh",
    "/usr/local/libexec/nagios/check_http", "-u", "/healthz"] }
```

It runs `check_http -H <ip> -p <port> -u /healthz` and returns the plugin's
exit status; `--host-flag` and `--port-flag` change the two flags for a plugin
that uses others. What emfor makes of it: 0 is up, **1 (WARNING) is down**, 2
(CRITICAL) is down, and 3 (UNKNOWN) is an error: the state does not change at
first, and after `fall` x `interval` of nothing but errors it becomes
`unknown` ([chapter 4](04-health-checks.md#rise-fall-and-errors)); the first line of the plugin's output is the reason, and a
plugin that has not finished at `timeout` is down. `--warning up` turns a
WARNING into up. A process is started for every check of every backend, so
choose `interval` with that in mind ([chapter 4](04-health-checks.md#exec)).

`tests/nagios/run.sh` checks both scripts in Docker, with a stand-in for the
plugin that says what a file tells it (ok, warn, crit, unknown, hang). Not
tried: the real plugins of the Monitoring Plugins or Nagios Plugins packages,
a Nagios or Icinga core, NRPE, Zabbix (its `UserParameter` can run
`check_emfor.sh` and use its text), FreeBSD.

## Running more than one daemon

Each store has one writer: the daemon locks `<store>.writer`, and a second
daemon with the same `[store] path` exits with status 1. To run two
daemons on one host (for testing), give them different store paths, DNS
ports and metrics ports.

## Upgrading

Install the daemon and every client library from the same release, in this
order:

1. Upgrade the client libraries (and `emfor status`) on the host, and restart
   the applications so that they load them: a running application keeps the
   library it started with. A client older than the daemon can fail every
   lookup with `INCOMPATIBLE` (the store format is version 2 since 0.4.0, see
   [store format version](06-clients.md#store-format-version)), and an older
   client can still choose a drained backend in the last tier of an open
   service ([draining a backend](05-backend-selection.md#draining-a-backend)).
2. Run `emfor check-config` with the new binary, before you restart. 0.5.0
   refuses options that 0.4.0 accepted (an `interval` above `24h`, a
   `timeout` below `10ms`, and more: [CHANGELOG.md](../../CHANGELOG.md)).
   With an invalid config, `systemctl restart emfor` leaves the daemon down
   (the check before the start fails, and the unit ends `failed` until
   `systemctl reset-failed emfor`): see
   [Restarts](../deploy/daemon.md#running-under-systemd) in the daemon guide.
3. Install the daemon and restart it.

Release notes that change what to do: 0.4.0 writes store format version 2
and encrypts peer messages by default (a group is upgraded host by host with
`encrypt = false`, [Peer groups](#peer-groups)); 0.5.0 validates the config
more strictly.

After the restart, applications that were already running the new library
follow the restarted daemon's store without another restart.

### What applications see during a restart

A lookup never waits for the daemon: it always gets an answer from the
store. But around a restart the store cannot tell good backends from bad
ones for a short time:

- **While the daemon is stopped.** After a clean stop the store is stale at
  once (the heartbeat is 0); after a kill, clients trust the old states for
  up to 3 seconds and then treat the store as stale. In a stale store every
  backend is a candidate: an `open` service returns any of them, a `closed`
  service fails with `ALL_DOWN`.
- **For the first 0.2 to 0.4 seconds after the start.** The daemon marks
  every backend `unknown` and checks each one about every 200 ms. Until the
  first results are in, all of them are candidates: `open` returns any
  backend, and `closed` fails with `ALL_DOWN` until one of its backends is
  healthy.

Measured with the daemon started again at once (after SIGTERM, and after
SIGKILL): 20 restarts per case, one host in a Debian container, loopback,
`tcp` checks with the default settings, two services of four backends each
with two backends down, and a client that looks up every millisecond. The
last wrong answer came about 0.25 seconds after the signal (median; at most
about 0.4 seconds). In that window half of the `open` answers were backends that
were down, and every `closed` lookup failed with `ALL_DOWN`. `emfor
check-config`, which the example systemd unit runs before every start,
took about 1 ms. When the daemon stayed down for 2 seconds, the window was
2.2 to 2.4 seconds. A service whose backends are all up is not affected,
because any backend is then a good answer. Other hardware, networks,
intervals and checks give other numbers.

What to do about it:

- If the host is behind a load balancer, take it out of rotation first, let
  the requests in flight finish, restart, and put it back when `emfor
  status` exits 0 (the daemon is running and every service shown has a
  healthy backend; see [emfor status](#emfor-status)). Work that does not
  come through the load balancer (batch jobs, queue workers, connection
  pools that reconnect) still looks up during the restart.
- Services with `on_all_down = "closed"`, with `failover` and a bad first
  backend, or with backups are the most affected; do this for hosts that
  use them.
- Install the new packages first and restart afterwards. Do not stop the
  daemon, install and start it: the window is then as long as the install.
- Restart hosts one at a time.
- A change of the config alone needs no restart: use a reload, which keeps
  the state of backends whose check did not change (see [what a reload
  changes](03-configuration.md#what-a-reload-changes)).

**From 0.1.x to 0.2.0**: the daemon is now started with a command. Change
your service definition before restarting:

| 0.1.x | 0.2.0 |
| --- | --- |
| `emfor --config X` | `emfor server --config X` |
| `emfor --check-config --config X` | `emfor check-config --config X` |

The store format and the plugin protocol are unchanged. Replace copies of
the 0.1.x systemd unit and rc.d script with the new examples. See
[CHANGELOG.md](../../CHANGELOG.md) for everything that changed.
