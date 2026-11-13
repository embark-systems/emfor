# 3. Configuration

[Manual](README.md) · Previous: [2. Installation](02-installation.md) · Next: [4. Health checks](04-health-checks.md)

The daemon reads one TOML file, `/etc/emfor/emfor.toml` by default
([chapter 2](02-installation.md#the-default-config-path)). Every key is
optional except the services' `check` and `backends`.
[examples/emfor.toml](../../examples/emfor.toml) shows every option with
comments.

## Value formats

| Kind | Format | Examples |
| --- | --- | --- |
| Duration | A string with a unit | `"500ms"`, `"2s"`, `"1m"` |
| Size | A string with a unit | `"64MiB"` |
| File mode | An octal string | `"0644"` |
| Percentage | A string ending in `%` | `"10%"` |
| Address | `IPv4:port` or `[IPv6]:port` | `"10.0.0.11:8080"`, `"[2001:db8::11]:8080"` |

Unknown keys anywhere in the file are errors, so a misspelled key never
goes unnoticed.

## Top level

```toml
site = "dc1"
```

| Key | Default | Meaning |
| --- | --- | --- |
| `site` | none | The data centre this host is in. Used by `balance = "locality"`, and required with `[peers]`. 1–63 characters of `a-z`, `0-9` and `-` (lowercased when read). Must come before the first table. |

## [store]

```toml
[store]
path = "/var/lib/emfor/state.mdb"
map_size = "64MiB"
file_mode = "0644"
lock_file_mode = "0666"
sync = false
```

| Key | Default | Meaning |
| --- | --- | --- |
| `path` | `/var/lib/emfor/state.mdb` | The store file. Clients look here unless `EMFOR_STORE` says otherwise. |
| `map_size` | `"64MiB"` | Largest size the store can grow to; at least `1MiB`. Commits fail with `MDB_MAP_FULL` when it is reached. |
| `file_mode` | `"0644"` | Mode the store file is created with. Applications need to read it. |
| `lock_file_mode` | `"0666"` | Mode of `<path>-lock`. Applications need to write it. See [who can use the store](08-operations.md#who-can-use-the-store) before keeping the default. |
| `sync` | `false` | `true` flushes every commit to disk. See [the store on disk](08-operations.md#the-store-on-disk). |

## [log]

| Key | Default | Meaning |
| --- | --- | --- |
| `level` | `"info"` | `trace`, `debug`, `info`, `warn` or `error`. When the environment variable `RUST_LOG` is set, it overrides this for the whole run. |

## [dns]

The DNS server runs only when the table is present; an empty `[dns]` table
turns it on with the defaults. See [chapter 7](07-dns.md).

| Key | Default | Meaning |
| --- | --- | --- |
| `listen` | `["127.0.0.1:5300"]` | Addresses to answer on, UDP and TCP. |
| `zone` | `"svc.emfor"` | Only names under the zone are answered. At most 242 characters. |
| `ttl` | `"1s"` | TTL of answers and of the zone SOA; whole seconds from `0s` to `3600s`. |

## [metrics]

The Prometheus endpoint runs only when the table is present.

| Key | Default | Meaning |
| --- | --- | --- |
| `listen` | `["127.0.0.1:5301"]` | Addresses to serve `/metrics` on. |

## [peers]

Optional. Makes this daemon a member of a **peer group**: the emfor daemons
of one `site` that list each other here. Members send each other a
heartbeat every second, and when one of them sees a backend go from
`healthy` to `unhealthy` it tells the others, which recheck that backend at
once and every 200ms (up to `fall` checks) instead of waiting for their
next `interval`. With three or more live members, three of them are
assigned to each backend: they check it every `interval` and send their
results to the others; two `down` results among those three mark the
backend down on every member. The other members let two agreeing results
decide and check the backend themselves about every fifth `interval`. The
vote and the split need no setting of their own; they use the services'
`interval`, and only members whose `check`, `check_options` and `timeout`
match count. Clients still read only
the local store. Without `[peers]` nothing of this runs. See
[chapter 1](01-concepts.md#peer-groups).

A service with `peers = false` (in its table or in `[defaults]`) is left
out of the group on this host: it sends no reports or results, ignores
those it receives, and always decides from its own checks. The other
services stay in the group. The other members do not know which services
a host leaves out and may still count it as one of a backend's three
assigned checkers, leaving that backend with two; so set `peers` the same
on every member.

```toml
site = "dc1"                               # required with [peers]

[peers]
listen = "10.0.0.5:5302"
members = ["10.0.0.5:5302", "10.0.0.6:5302", "10.0.0.7:5302"]
secret_file = "/etc/emfor/peers.key"
```

| Key | Default | Meaning |
| --- | --- | --- |
| `listen` | required | This daemon's UDP address in the group, one of `members`. A specific address, not `0.0.0.0` or `::`; other members check that messages come from it. 5302 is the usual port. |
| `members` | required | Every member's `listen`, this daemon included: 2 to 64 addresses, no repeats, all IPv4 or all IPv6 like `listen`. The same list can go on every host of the site. |
| `secret_file` | required | Absolute path to the shared secret that signs every message (HMAC-SHA256): at least 32 bytes after one trailing line feed is removed, not readable by other users (`chmod 600` or `640`). |

Members only share reports for a service when its check is defined the
same way on both: the same `check`, `check_options` and `timeout`. Other
settings (`interval`, `rise`, `fall`, the backend list) may differ.

The secret file is read again every second. To change the secret, write the
new one on every host within a minute of each other: after its own file
changed, a host still signs with the previous secret for 60 seconds and
accepts it for 120 seconds, so no message is dropped meanwhile. Messages more than 5 seconds off the
receiver's clock are dropped, so keep the hosts' clocks in sync (NTP).

## [defaults]

Values every service uses unless it sets its own. The built-in values:

```toml
[defaults]
interval = "2s"
timeout = "1s"
rise = 2
fall = 3
jitter = "10%"
on_all_down = "open"
balance = "round-robin"
peers = true
```

| Key | Rule | Meaning |
| --- | --- | --- |
| `interval` | at least `100ms` | Time from the end of one check of a backend to the start of the next. |
| `timeout` | above 0, at most `interval` | How long one check may take. |
| `rise` | at least 1 | Consecutive passes before a backend is healthy. |
| `fall` | at least 1 | Consecutive failures before a backend is unhealthy. |
| `jitter` | `0%` to `50%` | Random spread of `interval`, so checks do not line up. |
| `on_all_down` | `open` or `closed` | What lookups do when no backend is healthy ([chapter 5](05-backend-selection.md#when-no-backend-is-healthy)). |
| `balance` | see [chapter 5](05-backend-selection.md) | How a backend is chosen. |
| `peers` | `true` or `false` | Whether the service takes part in the [peer group](#peers). No effect without `[peers]`. |

## [services."name"]

```toml
[services."api.svc.emfor"]
check = "http"
backends = ["10.0.0.11:8080", "10.0.0.12:8080"]
interval = "1s"
dns_ttl = "5s"

[services."api.svc.emfor".check_options]
path = "/healthz"
```

| Key | Required | Meaning |
| --- | --- | --- |
| `check` | yes | `tcp`, `http`, `exec`, or the name of a declared plugin. |
| `backends` | yes | A non-empty list of backends (below). |
| `check_options` | no | Options of the checker ([chapter 4](04-health-checks.md)). |
| `interval`, `timeout`, `rise`, `fall`, `jitter`, `on_all_down`, `balance`, `peers` | no | Override `[defaults]` for this service. |
| `dns_ttl` | no | Overrides `[dns] ttl` for this service; whole seconds, `0s` to `3600s`. |

**Service names** are host-name-like: lowercased, one trailing dot
removed, at most 253 characters of dot-separated labels, each 1–63
characters of `a-z`, `0-9` and `-` not starting or ending with `-`. Two
names that differ only in case are duplicates. Clients normalize the name
they look up the same way.

**Backends** are strings, or tables when they need a weight or a site:

```toml
backends = [
  "10.0.4.11:9200",
  { addr = "10.0.4.12:9200", weight = 3 },
  { addr = "10.1.4.11:9200", site = "dc2" },
]
```

| Key | Default | Meaning |
| --- | --- | --- |
| `addr` | required | `IPv4:port` or `[IPv6]:port`, port 1–65535. Unique within the service. |
| `weight` | 1 | 1 to 100. Used by `balance = "weighted"`. |
| `site` | none | The backend's data centre. Used by `balance = "locality"`. |

`weight` and `site` are accepted under every `balance`, so you can switch
`balance` without editing the backends.

## [plugins."name"]

```toml
[plugins."redis"]
command = ["/usr/local/libexec/emfor-check", "redis"]
```

| Key | Meaning |
| --- | --- |
| `command` | Required. The program and its arguments; the first element is an absolute path to an executable. Run without a shell. |

The name is 1–63 characters of `a-z`, `0-9`, `_` and `-`, and not `tcp`,
`http` or `exec`. A service uses the plugin with `check = "<name>"`. Every
declared plugin is started, used or not. See
[chapter 4](04-health-checks.md#plugins-and-emfor-check).

## Checking a config

```sh
$ emfor check-config
config ok: 8 services, 19 backends
```

`emfor check-config` checks the whole file and prints every error, each
with its TOML path, for example:

```
services."api.svc.emfor".timeout: timeout must not exceed interval
```

It exits 0 when the config is valid and 2 otherwise. It starts the declared
plugins so that they can check their services' `check_options`, and stops
them before it exits. It does not open the store. A daemon started with an
invalid config logs the same errors and exits 2.

## What a reload changes

`SIGHUP` (`systemctl reload emfor`, `service emfor reload`) re-reads the
file. A config with any error is rejected as a whole: the daemon logs every
error and keeps running the old config.

| Change | On reload |
| --- | --- |
| Services and backends added or removed | Applied; new backends start `unknown`. |
| `check` or `check_options` of a service | Applied; its backends start over from `unknown`. |
| `interval`, `timeout`, `rise`, `fall`, `jitter`, `on_all_down`, `peers` | Applied; backends keep their state. |
| `balance`, `weight`, `site` (top level and backends), `dns_ttl` | Applied. |
| `[dns] zone`, `[dns] ttl` | Applied. |
| `[log] level` | Applied (unless `RUST_LOG` is set). |
| A plugin's `command` | The plugin is restarted, and the services that use it start over from `unknown`. Unchanged plugins keep running. |
| Anything in `[store]` | Rejected; needs a restart. |
| `[dns] listen`, adding or removing `[dns]` | Rejected (`dns listen settings require restart`). |
| `[metrics] listen`, adding or removing `[metrics]` | Rejected (`metrics listen settings require restart`). |
| `[peers] members`, `[peers] secret_file` | Applied; added members start as not live. |
| `[peers] listen`, adding or removing `[peers]` | Rejected (`peers listen settings require restart`). |
