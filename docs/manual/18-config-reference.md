# 18. Configuration reference

[Contents](README.md) · Previous: [17. Migrating](17-migrating.md) · Next: [19. Glossary](19-glossary.md)

Every key emfor 0.7.0 accepts. Any other key is an error (`<path>: unknown
key`). The options of each check are in [chapter 6](06-health-checks.md)
(`tcp`, `http`, `exec`) and [chapter 7](07-plugins.md) (`emfor-check`).

## Value formats

| Type | Form | Examples |
| --- | --- | --- |
| duration | A string of number-and-unit terms; units `ms`, `s`, `m`, `h` (and spelled out, `seconds`, `minutes`...). `"0"` alone is allowed. A bare number is refused. | `"500ms"`, `"1.5s"`, `"1h 30m"` |
| percentage | A string with `%` | `"10%"` |
| size | A string with `B`, `KiB`, `MiB` or `GiB` | `"64MiB"` |
| mode | An octal string, at most `0777` | `"0640"` |
| address | `"IPv4:port"` or `"[IPv6]:port"`, port 1–65535, never a host name | `"10.0.0.11:8080"`, `"[2001:db8::11]:8080"` |
| site | 1–63 of `a-z`, `0-9`, `-`, not starting or ending with `-` | `"dc1"` |
| service name | Labels of `a-z`, `0-9`, `-` (1–63 each), dots between, at most 253; case and a trailing dot are ignored | `"db.svc.emfor"` |

## Top level

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `site` | site | none | This host's data centre. Required with `[peers]`; used by `locality`. |
| `include_dir` | path | none | A directory of `*.toml` files merged in name order; relative to the main file. **Main file only.** Must come before the first table. |

## [store]

**Main file only. No key can be changed by a reload.**

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `path` | path | `/var/lib/emfor/state.mdb` | Clients read `EMFOR_STORE`, so keep the two equal. The directory must exist. |
| `map_size` | size | `"64MiB"` | At least `1MiB`, rounded up to 64 KiB. |
| `file_mode` | mode | `"0640"` | Set on the store at every start. |
| `lock_file_mode` | mode | `"0660"` | Set on `<path>-lock` at every start. A mode others can write gives a warning. |
| `sync` | boolean | `false` | `true` flushes every commit to disk before it completes. |

