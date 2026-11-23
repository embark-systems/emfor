# 5. Configuring emfor

[Contents](README.md) · Previous: [4. Installing](04-installing.md) · Next: [6. Health checks](06-health-checks.md)

This chapter is about the config as a whole: its format, how to split it over
several files, how to check it and how to apply a change. The checks are in
[chapter 6](06-health-checks.md) and [chapter 7](07-plugins.md), the choice of
a backend in [chapter 8](08-choosing-a-backend.md), and every key with its
default in [chapter 18](18-config-reference.md).

## The shape of a config

The config is one TOML file, `/etc/emfor/emfor.toml` by default. Only the
services' `check` and `backends` are required; everything else has a
default. A realistic small config:

```toml
site = "dc1"                       # this host's data centre (optional)

[log]
level = "info"

[dns]                              # optional DNS server, defaults
[metrics]                          # optional /metrics, defaults

[defaults]                         # apply to every service
interval = "2s"
timeout = "1s"

[plugins."pgsql"]
command = ["/usr/bin/emfor-check", "pgsql"]

[services."api.svc.emfor"]
check = "http"
backends = ["10.0.0.11:8080", "10.0.0.12:8080"]
check_options = { path = "/healthz" }

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
on_all_down = "closed"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
check_options = { user = "emfor", database = "postgres", role = "primary", password_file = "/etc/emfor/secrets/pg.pass" }
```

The tables:

| Table | What it holds |
| --- | --- |
| top level | `site` and `include_dir`. They must come before the first `[table]`, as TOML puts a key after a table header into that table. |
| `[store]` | Where the state store is, its size and its file modes. |
| `[log]` | The log level and format. |
| `[dns]` | The DNS server; it runs only when the table is present. |
| `[metrics]` | The Prometheus endpoint; it runs only when the table is present. |
| `[peers]` | The peer group ([chapter 11](11-peer-groups.md)). |
| `[defaults]` | Settings every service uses unless it sets its own. |
| `[plugins."<name>"]` | A plugin program that services can use as their `check`. |
| `[services."<name>"]` | One service: its check, its backends and its settings. |

**Unknown keys are errors**, anywhere in the file, so a misspelled key never
goes unnoticed. The flip side: a key that a newer release added makes an older
release refuse the config.

## Value formats

| Kind | Format | Examples |
| --- | --- | --- |
| Duration | A string: one or more number-and-unit terms. Units `ms`, `s`, `m`, `h`, `d` and spelled-out forms (`seconds`, `minutes`, ...); a decimal fraction is allowed; `"0"` alone is zero. | `"500ms"`, `"2s"`, `"1.5s"`, `"1h 30m"`, `"2 seconds"` |
| Size | A string with `KiB`, `MiB` or `GiB`, or a plain number of bytes | `"64MiB"` |
| File mode | An octal string | `"0640"` |
| Percentage | A string ending in `%` | `"10%"` |
| Address | `IPv4:port` or `[IPv6]:port`, port 1–65535 | `"10.0.0.11:8080"`, `"[2001:db8::11]:8080"` |

A bare number is not a duration: `interval = "2"` is refused with
`invalid duration "2", use e.g. "500ms" or "2s"`.

## Settings and defaults

Each service gets its timing and choice settings from, in order: its own
table, then `[defaults]`, then the built-in values:

```toml
[defaults]
interval = "2s"          # time from the end of one check to the start of the next
timeout = "1s"           # how long one check may take; at most interval
rise = 2                 # passes in a row before healthy
fall = 3                 # failures in a row before unhealthy
jitter = "10%"           # random spread of interval, 0% to 50%
on_all_down = "open"     # or "closed"
balance = "round-robin"
peers = true             # take part in the peer group, if there is one
slow_start = "0s"        # off; or 1s to 1h
```

That is exactly the built-in set, so this table changes nothing. A service
overrides any of them:

```toml
[services."report.svc.emfor"]
check = "tcp"
backends = ["10.0.9.1:5432"]
interval = "30s"
timeout = "5s"
fall = 2
```

## Splitting the config over a directory

With `include_dir`, the daemon reads the main file and then every `.toml`
file of a directory, and merges them into one config. This lets a package own
the main file while each team or tool owns the files of its services:

```
/etc/emfor/emfor.toml              include_dir = "conf.d", [dns]
/etc/emfor/conf.d/10-web.toml
/etc/emfor/conf.d/99-local.toml
```

