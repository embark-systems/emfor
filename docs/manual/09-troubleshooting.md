# 9. Troubleshooting

[Manual](README.md) · Previous: [8. Operations](08-operations.md)

Start with `emfor status`: it shows whether the daemon is running, each
backend's state, and the reason of the last failed check. Then look at the
daemon's log (`journalctl -u emfor`, or syslog on FreeBSD).

## The daemon does not start

| Message or exit status | Cause | Fix |
| --- | --- | --- |
| Exit 2, config errors listed | The config is invalid. | `emfor check-config` lists every error with its TOML path. |
| Exit 2, usage printed | No command given (0.1.x style). | Use `emfor server`. |
| `--check-config was replaced by "emfor check-config"` | A 0.1.x command line. | See [upgrading](08-operations.md#upgrading). |
| Exit 1, names an address | A `[dns]` or `[metrics]` listen address is in use, or needs privileges (port 53). | Free the port, or choose another one. |
| Exit 1, names `<store>.writer` | Another emfor already runs with this store. | Stop it, or use another `[store] path`. |
| Exit 1, cannot open the store | The directory is missing or not writable for the daemon's user, or the lock file belongs to another user. | Create the directory with the right owner; see [lock file owned by another user](#lock-file-owned-by-another-user). |
| `plugins."x".command: ...` | A plugin failed to start or to answer the handshake. | Run the command by hand; check the path and its permissions. |

## Every lookup fails

| Error | Cause | Fix |
| --- | --- | --- |
| `UNAVAILABLE`, file not found | The daemon has never run, or `EMFOR_STORE` and `[store] path` differ. | Start the daemon; set `EMFOR_STORE`. |
| `UNAVAILABLE`, permission denied | The application user cannot read the store or write `<store>-lock`. | See [who can use the store](08-operations.md#who-can-use-the-store). |
| `UNAVAILABLE`, mentions the LMDB format or the release | `libemfor` is from another emfor release than the daemon. | Install both from the same release. |
| `UNAVAILABLE`, mentions `EMFOR_MAX_STALENESS` | The variable has an invalid value. | Use a whole number with `ms` or `s`. |
| `INCOMPATIBLE` | The store has another format version. | Install the client from the daemon's release. |
| `NOT_FOUND` | No service of that name in the running config. | Check the spelling, and that the reload was applied (`emfor status`). |

## A service is ALL DOWN

1. Look at the reasons in `emfor status --service <name>`.
2. If every backend shows the same local-looking reason (TLS, a CA file,
   `connect: Network is unreachable`), the problem is likely on the
   checking host, not the backends.
3. If the daemon line says `stale` or `stopped`, the backends are not
   down: the daemon is. Every `closed` service fails until it is running
   again. Restart it and look at its log.
4. Check the check itself: run `curl` or `nc` from the host to the
   backend with the same path, port and options.

## A backend flaps

`FLAPPING` in `emfor status` or the `EmforBackendFlapping` alert means
frequent changes between healthy and unhealthy. Common causes: a `timeout`
close to the backend's normal response time, or an overloaded backend.
Raise `timeout` (up to `interval`), raise `fall` and `rise`, or check the
backend. `emfor status --history --service <name>` shows when and why it
changed.

## Checks end in error

`error` results do not change a backend's state, but a backend that has
only errors for `fall × interval` returns to `unknown`. Typical reasons:

| Reason starts with | Cause |
| --- | --- |
| `plugin <name> unavailable`, `plugin exited` | The plugin crashed or is restarting; see the log for its exit status. |
| `plugin did not answer within timeout` | The plugin is too slow or handles checks one at a time. |
| `password_file: `, `ca_file: ` | The file cannot be read by the daemon's user. |
| `exit code N` (exec, N not 1 or 2) | The script failed in an unexpected way. |

## DNS does not resolve

| Symptom | Cause | Fix |
| --- | --- | --- |
| `dig @127.0.0.1 -p 5300` gets no answer | No `[dns]` table, or another listen address. | Check `emfor status` (the daemon line shows `dns ...`). |
| `REFUSED` | The name is outside the zone. | Name the service under the zone, or change `zone`. |
| `NXDOMAIN` | No service of that name. | Check the name. |
| `SERVFAIL` | No healthy backend and `on_all_down = "closed"`; or a validating unbound without `domain-insecure`. | See [ALL DOWN](#a-service-is-all-down); fix the unbound config. |
| Direct query works, `getent hosts` does not | The host resolver does not forward the zone. | Follow [chapter 7](07-dns.md#forwarding-from-the-host-resolver). |
| A failed backend is still returned | Resolver or application caching. | See [caching](07-dns.md#caching). |

## The store looks stale while the daemon runs

`emfor status` says `stale`, or clients act as if every backend were down,
while the daemon process is alive:

- `emfor_store_commit_failures_total` rising and `MDB_MAP_FULL` in the log:
  the store is full. Raise `[store] map_size` and restart.
- Commits failing for other reasons: the log names them.
- The system clock jumped forward: the heartbeat looks old until the next
  commit, about a second later.
- A local user holds the store's write lock (a writable lock file); see
  [who can use the store](08-operations.md#who-can-use-the-store).

## Lock file owned by another user

If the lock file `<store>-lock` was deleted while applications ran, the
first process to open the store recreated it with its own owner, and the
daemon (another user) may fail to open the store. Stop the applications or
fix the file's owner and mode (`chown emfor`, `chmod` to
`lock_file_mode`), then start the daemon. Never delete the lock file alone.

## A reload did not take effect

A reload is all or nothing. Look for `reload rejected` and the error lines
in the log, or run `emfor check-config`. `[store]`, `[dns] listen` and
`[metrics] listen` changes always need a restart.
