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
| `include_dir` | none | A directory of further `.toml` files that are merged into this config ([below](#splitting-the-config-over-a-directory)). Only allowed in the main file. Must come before the first table. |

## [store]

```toml
[store]
path = "/var/lib/emfor/state.mdb"
map_size = "64MiB"
file_mode = "0640"
lock_file_mode = "0660"
sync = false
```

| Key | Default | Meaning |
| --- | --- | --- |
| `path` | `/var/lib/emfor/state.mdb` | The store file. Clients look here unless `EMFOR_STORE` says otherwise. |
| `map_size` | `"64MiB"` | Largest size the store can grow to; at least `1MiB`, rounded up to a multiple of `64KiB` (LMDB needs a multiple of the page size). Commits fail with `MDB_MAP_FULL` when it is reached. |
| `file_mode` | `"0640"` | Mode of the store file, set at every start on a new or an existing file. Applications need to read it, so their users have to be in the daemon's group; `"0644"` lets every local user read it. |
| `lock_file_mode` | `"0660"` | Mode of `<path>-lock`, set at every start. Applications need to write it, so their users have to be in the daemon's group; `"0666"` lets every local user, and `emfor check-config` and the daemon warn about a mode other users can write. See [who can use the store](08-operations.md#who-can-use-the-store). Changed from `"0644"` and `"0666"` in 0.7.0. |
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
| `zone` | `"svc.emfor"` | Only names under the zone are answered. At most 242 characters. A service in the zone must leave room for the SRV target name of each backend, `<ip label>.<service>` in at most 253 characters (an IPv6 label is 39). |
| `ttl` | `"1s"` | TTL of answers and of the zone SOA; whole seconds from `0s` to `3600s`. |
| `rate_limit` | `100` | UDP responses a second for each client network (IPv4 /24, IPv6 /56); `0` is no limit. Loopback clients and TCP are never limited. See [Rate limit](07-dns.md#rate-limit). |

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
decide and check the backend themselves about every third `interval`, and
recheck every 200ms when their own check fails while the others see the
backend up. The
vote and the split need no setting of their own; they use the services'
`interval`, and only members whose `check`, `check_options` and `timeout`
match count. Clients still read only
the local store. Without `[peers]` nothing of this runs. See
[chapter 1](01-concepts.md#peer-groups).

A service with `peers = false` (in its table or in `[defaults]`) is left
out of the group on this host: it sends no reports or results, ignores
those it receives, and always decides from its own checks. The other
services stay in the group. The host announces the services it leaves out
in its heartbeats (as short hashes of their names, at most 80 of them), and
the other members then do not count it as one of those services' assigned
checkers, so their backends keep three where enough members are live. Each
host can therefore leave out its own services. Peer groups and this
announcement both came in 0.3.0, and 0.2.0 refuses a config with
`[peers]`, so every member reads it. With more than 80 services left out
the host logs a warning and the others still count it for the rest.

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
| `secret_file` | required | Absolute path to the shared secret that authenticates and encrypts every message: at least 32 bytes after one trailing line feed is removed, not readable by other users and not writable by group or others (`chmod 600` or `640`), and owned by root or by the user running emfor (any group). |
| `previous_secret_file` | none | Optional. Absolute path to the secret `secret_file` held before its last change, with the same conditions. Read only at start, so that a daemon restarted during a secret change keeps signing with and accepting the previous secret as if it had not restarted (see below). |
| `encrypt` | `true` | `true` sends every message encrypted, with keys derived from the secret: `EMP3` (XChaCha20-Poly1305, 24-byte random nonce) to members that run this release, `EMP2` (ChaCha20-Poly1305, 12-byte random nonce) to the others, picked per member as described below. `false` sends them only signed (`EMP1`: HMAC-SHA256), so backend names, addresses and reasons can be read on the network. `EMP2` and `EMP3` are accepted either way; `EMP1` only while `accept_plaintext` is `true`, and while it is accepted and `encrypt` is `true` the daemon logs a warning, once a minute per member, for each member that still sends `EMP1`. |
| `accept_plaintext` | `false` when `encrypt` is `true`, `true` when `encrypt` is `false` | `false` drops every `EMP1` message (counted as `plaintext` in `emfor_peer_messages_rejected_total`), so nobody who learns the secret can make this daemon read a signed but unencrypted message. With the default, a daemon that encrypts therefore does not hear a member that has `encrypt = false` or runs 0.3.0: that member is not live here. Set `true` on a host while some member of the group still sends `EMP1` (see below); `false` needs `encrypt = true`. Before 0.7.0 the default was `true` whatever `encrypt` was. |

Members only share reports for a service when its check is defined the
same way on both: the same `check`, `check_options` and `timeout`. Other
settings (`interval`, `rise`, `fall`, the backend list) may differ.

The secret file is read again every second. To change the secret, write the
new one on every host within a minute of each other: after its own file
changed, a host still signs with the previous secret for 60 seconds and
accepts it for 120 seconds, so no message is dropped meanwhile. If the file
changes again within those 60 seconds (say, to fix a wrong secret), the
host keeps signing with the secret it signed with before, for 60 seconds
after the latest change.

The previous secret is kept only in memory. A daemon restarted during a
change knows only the new secret: members whose file has not changed yet
drop its messages (`auth`), and it theirs, until their files change. To
restart safely during a change, keep the old secret in a second file and
name it in `previous_secret_file`, for example:

```sh
cp -p /etc/emfor/peers.key /etc/emfor/peers.key.old  # keeps the mode
cat new.key > /etc/emfor/peers.key  # the same new secret on every host; keeps the mode
```

A daemon that starts within 120 seconds of the change to `secret_file`
(by the file's modification time) then signs with the old secret until 60
seconds after that change and accepts it until 120 seconds after it. Later
starts ignore `previous_secret_file`, so it can stay in the config.

Messages more than 5 seconds off the receiver's clock are dropped, so keep
the hosts' clocks in sync (NTP).

With `encrypt = true` the format is picked per member. Every `EMP2`
message this release sends carries `"emp3": true` in its encrypted
payload; a member is sent `EMP3` once the latest encrypted message accepted
from it is `EMP3` or such an `EMP2`, and `EMP2` otherwise: before any
message from it was accepted, after a start or a reload that added it, and
when it sends `EMP2` without the field (0.4.0) or `EMP1`. Each change is logged at
`info` ("sending EMP3 to peer ... from now on"). `EMP3` exists because a
12-byte random nonce is safe for about 2^32 messages per secret, which a
large group can send within days; a 24-byte one has no such limit. A
group running 0.4.0 needs no configuration change to upgrade: upgrade the
hosts one at a time, and members of this release use `EMP3` between each
other after their first message, `EMP2` with 0.4.0 members. Going back to
0.4.0 needs no change either, with `encrypt` on or off: the other members
send the host `EMP2` again once they accept its first message. Until then,
about one second after it starts, it drops their `EMP3` messages (counted
as `malformed`).

Encryption was added in 0.4.0. Members running 0.3.0 send and accept
only `EMP1` and drop encrypted messages (counted as `malformed`), so to
upgrade a group from 0.3.0, set `encrypt = false` on each host as it is
upgraded; once every member runs this release, remove the line (or set
`encrypt = true`) and reload each daemon. From 0.7.0 a daemon with
`encrypt = true` drops the plaintext messages of a member that has not
reloaded yet, so while any member still sends plaintext, give the hosts
that already encrypt an explicit `accept_plaintext = true`: in the same
config change that upgrades a host from 0.3.0 or 0.4.0 (they do not know
the key, which is from 0.5.0), or before the upgrade on a host that runs
0.5.0 or 0.6.0. Once every member has `encrypt = true` and
`emfor_peer_messages_rejected_total{reason="plaintext"}` stops
increasing, remove `accept_plaintext = true` and reload. The encryption has no forward
secrecy: whoever later gets the secret can read messages recorded before.
Rotate the secret regularly, as above, and keep the file readable only by
the daemon.

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
slow_start = "0s"
```

| Key | Rule | Meaning |
| --- | --- | --- |
| `interval` | `100ms` to `24h` | Time from the end of one check of a backend to the start of the next. |
| `timeout` | at least `10ms`, at most `interval` | How long one check may take. A plugin gets it in whole milliseconds, less a small margin. |
| `rise` | at least 1 | Consecutive passes before a backend is healthy. |
| `fall` | at least 1 | Consecutive failures before a backend is unhealthy. |
| `jitter` | `0%` to `50%` | Random spread of `interval`, so checks do not line up. |
| `on_all_down` | `open` or `closed` | What lookups do when no backend is healthy ([chapter 5](05-backend-selection.md#when-no-backend-is-healthy)). |
| `balance` | see [chapter 5](05-backend-selection.md) | How a backend is chosen. |
| `peers` | `true` or `false` | Whether the service takes part in the [peer group](#peers). No effect without `[peers]`. |
| `slow_start` | `0s` (off), or `1s` to `1h` | How long a backend that recovers gets a growing share of traffic ([chapter 5](05-backend-selection.md#slow-start)). |

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
| `interval`, `timeout`, `rise`, `fall`, `jitter`, `on_all_down`, `balance`, `peers`, `slow_start` | no | Override `[defaults]` for this service. |
| `dns_ttl` | no | Overrides `[dns] ttl` for this service; whole seconds, `0s` to `3600s`. |

**Service names** are host-name-like: lowercased, one trailing dot
removed, at most 253 characters of dot-separated labels, each 1–63
characters of `a-z`, `0-9` and `-` not starting or ending with `-`. Two
names that differ only in case are duplicates. Clients normalize the name
they look up the same way.

**Backends** are strings, or tables when they need a weight, a site,
`backup` or `drain`:

```toml
backends = [
  "10.0.4.11:9200",
  { addr = "10.0.4.12:9200", weight = 3 },
  { addr = "10.1.4.11:9200", site = "dc2" },
  { addr = "10.2.4.11:9200", backup = true },
  { addr = "10.0.4.13:9200", drain = true },
]
```

| Key | Default | Meaning |
| --- | --- | --- |
| `addr` | required | `IPv4:port` or `[IPv6]:port`, port 1–65535. Unique within the service. |
| `weight` | 1 | 1 to 100. Used by `balance = "weighted"`, and by `"sticky"` with an affinity key. |
| `site` | none | The backend's data centre. Used by `balance = "locality"`. |
| `backup` | `false` | `true` or `false`. A backup is used only while no other backend passes its checks ([chapter 5](05-backend-selection.md#backup-backends)). At least one backend of a service must not be a backup. |
| `drain` | `false` | `true` or `false`. A drained backend is never chosen, by any `balance` or in DNS, while its checks go on ([chapter 5](05-backend-selection.md#draining-a-backend)). |

`weight`, `site`, `backup` and `drain` are accepted under every `balance`, so you
can switch `balance` without editing the backends.

## [plugins."name"]

```toml
[plugins."redis"]
command = ["/usr/local/libexec/emfor-check", "redis"]
```

| Key | Meaning |
| --- | --- |
| `command` | Required. The program and its arguments; the first element is an absolute path to an executable. No element may contain NUL. Run without a shell. |

The name is 1–63 characters of `a-z`, `0-9`, `_` and `-`, and not `tcp`,
`http` or `exec`. A service uses the plugin with `check = "<name>"`. Every
declared plugin is started, used or not. See
[chapter 4](04-health-checks.md#plugins-and-emfor-check).

## Splitting the config over a directory

With `include_dir = "conf.d"` in the main file, the daemon reads the main
file and then every `.toml` file in that directory, and merges them into
one config before checking it. A relative path is relative to the directory
of the main file (`/etc/emfor/conf.d` for `/etc/emfor/emfor.toml`). This
lets a package own `emfor.toml` while each team or configuration tool owns
the files for its services:

```
/etc/emfor/emfor.toml            include_dir = "conf.d", [store], [dns]
/etc/emfor/conf.d/01-plugins.toml
/etc/emfor/conf.d/10-web.toml
/etc/emfor/conf.d/99-local.toml
```

**Which files, in what order.** Regular files, and links to regular
files, whose name ends in `.toml` and does not start with `.`. They are
read in the byte order of their names, not the order of your locale, and a
later file overrides an earlier one; the main file is the lowest layer.
Write the numbers with leading zeros: `9-a.toml` sorts after `10-a.toml`.
Everything else is ignored (`web.toml~`, `.web.toml.swp`,
`web.toml.disabled`, subdirectories), so renaming a file takes it out of
the config. A name that is not valid UTF-8 is skipped with a warning in
the log. An empty directory is fine.

**How files merge.** Two tables merge key by key, as deep as they go.
Anything else is replaced as a whole by the later value: a number, a string,
a list (including `backends` and `command`), or a value of another type.

```toml
# emfor.toml
[services."api.svc.emfor"]
check = "tcp"
backends = ["10.0.0.1:80", "10.0.0.2:80"]
interval = "2s"
```

```toml
# conf.d/99-local.toml
[services."api.svc.emfor"]
interval = "5s"
```

The service keeps its `check` and both `backends` and gets `interval =
"5s"`. To change the backends, give the whole list again. A later file
cannot remove a service, a plugin or a backend; take it out of the file
that defines it, or rename that file so it is ignored. A service whose
merged table has no `check` or no `backends` is an error, as in a single
file. The merged config is checked, started, reloaded and shared with peers
exactly like the same text in one file; how it is split does not change
the service definitions peers compare.

**Only in the main file.** `[store]` and `include_dir` itself. `emfor
status` and `emfor lookup` read only the main file, so a fragment cannot
move the store, and a fragment cannot include another directory. Anyone
who only runs `status` and `lookup` needs to read the main file, not the
directory. Everything else, including `site`, `[dns]`, `[peers]` and
`[plugins]`, may be in any file.

**Permissions.** The directory and every file read from it must not be
writable by group or others (mode `0755` and `0644` are fine, `0775` and
`0664` are not); otherwise the config is refused and the error names the
path and mode. The owner must be root or the user running emfor (for a
link, the owner of the file it points to); otherwise the config is refused
too, because a file owned by anyone else can be changed by that user. A
fragment can declare a plugin `command` or an `exec` check, so whoever can
write there can run programs as the daemon's user. Run
`emfor check-config` as the user the daemon runs as (the systemd unit and
the FreeBSD `configtest` do); run by hand as root, it accepts only files
owned by root.
The main file is not checked this way, so protect it yourself (`0644`,
owned by root).

**The directory must exist.** A package creates it. A config whose
`include_dir` does not exist is an error: the daemon logs it and exits 2 at
start, and a reload is rejected and the old config keeps running.

**Errors name their file.** A TOML syntax error is reported as
`file:line:column`, and any other error as its TOML path followed by the
file that set that key (`services."api.svc.emfor".interval: ... (in
/etc/emfor/conf.d/99-local.toml)`). When an error comes from values in two
files, such as a `timeout` in one and an `interval` in another, the file
named is the later one.

**Writing the files.** Write a file to a temporary name outside the
directory or ending in something other than `.toml`, then rename it into
place. A reload that reads a half-written file is rejected and the old
config stays, but an atomic rename avoids the failed reload.

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

With `include_dir` set, it then lists the files it read, the main file
first and the others in the order they were merged:

```
config ok: 8 services, 19 backends
read /etc/emfor/emfor.toml
read /etc/emfor/conf.d/10-web.toml
read /etc/emfor/conf.d/99-local.toml
```

The daemon logs the same list at start and after each reload. Without
`include_dir` the output is the single line.

**Seeing the merged config.** `emfor check-config --show` checks the config
the same way, and then prints the merged config as TOML on standard output,
each value followed by the file that last set it:

```
# include_dir = "conf.d" (its files are merged below)

[services."api.svc.emfor"]
backends = ["10.0.0.1:80", "10.0.0.2:80"] # /etc/emfor/emfor.toml
check = "tcp" # /etc/emfor/emfor.toml
interval = "5s" # /etc/emfor/conf.d/99-local.toml
```

A list such as `backends` is set as a whole, so its comment names the file
that set the list. The `config ok` and `read` lines go to standard error, so
`emfor check-config --show > merged.toml` saves a file that checks again
anywhere, because the `include_dir` line is written as a comment (its files
are merged already); comments aside the file has the same content. Relative
paths in the config other than `include_dir` do not depend on the file's
directory. An invalid config prints nothing
on standard output and exits 2. Without `include_dir` the output starts with
one comment naming the file and has no other comments. The built-in defaults
are not added: a key you did not write does not appear. Nothing is
redacted; the config holds password file paths, not passwords, but a plugin
`command` is printed as written, so do not put a secret there. The comment
format is for people; do not parse it.

It exits 0 when the config is valid and 2 otherwise (1 when it cannot write its output, for example to a full disk; a closed pipe is not an error). It starts the declared
plugins so that they can check their services' `check_options`, and stops
them before it exits. It does not open the store. A daemon started with an
invalid config logs the same errors and exits 2.

## What a reload changes

`SIGHUP` (`systemctl reload emfor`, `service emfor reload`) re-reads the
file, and the files of `include_dir` with it. A config with any error, in
any file, or with an `include_dir` that has gone missing, is rejected as a
whole: the daemon logs every
error and keeps running the old config.

| Change | On reload |
| --- | --- |
| Services and backends added or removed | Applied; new backends start `unknown`. |
| `check` or `check_options` of a service | Applied; its backends start over from `unknown`. |
| `interval`, `timeout`, `rise`, `fall`, `jitter`, `on_all_down`, `peers` | Applied; backends keep their state. A new `interval` or `jitter` also applies to the wait already under way: the next check is due the new wait after the last one ended, at once when that is past. |
| `balance`, `weight`, `site` (top level and backends), `backup`, `drain`, `dns_ttl`, `slow_start` | Applied; backends keep their state, and a backend in slow start keeps its recovery time. |
| `[dns] zone`, `[dns] ttl`, `[dns] rate_limit` | Applied. |
| `[log] level` | Applied (unless `RUST_LOG` is set). |
| A plugin's `command` | The plugin is restarted, and the services that use it start over from `unknown`. Unchanged plugins keep running. |
| Anything in `[store]` | Rejected; needs a restart. |
| `[dns] listen`, adding or removing `[dns]` | Rejected (`dns listen settings require restart`). |
| `[metrics] listen`, adding or removing `[metrics]` | Rejected (`metrics listen settings require restart`). |
| `[peers] members`, `[peers] secret_file`, `[peers] encrypt`, `[peers] accept_plaintext` | Applied; added members start as not live. `previous_secret_file` is checked, but read only at start. |
| `[peers] listen`, adding or removing `[peers]` | Rejected (`peers listen settings require restart`). |