```toml
# /etc/emfor/emfor.toml
include_dir = "conf.d"        # relative to this file's directory

[dns]
```

The deb and rpm packages ship exactly this layout, with a main file that holds
only the `include_dir` line.

**Which files, in which order.**

- Regular files, and links to regular files, whose name ends in `.toml` and
  does not start with `.`. Everything else is ignored: `web.toml~`,
  `.web.toml.swp`, `web.toml.disabled`, subdirectories. Renaming a file to
  `.disabled` takes it out.
- Read in the byte order of their names, not your locale's order. Use leading
  zeros: `9-a.toml` sorts after `10-a.toml`.
- The main file is the lowest layer; a later file overrides an earlier one.
- An empty directory is fine. A missing one is an error.

**How files merge.** Two tables merge key by key, as deep as they go.
Anything else (a number, a string, a list) is replaced as a whole by the later
value. So:

```toml
# /etc/emfor/conf.d/10-web.toml
[services."web.svc.emfor"]
check = "tcp"
backends = ["10.0.0.21:80", "10.0.0.22:80"]
```

```toml
# /etc/emfor/conf.d/99-local.toml
[services."web.svc.emfor"]
interval = "5s"
```

`web.svc.emfor` keeps its `check` and both backends and gets
`interval = "5s"`. To change the backends, give the whole list again. A later
file cannot remove a service, a plugin or a backend: take it out of the file
that defines it.

**Only in the main file**: `[store]` and `include_dir` itself. A fragment that
sets them is refused (`/etc/emfor/conf.d/60-s.toml: store is only allowed in
the main config file`). Everything else, `site`, `[dns]`, `[peers]` and
`[plugins]` included, may be in any file.

**Write files atomically.** Write the new file under a name that does not end
in `.toml` (or outside the directory), then `mv` it into place. A reload that
reads a half-written file is rejected, and the old config keeps running, but
the rename avoids the failed reload.

## Who may own and write the config files

A config can start programs (a plugin `command`, an `exec` check), which then
run as the daemon's user. So emfor refuses a config file that someone else
could change:

- The **main file**, the **`include_dir` directory** and **every file in it**
  must not be writable by group or others: modes `0644` and `0755` are fine,
  `0664` and `0775` are not.
- They must be **owned by root or by the user the daemon runs as**. For a
  link, the file it points to counts.

```console
# chmod 0664 /etc/emfor/emfor.toml
# emfor check-config
/etc/emfor/emfor.toml: writable by group or others (mode 0664); refusing to load it; to fix it: chmod g-w,o-w /etc/emfor/emfor.toml
```

The check applies whenever `emfor server` (at start and at every reload) and
`emfor check-config` read the config. Before 0.7.0 only the fragments were
checked, not the main file. `emfor status` and `emfor lookup` read only the
`[store]` table of the main file and do not check it. The directory that holds
the main file (`/etc/emfor`) is not checked; keep it writable by root only, as
the packages do.

The owner rule depends on who runs the command. Run `emfor check-config` as
the daemon's user, as the systemd unit and the FreeBSD `configtest` do. Run by
hand as root, it accepts only files owned by root.

## Checking a config

For the config at the top of this chapter:

```console
# emfor check-config
service db.svc.emfor: check_options has a password_file but tls is not true: the password is not protected on the network, and anyone who can see the traffic, or answer in the backend's place, can get it
config ok: 2 services, 4 backends
```

`emfor check-config` reads the whole config and prints **every** error, each
with its TOML path, and with the file when the config is split. Here the
split example above has one more file, `conf.d/50-x.toml`, that sets
`timeout = "10s"` for `web.svc.emfor`:

```console
# emfor check-config
services."web.svc.emfor".timeout: timeout must not exceed interval (in /etc/emfor/conf.d/50-x.toml)
```

When an error comes from values in two files (a `timeout` in one, an
`interval` in another), the file named is the one that last set the key in
the error's path, here `timeout`, even though `99-local.toml` set `interval`
later. When no file set that key itself, it is the last file that set the
nearest table above it.

What it does:

- It starts every declared plugin and lets it check the `check_options` of
  its services, then stops it. So a wrong plugin option is found before you
  reload.
- It reads password files, CA files and peer secret files as the checks would.
- It warns (without failing) about secret files other users can read, about
  passwords sent without TLS, and about store file modes that other users can
  write ([chapter 13](13-security.md)).