## [log]

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `level` | `"trace"`, `"debug"`, `"info"`, `"warn"`, `"error"` | `"info"` | `RUST_LOG` overrides it. |
| `format` | `"text"`, `"json"` | `"text"` | `"json"` writes one JSON object per line ([chapter 12](12-operating.md#json-lines)). Restart to change; a reload that changes it is rejected. |

## [dns]

The DNS server runs only when this table exists.

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `listen` | array of addresses | `["127.0.0.1:5300"]` | Not empty, no duplicates. Restart to change. |
| `zone` | name | `"svc.emfor"` | At most 242 characters. |
| `ttl` | duration, whole seconds | `"1s"` | 0–3600. |
| `rate_limit` | integer | `100` | UDP answers per second per /24 or /56; 0–1,000,000; `0` is off. |

## [metrics]

The metrics server runs only when this table exists.

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `listen` | array of addresses | `["127.0.0.1:5301"]` | Restart to change. |

## [peers]

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `listen` | address (a string) | required | Not `0.0.0.0` or `::`; one of `members`. Restart to change. |
| `members` | array of addresses | required | 2–64, no duplicates, one address family. |
| `secret_file` | absolute path | required | Owner root or the daemon's user; not readable by others; not writable by group or others; 32 bytes to 64 KiB. |
| `previous_secret_file` | absolute path | none | Same rules. Read at start only. |
| `encrypt` | boolean | `true` | |
| `accept_plaintext` | boolean | `false` if `encrypt`, else `true` | `false` with `encrypt = false` is an error. |
| `cipher` | string | `"chacha20-poly1305"` | `"chacha20-poly1305"` or `"aes-256-gcm"`; the latter needs `encrypt = true`. See [chapter 11](11-peer-groups.md#choosing-aes-gcm). |

## [defaults]

Settings for every service; a service overrides each one.

| Key | Type | Default | Rule |
| --- | --- | --- | --- |
| `interval` | duration | `"2s"` | 100 ms to 24 h. |
| `timeout` | duration | `"1s"` | At least 10 ms, at most `interval`. |
| `rise` | integer | `2` | At least 1. |
| `fall` | integer | `3` | At least 1. |
| `jitter` | percentage | `"10%"` | 0–50%. |
| `on_all_down` | `"open"`, `"closed"` | `"open"` | |
| `balance` | `"round-robin"`, `"sticky"`, `"failover"`, `"weighted"`, `"locality"`, `"fastest"` | `"round-robin"` | |
| `peers` | boolean | `true` | `false` keeps services out of the peer group on this host. |
| `slow_start` | duration | `"0s"` | `0s` or 1 s to 1 h. |

## [plugins."NAME"]

`NAME` is 1–63 of `a-z`, `0-9`, `_`, `-`, not `tcp`, `http` or `exec`.

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `command` | array of strings | required | The first element is an absolute path to an executable. |
| `env_pass` | array of variable names | `[]` | Not `EMFOR_IP`, `EMFOR_PORT`, `EMFOR_SERVICE`. Changing it restarts the plugin. |

## [services."NAME"]

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `check` | `"tcp"`, `"http"`, `"exec"` or a plugin name | required | |
| `backends` | array of addresses or backend tables | required | At least one; no duplicates; at least one not a backup. |
| `check_options` | table | `{}` | Depends on `check`. |
| `dns_ttl` | duration, whole seconds | `[dns] ttl` | 0–3600. Not allowed in `[defaults]`. |
| every key of `[defaults]` | | from `[defaults]` | |

A backend table:

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `addr` | address | required | |
| `weight` | integer | `1` | 1–100. Used by `weighted` (also its slow start) and by `sticky` with a key; other methods ignore it. |
| `site` | site | none | Used by `locality`. |
| `backup` | boolean | `false` | Used only while no primary passes. |
| `drain` | boolean | `false` | Never chosen; checks go on. A reload, no state reset. |

## Reloading

`[store]`, the `listen` keys of `[dns]`, `[metrics]` and `[peers]`, and adding
or removing those tables need a restart; everything else is applied by a
reload. What each change does to the backends' states is in
[chapter 5](05-configuring.md#applying-a-change-reload).

## A complete example

```toml
site = "dc1"

[store]
path = "/var/lib/emfor/state.mdb"
map_size = "64MiB"

[log]
level = "info"

[dns]
listen = ["127.0.0.1:5300"]
zone = "svc.emfor"
ttl = "1s"

[metrics]
listen = ["127.0.0.1:5301"]

[defaults]
interval = "2s"
timeout = "1s"
rise = 2
fall = 3
jitter = "10%"

[plugins."pgsql"]
command = ["/usr/bin/emfor-check", "pgsql"]

[services."api.svc.emfor"]
check = "http"
balance = "locality"
slow_start = "30s"
backends = [
  { addr = "10.1.0.11:8080", site = "dc1" },
  { addr = "10.1.0.12:8080", site = "dc1", weight = 2 },
  { addr = "10.2.0.11:8080", site = "dc2", backup = true },
]
check_options = { path = "/healthz", expect_status = ["200"] }

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
on_all_down = "closed"
dns_ttl = "0s"
backends = ["10.1.3.1:5432", "10.1.3.2:5432"]
check_options = { user = "emfor", database = "postgres", role = "primary", password_file = "/etc/emfor/secrets/pg.pass" }

[services."ssh.svc.emfor"]
check = "tcp"
interval = "10s"
backends = ["10.1.0.11:22", "10.1.0.12:22"]
check_options = { expect = "SSH-" }
```

The commented example config of the repository,
[examples/emfor.toml](../../examples/emfor.toml), lists every key too.
