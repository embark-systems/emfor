# 2. Installation

[Manual](README.md) · Previous: [1. Concepts](01-concepts.md) · Next: [3. Configuration](03-configuration.md)

emfor is built from source with Cargo. Linux and FreeBSD are the
deployment platforms; macOS works for development.

## What you install where

| Host | Install |
| --- | --- |
| Every application host | The daemon `emfor`, its config, and a service definition (systemd or rc.d). |
| Hosts that check redis, MySQL, PostgreSQL or LDAP | Also `emfor-check`. |
| Hosts whose applications use a client library | `libemfor`, and the wrapper for the application's language. |
| Hosts whose programs use DNS only | Nothing more; forward the zone from the host resolver ([chapter 7](07-dns.md)). |

## Building the daemon

You need a Rust toolchain that supports the 2024 edition (Rust 1.85 or
later). From a checkout or an unpacked source release:

```sh
cargo build --release --bin emfor --bin emfor-check
install -m 0755 target/release/emfor /usr/local/bin/emfor
install -m 0755 target/release/emfor-check /usr/local/libexec/emfor-check
```

`emfor-check` is optional; install it only if you use its plugins
([chapter 4](04-health-checks.md#plugins-and-emfor-check)). The database
drivers are linked into `emfor-check` only, not into the daemon or the
client library.

### The default config path

`emfor` reads `/etc/emfor/emfor.toml` unless `--config` names another
file. The default is fixed when the binary is built, from the environment
variable `EMFOR_PREFIX`:

| `EMFOR_PREFIX` | Default config path |
| --- | --- |
| not set, empty, or `/usr` | `/etc/emfor/emfor.toml` |
| `/usr/local` | `/usr/local/etc/emfor/emfor.toml` |
| `/opt/emfor` | `/opt/emfor/etc/emfor/emfor.toml` |

```sh
EMFOR_PREFIX=/usr/local cargo build --release --bin emfor
```

The prefix must be an absolute path, or the build fails. `emfor --help`
shows the path that was built in. Only the config path follows the prefix:
the store stays at the config's `[store] path` (default
`/var/lib/emfor/state.mdb`), which is also where clients look by default.

### With cargo install

The repository root is a workspace, not a package, so name each crate:

```sh
EMFOR_PREFIX=/usr/local cargo install --locked --path crates/emfor-daemon --root /usr/local
cargo install --locked --path crates/emfor-check --root /usr/local
```

`--root` only decides where the binaries go; set `EMFOR_PREFIX` as well.

## Running under systemd

The repository has a hardened unit,
[examples/systemd/emfor.service](../../examples/systemd/emfor.service). It
runs `/usr/local/bin/emfor server --config /etc/emfor/emfor.toml` as user
`emfor`:

```sh
useradd --system --no-create-home --shell /usr/sbin/nologin emfor
install -m 0644 examples/systemd/emfor.service /etc/systemd/system/emfor.service
install -d /etc/emfor
install -m 0644 examples/emfor.toml /etc/emfor/emfor.toml   # then edit it
emfor check-config
systemctl daemon-reload
systemctl enable --now emfor
emfor status
```

What the unit does for you:

- `StateDirectory=emfor` creates `/var/lib/emfor` for the store, owned by
  `emfor`. Do not switch to `DynamicUser=`: its directory is not reachable
  by applications.
- `systemctl reload emfor` runs `emfor check-config` first and sends SIGHUP
  only when the config is valid.
- The daemon is restarted after a crash, including exit status 70, but not
  after status 1 or 2, which a retry does not fix
  ([exit codes](08-operations.md#exit-codes)).
- Logs go to the journal: `journalctl -u emfor`.
- The file system is read-only except `/var/lib/emfor`. A `[store] path`
  elsewhere needs `ReadWritePaths=`; DNS on port 53 needs
  `CAP_NET_BIND_SERVICE` (see [docs/deploy/daemon.md](../deploy/daemon.md#running-under-systemd)).

Build without `EMFOR_PREFIX` (or with `/usr`) so that `emfor status` and
`emfor check-config` find `/etc/emfor/emfor.toml` without `--config`.

## Running on FreeBSD

[examples/freebsd/](../../examples/freebsd/) is an example port
(`net-mgmt/emfor`) that builds `emfor` and `emfor-check`, installs an rc.d
script and a sample config, and creates the `emfor` user and
`/var/db/emfor`, where its sample config puts the store (applications set
`EMFOR_STORE=/var/db/emfor/state.mdb`). The rc.d script runs the daemon
under `daemon(8)`, which starts it again 2 seconds after it exits. After
installing the port and editing `/usr/local/etc/emfor/emfor.toml`:

```sh
sysrc emfor_enable=YES
service emfor start
emfor status
```

| rc.conf variable | Default | Meaning |
| --- | --- | --- |
| `emfor_enable` | `NO` | Start at boot. |
| `emfor_config` | `/usr/local/etc/emfor/emfor.toml` | Config file. |
| `emfor_user` | `emfor` | User the daemon runs as. It needs write access to the store's directory. |

`service emfor configtest` checks the config (as `emfor_user`);
`service emfor reload` checks it and then sends SIGHUP; `start` refuses an
invalid config. The daemon's log goes to syslog with the tag `emfor`.

Without the port, build emfor with `EMFOR_PREFIX=/usr/local`, install the
port's rc.d script with its prefix filled in, and create the `emfor` user
(or set `emfor_user`), which needs write access to the directory of
`[store] path`:

```sh
sed 's|%%PREFIX%%|/usr/local|g' examples/freebsd/files/emfor.in > /usr/local/etc/rc.d/emfor
chmod 0555 /usr/local/etc/rc.d/emfor
pw useradd emfor -d /nonexistent -s /usr/sbin/nologin -c "emfor daemon"
```

Read the comments in the script and the port's README before using them.
Both have been run on FreeBSD 15.1-RELEASE (arm64, in a VM), the port with
`make stage check-plist install`; not with poudriere, on 14.x or on amd64.

## Installing the client libraries

Every client reads the store through `libemfor`, built from
[clients/rust](../../clients/rust/README.md). It contains the daemon's own
LMDB build: a system liblmdb cannot read the store.

**Install the client library from the same emfor release as the daemon,
and upgrade both together.** A mismatch makes lookups fail with
`UNAVAILABLE` and a message that says so.

| Language | How |
| --- | --- |
| Rust | Depend on the `emfor-client` crate from the same release (`clients/rust`). |
| C | `cargo install cargo-c`, then in `clients/rust`: `cargo cinstall --release --features capi --prefix /usr/local`. This installs `libemfor`, `emfor.h` and `emfor.pc`. |
| Go | Install `libemfor`, then `go get github.com/embark-systems/emfor/clients/go@v0.2.0` (needs cgo and `pkg-config emfor`). |
| Python | Install `libemfor`, then `pip install clients/python`. Set `EMFOR_LIBRARY` if the library is outside the standard directories. |
| Perl | Install `libemfor`, then in `clients/perl`: `perl Makefile.PL && make -f Makefile.xs && make -f Makefile.xs install`. |
| PHP | Install `libemfor`, then build the extension with `phpize` (see [clients/php](../../clients/php/README.md)) and enable `extension=emfor`. |

Prefer the shared library: it exports only `emfor_*` symbols, so its LMDB
cannot clash with another LMDB in the same process. [Chapter 6](06-clients.md)
shows how to use each client.