- It does not open the store and does not need the daemon to run.
- Exit status 0 when the config is valid, 2 when it is not, 1 when it cannot
  write its output (a full disk; a closed pipe is not an error).

With `include_dir` it then lists the files it read, in merge order:

```console
# emfor check-config
config ok: 1 services, 2 backends
read /etc/emfor/emfor.toml
read /etc/emfor/conf.d/10-web.toml
read /etc/emfor/conf.d/99-local.toml
```

### Seeing the merged config

`emfor check-config --show` prints the merged config as TOML on standard
output, each value followed by the file that last set it. The `config ok`
lines go to standard error.

```console
# emfor check-config --show 2>/dev/null
# include_dir = "conf.d" (its files are merged below)

[dns]

[services."web.svc.emfor"]
backends = ["10.0.0.21:80", "10.0.0.22:80"] # /etc/emfor/conf.d/10-web.toml
check = "tcp" # /etc/emfor/conf.d/10-web.toml
interval = "5s" # /etc/emfor/conf.d/99-local.toml
```

- Built-in defaults are not added: a key you did not write does not appear.
- Nothing is redacted. The config holds paths to password files, not
  passwords, but a plugin `command` is printed as written: do not put a secret
  there.
- `emfor check-config --show > merged.toml` gives a file that checks again
  anywhere, because the `include_dir` line becomes a comment.
- The comment format is for people; do not parse it.
- An invalid config prints nothing on standard output and exits 2.

## Applying a change: reload

Send the daemon `SIGHUP` to re-read the config, the `include_dir` files
included. Use your service manager, which checks the config first and does not
signal the daemon when it is invalid:

```sh
# systemctl reload emfor       # Linux with systemd
# service emfor reload         # FreeBSD
```

A reload is all or nothing. A config with any error, in any file, is rejected
as a whole; the daemon logs every error and keeps the old config:

```
ERROR emfor_daemon::cli: invalid config /etc/emfor/emfor.toml: 2 error(s)
ERROR emfor_daemon::cli: defaults.bogus: unknown key
ERROR emfor_daemon::cli: defaults.timeout: timeout must not exceed interval
ERROR emfor_daemon::cli: reload rejected; keeping the previous config
```

What a reload can change:

| Change | On reload |
| --- | --- |
| Services or backends added or removed | Applied. New backends start `unknown`. |
| `check` or `check_options` of a service | Applied. Its backends start over from `unknown`. |
| `interval`, `timeout`, `rise`, `fall`, `jitter`, `on_all_down`, `peers` | Applied; backends keep their state. A new `interval` or `jitter` also applies to the wait already under way. |
| `balance`, `weight`, `site`, `backup`, `drain`, `dns_ttl`, `slow_start` | Applied; backends keep their state, and a backend in slow start keeps its recovery time. |
| `[dns] zone`, `ttl`, `rate_limit` | Applied. |
| `[log] level` | Applied, unless the `RUST_LOG` environment variable is set. |
| `[log] format` | Rejected with `log format settings require restart`; restart. |
| A plugin's `command` or `env_pass` | The plugin is restarted, and the services that use it start over from `unknown`. Unchanged plugins keep running. |
| `[peers] members`, `secret_file`, `encrypt`, `accept_plaintext` | Applied. Added members start as not live. `previous_secret_file` is checked but only read at start. |
| Anything in `[store]` | Rejected: `store: store settings require restart`. |
| `[dns] listen`, adding or removing `[dns]` | Rejected: `dns.listen: dns listen settings require restart`. |
| `[metrics] listen`, adding or removing `[metrics]` | Rejected: `metrics.listen: metrics listen settings require restart`. |
| `[metrics] allow` | Rejected when it differs from the running one: `metrics.allow: metrics allow settings require restart`. |
| `[peers] listen`, adding or removing `[peers]` | Rejected: `peers.listen: peers listen settings require restart`. |

