# 16. Troubleshooting

[Contents](README.md) · Previous: [15. Upgrading](15-upgrading.md) · Next: [17. Migrating from HAProxy or Consul](17-migrating.md)

Start with these three commands; most problems show up in one of them:

```sh
# emfor check-config                       # is the config valid?
# emfor status                             # is the daemon running, what does it see?
# emfor lookup <service> --explain         # why does a lookup answer what it answers?
```

and the log: `journalctl -u emfor` (systemd) or syslog with the tag `emfor`
(FreeBSD). The emfor messages below were produced by emfor 0.7.0 in a Debian
test container, except the reasons of plugins, which come from the code, and
the systemd and DNS symptoms, which come from the repository's documents.

## The daemon does not start

| Message | Cause | Fix |
| --- | --- | --- |
| `cannot open state store: /var/lib/emfor/state.mdb.writer: No such file or directory (os error 2)` | The store directory does not exist; the daemon does not create it. | `install -d -o emfor -g emfor -m 0755 /var/lib/emfor` (the unit's `StateDirectory=` does this). |
| `cannot open state store: store is already in use by another emfor daemon: lock file /var/lib/emfor/state.mdb.writer is held` | Another daemon uses the store. | Stop it, or give this one another `[store] path`. |
| `cannot start metrics server: cannot listen on 127.0.0.1:8080 (tcp): Address already in use (os error 98)` | The port is taken. | Free it or change `listen`. |
| `emfor status` prints `store is empty: the daemon has not written to it yet; if it should be running, check its log` | The daemon opened the store and exited before its first write, for example because it could not bind its DNS port. The store is not damaged. | Read the log (`journalctl -u emfor`) for the error the daemon exited with; the rows above and the port in `[dns] listen` are the usual causes. |
| `invalid config /etc/emfor/emfor.toml: 2 error(s)` and the error lines | Invalid config (exit 2). | Run `emfor check-config` and fix each line (below). |
| `/etc/emfor/conf.d: writable by group or others (mode 0775); refusing to load it; to fix it: chmod g-w,o-w /etc/emfor/conf.d` | A config file or directory others can change. | Run the `chmod` (or `chown`) the message names. |
| `include_dir: cannot read directory /etc/emfor/conf.d: No such file or directory (os error 2)` | `include_dir` names a missing directory. | Create it, or remove `include_dir`. |
| `--check-config was replaced by "emfor check-config"` | An old command line. | Use `emfor check-config`. |
| systemd: the unit is `failed` after repeated starts | Five failed starts in a minute (`StartLimitBurst=5`). | Fix the cause, then `systemctl reset-failed emfor`. |

## Config errors

`emfor check-config` prints one line per error, prefixed by the key's path
and, for a `conf.d` file, followed by the file:

```text
services."web.svc.emfor".timeout: timeout must not exceed interval (in /etc/emfor/conf.d/50-x.toml)
```

| Message | Fix |
| --- | --- |
| `defaults.interval: invalid duration "2", use e.g. "500ms" or "2s"` | A duration needs a unit. |
| `defaults.jitter: jitter above 50% or below 0%` | `jitter` is `"0%"`–`"50%"`. |
| `services.Bad_Name: invalid service name: labels of [a-z0-9-], 1-63 characters each, 253 characters total` | Lower-case DNS-style names only. |
| `backend "web1:80" must be an IP literal with port: IPv4:port or [IPv6]:port, port 1-65535` | Backends are IP addresses, never host names. |
| `duplicate backend "10.0.0.1:80"` | Each backend once per service. |
| `weight must be from 1 to 100` | |
| `at least one backend must not be a backup` | |
| ``unknown checker `nope` `` | `check` names neither a built-in check nor a declared plugin. |
| `check_options.bogus: unknown key` | A typo, or an option of another check. |
| `path: must start with '/'` | http `path`. |
| `expect_status[0]: status 700 outside 100-599` | |
| `tls_verify: requires tls = true` | tcp TLS keys need `tls = true`. |
| `expect: required when send is set` | tcp. |
| `command: "check.sh" is not an absolute path` | exec and plugin commands are absolute. |
| `env_pass: EMFOR_IP is set by emfor` | Do not list `EMFOR_IP`, `EMFOR_PORT`, `EMFOR_SERVICE`. |
| `site: invalid site label: 1-63 characters of [a-z0-9-], not starting or ending with -` | |
| `/etc/emfor/conf.d/60-s.toml: store is only allowed in the main config file` | `[store]` and `include_dir` belong in the main file. |
| `/etc/emfor/conf.d/70-b.toml:1:24: invalid TOML: unclosed table, expected ']'` | TOML syntax; line and column are given. |

## A reload did not take effect

A reload is all or nothing. The log says:

```text
ERROR emfor_daemon::cli: invalid config /etc/emfor/emfor.toml: 2 error(s)
...
reload rejected; keeping the previous config
```

Some settings are never reloaded:
`store: store settings require restart`,
`dns.listen: dns listen settings require restart`, and the same for
`metrics.listen` and `peers.listen`. Restart for those.

## Every lookup fails

| Error | Cause | Fix |
| --- | --- | --- |
| `UNAVAILABLE: store /var/lib/emfor/state.mdb does not exist; is the emfor daemon running?` | The daemon has never run, or `EMFOR_STORE` points elsewhere. | Start the daemon; check `[store] path` against `EMFOR_STORE`. |
| `UNAVAILABLE: cannot open store /var/lib/emfor/state.mdb: Permission denied` | The application's user cannot read the store or write the lock file. | Add it to the daemon's group ([chapter 12](12-operating.md#who-can-read-the-store)) and restart the application. |
| `UNAVAILABLE: EMFOR_MAX_STALENESS="3" is invalid: ...` | The variable needs a unit. | `EMFOR_MAX_STALENESS=3s`. |
| `UNAVAILABLE` saying the LMDB format differs | The client comes from another release. | Install clients from the daemon's release. |
| `INCOMPATIBLE` | A client older than the store's format (a client before 0.4 on a version 2 store), or a record the client cannot parse (damage, or a bug worth reporting). | Upgrade the client; if it is from the daemon's release, report it. |
| `NOT_FOUND: no service named "nope.svc.emfor"` | A typo, or the service is not in this host's config. | `emfor status` lists the services. |

`emfor status` shows the same problems from the command line:
`cannot open store /var/lib/emfor/state.mdb: no such file` and
`cannot open store ...: Permission denied (os error 13)` (exit 2).

## A service is ALL DOWN

```text
ALL_DOWN: service db.svc.emfor: no backend is healthy, and on_all_down is closed
```

1. `emfor status --service db.svc.emfor` shows each backend's reason.
2. `emfor lookup db.svc.emfor --explain` shows why each one is out; every
   backend `drained` also gives `ALL_DOWN`.
3. If the message says
   `the store is stale (emfor daemon not updating it)`, the backends are not
   the problem: the daemon is. `emfor status` then says
   `daemon: stale, heartbeat 4.5s ago` or `daemon: stopped`.
4. Run the check by hand from the same host (`curl`, `nc`, the exec program
   as the daemon's user).

## Reading the reasons

| Reason | From | Meaning |
| --- | --- | --- |
| `Connection refused (os error 111)` | tcp | Nothing listens on the port. |
| `timeout` | any | No answer within `timeout`. |
| `expect: got "..."` | tcp | The answer did not match `expect`. |
| `error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)` | http | Nothing listens. |
| `unexpected status 503` | http | The status is not in `expect_status`. |
| `cannot build http client: ...; https checks need the system CA certificates, or a ca_file, unless tls_verify = false` | http (error) | Install `ca-certificates`, point `ca_file` at your private CA, or set `tls_verify = false`. |
| `exit code 1`, or the program's first output line | exec | The check program said down. |
| `cannot run /path: ...` | exec (error) | Not found, not executable, or not allowed under the unit's sandbox. |
| `killed by signal 9` | exec | The program crashed or was killed. |
| `connect: Connection refused (os error 111)` | emfor-check plugins | |
| `plugin <name> unavailable`, `plugin exited` | plugin (error) | The plugin crashed or is restarting; see the log. |
| `plugin <name> busy: it is not reading its requests` | plugin (error) | The plugin stopped reading; it is restarted after 3 unanswered requests. |
| `password_file: ...` | plugin (error) | The daemon's user cannot read the file. |
| `peers 2/3: <reason>` | peer group | Two of three assigned checkers saw it down. |
| `local: <reason>` | peer group | This host saw it down; the votes did not. |
| `drained` | store | `drain = true` ([chapter 8](08-choosing-a-backend.md#drain)). |
| `check task restarted` (in the history) | daemon | A check task stopped and was restarted. Look for `check task of backend ... stopped` in the log and report it. |

An `error` never marks a backend down, but nothing but errors for
`fall × interval` takes it back to `unknown`.

## A backend flaps

`FLAPPING (5 changes in 10m)` in `emfor status`, or the
`EmforBackendFlapping` alert, means frequent changes. Usually the `timeout`
is close to the backend's normal response time, or the backend is overloaded.
`emfor status --history --service <name>` shows when and why. Raise
`timeout` (at most `interval`), raise `fall` and `rise`, or fix the backend.

## Checks work by hand but not in emfor

- **The environment.** Since 0.7.0 check programs get only `PATH`, `LANG`,
  `TZ` and `LC_*`. Add what they need to `env_pass`.
- **The user.** The daemon runs as `emfor`; run the check with
  `runuser -u emfor -- ...`.
- **The sandbox.** The unit makes the file system read-only and hides other
  processes; a check that writes a file or reads `/home` fails there.
- **Redirects.** http checks do not follow them, and a `301` is inside the
  default `200-399`, so a redirect counts as up. Set
  `expect_status = ["200"]` (or point `path` at the final page) to require it.
- **Proxies.** http checks ignore `http_proxy`.

## DNS does not resolve

| Symptom | Cause | Fix |
| --- | --- | --- |
| No answer at all | No `[dns]` table, the daemon is down, or the wrong port. | `dig @127.0.0.1 -p 5300 <service>`. |
| `REFUSED` | The name is outside the zone. | Check `[dns] zone`. |
| `NXDOMAIN` | No such service, or it was just added (negative caching for `ttl`). | `emfor status`. |
| `SERVFAIL` | All down and `closed`, every backend drained, the daemon just started, or a validating unbound without `domain-insecure`. | `emfor lookup <service> --explain`. |
| Works with `dig @127.0.0.1 -p 5300`, not with `getent hosts` | The host resolver does not forward the zone. | [Chapter 10](10-dns.md#forwarding-from-the-host-resolver). |
| A failed backend is still returned | Resolver or runtime caching. | [Chapter 10](10-dns.md#caching). |

## The store looks stale while the daemon runs

- `emfor_store_commit_failures_total` rises, `MDB_MAP_FULL` in the log: the
  store is full. Raise `[store] map_size` and restart.
- The host's clock jumped, in either direction: clients compare the heartbeat
  with their own clock. A heartbeat more than `EMFOR_MAX_STALENESS` ahead
  counts as stale too; `emfor status` then says `heartbeat <age> in the
  future`. It clears by itself within about a second of the next heartbeat.
- The daemon is overloaded or stopped (`SIGSTOP`, a debugger).

## The lock file is owned by another user

If `<store>-lock` was deleted while applications ran, the first process to
open the store recreated it with its own owner, and the daemon may then fail
to start. Stop the applications, or `chown emfor:emfor` and `chmod 0660` the
lock file, then start the daemon. Never delete the lock file alone.

## Peer problems

| Symptom | Cause |
| --- | --- |
| `emfor_peer_messages_rejected_total{reason="auth"}` rises | Members have different secrets, or one restarted during a secret change ([chapter 11](11-peer-groups.md#a-restart-during-the-change)). |
| `reason="plaintext"` | A member sends `EMP1` (`encrypt = false`) to one that refuses it. |
| `reason="sender"` | A message from an address that is not in `members` (NAT, a wrong `listen`). |
| `reason="site"` | A member with another `site`. |
| `reason="stale"` or `replay` | Clocks differ by more than 5 seconds in either direction ([chapter 11](11-peer-groups.md#the-secret)), or messages are duplicated on the way. |
| `peers: 1/2 live, ... too few live members to vote` | Fewer than three live members: no votes, each decides alone. |
| A digest mismatch in `emfor status --json` | Members check a service differently (options, timeout, plugin command, `env_pass`). |
| `local-up` in `emfor status` | Down by votes but up here: a network path from this host only, or a different check. |
