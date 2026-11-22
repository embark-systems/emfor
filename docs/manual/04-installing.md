# 4. Installing

[Contents](README.md) · Previous: [3. Concepts](03-concepts.md) · Next: [5. Configuring emfor](05-configuring.md)

## What goes on which host

| Host | Install |
| --- | --- |
| Every application host | The daemon `emfor`, a config, and a service definition (systemd unit or rc.d script). |
| Hosts that use a check of `emfor-check` (redis, MySQL, PostgreSQL, LDAP, DNS, RADIUS, SIP, Diameter, SMTP, AMQP, MongoDB, gRPC, QUIC) | Also `emfor-check`. The packages always include it. |
| Hosts whose applications use a client library | `libemfor` and the wrapper for the application's language. |
| Hosts whose programs only use DNS | Nothing more. Forward the zone from the host resolver ([chapter 10](10-dns.md#forwarding-from-the-host-resolver)). |

Install the daemon and every client library **from the same emfor release**,
and upgrade them together. The client library contains the daemon's own
LMDB build; a mismatch makes lookups fail with `UNAVAILABLE` and a message
that says so.

## Packages: deb and rpm

The repository builds a deb and an rpm. They are **not published** anywhere
and **not signed**; you build them yourself, in Docker:

```sh
$ packages/build.sh                         # for this machine's architecture
$ PLATFORM=linux/amd64 packages/build.sh    # for another one (slow if emulated)
```

The packages land in `packages/dist/` as `emfor_<version>-1_<arch>.deb` and
`emfor-<version>-1.<arch>.rpm`.

The binaries are built against glibc 2.31, so they run on Debian 11 and later,
Ubuntu 20.04 and later, and RHEL 9 and its relatives.

What a package installs:

| Path | What |
| --- | --- |
| `/usr/bin/emfor`, `/usr/bin/emfor-check` | The daemon and the plugin program. The default config path is `/etc/emfor/emfor.toml`. |
| `/usr/lib/systemd/system/emfor.service` | The hardened systemd unit ([below](#the-systemd-unit)). |
| `/etc/emfor/emfor.toml` | A config that only sets `include_dir = "conf.d"`. Kept on upgrade. |
| `/etc/emfor/conf.d/` | Your service files go here; a `README` explains it. |
| `/usr/share/doc/emfor/` | The commented example config, the Prometheus rules in `examples/prometheus/` and the manual in `manual/`. |

The scripts create the user and group `emfor` and make `/etc/emfor` and
`/etc/emfor/conf.d` owned by root with mode 0755. The store directory
`/var/lib/emfor` is created by the unit (`StateDirectory=emfor`).

```sh
# dpkg -i emfor_0.7.1-1_amd64.deb      # Debian, Ubuntu: enabled and started at once
# rpm -i emfor-0.7.1-1.x86_64.rpm      # RHEL and relatives: not enabled
# vi /etc/emfor/conf.d/10-web.toml     # your services
# emfor check-config
# systemctl enable --now emfor         # rpm; for the deb: systemctl reload emfor
# emfor status
```

- **deb**: `dpkg -r` keeps the config; `dpkg -P` removes the config and
  `/var/lib/emfor`. The user stays.
- **rpm**: an upgrade restarts a running daemon. Since 0.7.0 the rpm marks
  `/etc/emfor/emfor.toml` as `%config(noreplace)`: an upgrade keeps your
  changed file and writes the packaged one next to it as `emfor.toml.rpmnew`.

Both packages depend on `ca-certificates`, which `https` checks with
verification need.

What has been tested: the deb and the rpm in Debian 12 and Rocky Linux 9
containers on arm64 (install, remove, purge, upgrade over a changed config),
and the deb under systemd 252 as PID 1 in a Debian 12 container. **Not
tested**: the rpm under systemd as a long-running service on a real host,
x86-64 packages, other distributions. See
[packages/README.md](../../packages/README.md).

## Building from source

You need Rust 1.85 or later (the 2024 edition). From a checkout or an
unpacked source release:

```sh
$ cargo build --release --bin emfor --bin emfor-check
# install -m 0755 target/release/emfor target/release/emfor-check /usr/bin/
```

`emfor-check` is optional; install it only if you use its checks. The
database drivers are linked into `emfor-check` only, not into the daemon or
the client library.

The daemon does not look for `emfor-check` anywhere: a plugin's `command`
names its path ([chapter 7](07-plugins.md)). Where it is depends on how you
installed it:

| Install | `emfor-check` |
| --- | --- |
| deb and rpm packages | `/usr/bin/emfor-check` |
| FreeBSD port | `/usr/local/bin/emfor-check` |
| From source, with the `install` line above | `/usr/bin/emfor-check` |
| From source, with `cargo install --root /usr/local` ([below](#the-default-config-path)) | `/usr/local/bin/emfor-check` |

### The default config path

`emfor` reads `/etc/emfor/emfor.toml` unless `--config` names another file.
The default is fixed at build time by the environment variable
`EMFOR_PREFIX`:

| `EMFOR_PREFIX` | Default config path |
| --- | --- |
| not set, empty, `/` or `/usr` | `/etc/emfor/emfor.toml` |
| `/usr/local` | `/usr/local/etc/emfor/emfor.toml` |
| `/opt/emfor` | `/opt/emfor/etc/emfor/emfor.toml` |

```sh
$ EMFOR_PREFIX=/usr/local cargo build --release --bin emfor
```

A relative prefix fails the build. `emfor --help` shows the path that was
built in. Only the config path follows the prefix: the store stays at
`[store] path` (default `/var/lib/emfor/state.mdb`), which is also where the
clients look.

With `cargo install`, name each crate (the repository root is a workspace):

```sh
# EMFOR_PREFIX=/usr/local cargo install --locked --path crates/emfor-daemon --root /usr/local
# cargo install --locked --path crates/emfor-check --root /usr/local
```

## The systemd unit

The packages install the unit; for a source install, copy
[examples/systemd/emfor.service](../../examples/systemd/emfor.service), which
runs `/usr/local/bin/emfor` (edit the three paths if you installed to
`/usr/bin`):

```sh
# useradd --system --no-create-home --shell /usr/sbin/nologin emfor
# install -m 0644 examples/systemd/emfor.service /etc/systemd/system/emfor.service
# install -d -m 0755 /etc/emfor /etc/emfor/conf.d
# install -m 0644 my-emfor.toml /etc/emfor/emfor.toml
# emfor check-config
# systemctl daemon-reload
# systemctl enable --now emfor
```

What the unit does:

| Setting | Effect |
| --- | --- |
| `User=emfor`, `Group=emfor` | The daemon runs unprivileged. |
| `ExecStartPre=... check-config` | An invalid config never starts. |
| `ExecReload=... check-config` then `kill -HUP` | `systemctl reload emfor` signals the daemon only when the config is valid. |
| `Restart=on-failure`, `RestartPreventExitStatus=1 2` | A crash (including exit 70) is restarted; exit 1 (cannot start) and 2 (invalid config) are not, because a retry does not fix them. |
| `StartLimitIntervalSec=60`, `StartLimitBurst=5` | After five failed starts in a minute the unit is `failed` until `systemctl reset-failed emfor`. New in 0.7.0. |
| `StateDirectory=emfor`, `StateDirectoryMode=0755` | Creates `/var/lib/emfor` for the store, owned by `emfor`. |
| `UMask=0027`, `ProtectProc=invisible`, `ProcSubset=pid` | New in 0.7.0. The store files set their own modes regardless. |
| `ProtectSystem=strict`, `NoNewPrivileges=yes`, `SystemCallFilter=@system-service` and more | Hardening. Plugins and `exec` checks run under the same limits. |
| `LimitNOFILE=65536` | Checks, plugins and DNS connections all use file descriptors. |
| `CapabilityBoundingSet=` | No capabilities. For DNS on port 53, replace it with `CapabilityBoundingSet=CAP_NET_BIND_SERVICE` and `AmbientCapabilities=CAP_NET_BIND_SERVICE`. |

Things to know:

- The file system is read-only for the daemon except `/var/lib/emfor`. A
  `[store] path` elsewhere needs `ReadWritePaths=` in a drop-in
  (`systemctl edit emfor`).
- Do not use `DynamicUser=`: its state directory is not reachable by the
  applications.
- `MemoryDenyWriteExecute=yes` is in the unit as a comment. You can turn it on
  if none of your plugins generates code at run time; it was tried with the
  daemon and the `dns` plugin on systemd 252.
- The logs go to the journal: `journalctl -u emfor`.
- A copy of the example unit made for an older release does not change by
  itself: compare it with the new one when you upgrade
  ([chapter 15](15-upgrading.md)).

## FreeBSD

[packages/freebsd/](../../packages/freebsd/) is an example port,
`net-mgmt/emfor`. It builds both programs with `EMFOR_PREFIX=/usr/local`,
installs an rc.d script and a sample config, creates the `emfor` user, an
empty `/usr/local/etc/emfor/conf.d`, and `/var/db/emfor` for the store.

```sh
# sysrc emfor_enable=YES
# service emfor start
# emfor status
```

| rc.conf variable | Default | Meaning |
| --- | --- | --- |
| `emfor_enable` | `NO` | Start at boot. |
| `emfor_config` | `/usr/local/etc/emfor/emfor.toml` | The config file. |
| `emfor_user` | `emfor` | The user the daemon runs as. It needs write access to the store's directory. |

- `service emfor configtest` checks the config, `service emfor reload` checks
  it and then sends SIGHUP, and `start` refuses an invalid config.
- The daemon runs under `daemon(8)`, which starts it again 2 seconds after it
  exits. Its log goes to syslog with the tag `emfor`.
- The plugin program is `/usr/local/bin/emfor-check`, not `/usr/bin`.
- The sample config puts the store at `/var/db/emfor/state.mdb`, not the
  default. Give every application on the host
  `EMFOR_STORE=/var/db/emfor/state.mdb`.

The port has been built and installed on FreeBSD 15.1-RELEASE (arm64, in a
virtual machine) with `make stage check-plist install`. It has not been run
through poudriere, on 13.x or 14.x, or on amd64 beyond the project's CI.

## Client libraries

Every client reads the store through `libemfor`, built from
[clients/rust](../../clients/rust/README.md) with
[cargo-c](https://github.com/lu-zero/cargo-c). It contains emfor's own LMDB
build; a system liblmdb cannot read the store.

```sh
$ cargo install cargo-c
$ cd clients/rust
# cargo cinstall --release --features capi --prefix /usr/local
```

This installs `libemfor.so` (with the soname `libemfor.so.0.7` for 0.7.x),
`libemfor.a`, the header `emfor.h` and `emfor.pc` for `pkg-config`.

| Language | Install |
| --- | --- |
| Rust | Depend on the `emfor-client` crate from the same release (`clients/rust`). No `libemfor` needed. |
| C | `libemfor` as above; build with `cc app.c $(pkg-config --cflags --libs emfor)`. |
| Go | `libemfor`, then `go get github.com/embark-systems/emfor/clients/go@v0.7.1`. Needs cgo and `pkg-config emfor`. |
| Python | `libemfor`, then `pip install clients/python` (Python 3.9 or later; it uses `ctypes`). Set `EMFOR_LIBRARY` to the library's path if it is outside the standard directories. |
| Perl | `libemfor`, then in `clients/perl`: `perl Makefile.PL && make -f Makefile.xs && make -f Makefile.xs install`. |
| PHP | `libemfor`, then build the extension with `phpize` from a copy of `clients/php` (see [chapter 9](09-clients.md#php)) and enable `extension=emfor`. PHP 8.1 or later. |

Prefer the shared library: it exports only `emfor_*` symbols, so its LMDB
cannot clash with another LMDB in the same process.

[Chapter 9](09-clients.md) shows how to use each one.