For the rejected ones, restart the daemon (`systemctl restart emfor`). A
restart has a short window in which lookups cannot tell good backends from bad
ones ([chapter 12](12-operating.md#what-applications-see-during-a-restart)), so
prefer a reload whenever it is enough.

## Generating the config

Since the backend list is config, something has to write it: your
configuration management (Ansible, Puppet, Salt, a template), or a script.
Some patterns that work well:

- One file per service (or per team) in `conf.d`, written whole by the tool.
- One backend per line, so a script can change a single line (the drain
  script of [chapter 12](12-operating.md#a-rolling-deploy-with-drain) relies on
  this).
- `[peers] members` alone in its own file, written from a cloud API
  ([chapter 11](11-peer-groups.md#members-from-a-cloud-api)).
- Always run `emfor check-config` before the reload, as the daemon's user.

## Using a configuration management tool

Ansible, Puppet, Salt and Chef can own the files of `conf.d` as they own any other
config. Five things need care, because the usual defaults of such tools do not
fit emfor.

1. **Reload, do not restart.** Most changes need only a reload (see the
   table above); a restart has a short window in which lookups cannot tell
   good backends from bad ones
   ([chapter 12](12-operating.md#what-applications-see-during-a-restart)).
   `systemctl reload emfor` also runs `emfor check-config` first and does not
   signal the daemon when the config is invalid. A change to `[store]` or to
   the `listen` of `[dns]`, `[metrics]` or `[peers]` is the exception: the
   daemon rejects it at a reload and keeps the old config, but the config is
   valid and the reload command only sends the signal, so the tool sees no
   failure: `systemctl reload emfor` exits 0 and Ansible reports `changed`.
   Restart for those keys, and read the log (`reload rejected`). Ansible:
   `state: reloaded` of `ansible.builtin.service`. Salt: `reload: True` of
   `service.running`, with `watch`. Puppet: the `restart` attribute of
   `service` takes the command to run instead of a stop and a start. Chef:
   `supports reload: true` on `service` and `notifies :reload` from the file.
2. **Run `emfor check-config` as the daemon's user.** Since 0.7.0 it starts
   the plugins the config declares, so run by root it runs them as root. The
   owner check of the files also depends on who runs it
   ([above](#who-may-own-and-write-the-config-files)).
3. **The tools' own validation does not fit one file.** Ansible's `validate`
   (`ansible.builtin.template`), Puppet's `validate_cmd` (`file`), Salt's
   `check_cmd` (`file.managed`) and Chef's `verify` (`file`) give your command one temporary file, before
   the file replaces the old one. `emfor check-config` reads a main file and
   its `include_dir`, and cannot check a single fragment. Two ways that work:
   - Check a whole staged copy of the config. `include_dir` is relative to
     the main file's directory, so a copy of the main file with a `conf.d`
     next to it, holding the old files and the new one, is a complete config:

     ```sh
     # emfor --config /path/to/stage/emfor.toml check-config
     ```

     The staged files must meet the same owner and mode rules.
     [examples/deploy/emfor-check-staged.sh](../../examples/deploy/emfor-check-staged.sh)
     builds such a copy and runs the check on it as the daemon's user, and
     can be the command that the tool's validation option calls (the header
     of the script has a line for each of the three tools). If you write your
     own command, set a `umask` in it (the script uses `umask 022`): the
     tool's umask can leave the staged `conf.d` writable by group or others
     (Ansible's left it `0777`), and `check-config` refuses that. A valid
     fragment gives `config ok`, and one that sets `timeout` above `interval`
     gives exit status 2 and names the file.
   - Write the file, check, and put the old file back when the check fails,
     as `emfor-drain.sh` does
     ([chapter 12](12-operating.md#a-rolling-deploy-with-drain)).
4. **Owners and modes.** The main file, the `conf.d` directory and its files
   must be owned by root or the daemon's user and not writable by group or
   others (`0644` files, `0755` directory). A peer secret must also not be
   readable by others, and `root:emfor` with `0640` is the usual way
   ([chapter 11](11-peer-groups.md#the-secret)). Set owner and mode on the
   resource, so that the file is never in place with a wrong one.
5. **Where the package comes from.** The deb and rpm of emfor are not in any
   repository ([chapter 4](04-installing.md#packages-deb-and-rpm)). Build
   them, or put them in a repository of your own, and let the tool install
   from there.

The tool settings above were run on Debian 12 (arm64, a container with
systemd) with the 0.7.0 deb, using Ansible core 2.14.18, Puppet 7.23.0,
Salt 3008.5 and Cinc Client 19.4.46 (the free build of Chef Infra Client; its
`verify` takes the temporary file as `%{path}`). Each tool installed a valid fragment and reloaded emfor, refused
a fragment with an error through that script (the old file stayed, and there
was no reload), and changed nothing when it applied the valid fragment again.
Not tested: other versions and systems, the Chef Infra Client from Progress
itself, and a tool that runs from a server (a Puppet server, a Salt master).
