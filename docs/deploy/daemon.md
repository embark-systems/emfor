# Building emfor and checking what it publishes

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
- `emfor --help` shows the path that was built in, after `--config`.
- Only the config path changes. The store stays at the `[store] path` of the config (default `/var/lib/emfor/state.mdb`), and clients keep their own default; see the client READMEs.

## emfor status

`emfor status` shows what the running daemon has published in the state store. It reads only the `[store]` table of the config (so it still works while the rest of the file has an error), opens the store read-only like a client, and never changes it.

```console
$ emfor status
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.4s ago, up 3h12m, site tpe1

api.svc.emfor  round-robin  on_all_down=open
  10.0.0.11:8080  healthy    0.8s ago  1.2ms
  10.0.0.12:8080  unhealthy  0.9s ago  -      connect: Connection refused

db.svc.emfor  failover  on_all_down=open  ALL DOWN
  10.0.3.1:5432   unhealthy  1.1s ago  -      auth: 28P01: password authentication failed for user "emfor"
```

- **daemon line**: `running` while the heartbeat is at most 3 seconds old; `stale` when it is older (the daemon hangs or is gone without a clean shutdown); `stopped` after a clean shutdown. The services shown are what the daemon last wrote; with a stale or stopped daemon, do not trust them.
- **service line**: the `balance`, `on_all_down`, and `ALL DOWN` when no backend is healthy.
- **backend lines**: health state after `rise` / `fall`, time since the last check finished (`never` before the first), the smoothed check latency (`-` when the backend is not healthy), `pool` for the fast backends of a `fastest` service, and the message of the last failed check. The message is cleared by the next passing check, which can happen before the backend counts as healthy again (`rise`).

On a terminal the states are colored; set `NO_COLOR` to turn that off. Piped output has no escape sequences.

### Options

| Option | Effect |
| --- | --- |
| `--config <path>` | Config file to take `[store] path` from |
| `--service <name>` | Show one service |
| `--json` | One JSON object: `store`, `daemon` (`running`, `stale`, `stopped`), `heartbeat_ms`, `heartbeat_age_ms`, `started_at_ms`, `site`, and `services` (the store's service records as written) |
| `--watch` | Redraw every second until Ctrl-C; needs a terminal, not with `--json` |

### Exit status

| Status | When |
| --- | --- |
| 0 | The daemon is running and every shown service has a healthy backend |
| 1 | The daemon is stale or stopped, or a shown service is `ALL DOWN` |
| 2 | The config or store cannot be read, the store has another format version, or `--service` names no service |

A single unhealthy backend does not change the status: failing over is the daemon's job. This makes `emfor status` usable as a monitoring check as it is, for example from cron or a Nagios-style plugin; add `--service` to alert on one service only.

### Watching

`emfor status --watch` redraws every second. When a backend's state or failure message changes, or a service turns `ALL DOWN` or back, that row is shown in reverse video for one frame. Below the services it lists the last 10 state changes it has seen since it started, newest first:

```
recent changes:
  14:03:12 api.svc.emfor 10.0.0.12:8080 healthy -> unhealthy
  14:01:40 daemon stopped -> running
```

The store keeps only the current state, so changes from before `--watch` started are not shown; use the daemon's log for history.
