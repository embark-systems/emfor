# FreeBSD port of emfor (example)

A port skeleton for `net-mgmt/emfor`: it builds `emfor` and `emfor-check`
from the GitHub release, installs the rc.d script and a sample config from
the source's `examples/`, and creates the `emfor` user and the store
directory.

Built and installed on FreeBSD 15.1-RELEASE (arm64, in a VM) with the
2026Q4 ports branch and the `rust` package; see [Checked](#checked). Not
yet through poudriere, and not on 14.x or amd64.

## Files

| File | Contents |
| --- | --- |
| `Makefile` | The port. `DISTVERSION` follows the emfor release. |
| `pkg-descr` | Package description. |
| `Makefile.crates` | Every crate in the release's `Cargo.lock`, as `CARGO_CRATES`. `USES=cargo` includes it from the port's directory. |
| `files/emfor.in` | The rc.d script for `USE_RC_SUBR`: `examples/rc.d/emfor` with `%%PREFIX%%` for `/usr/local` and `emfor_user` defaulting to `emfor`. |

## Making the port

In a ports tree (or an overlay), as root:

```sh
mkdir -p /usr/ports/net-mgmt/emfor
cp -R Makefile Makefile.crates pkg-descr files /usr/ports/net-mgmt/emfor/
cd /usr/ports/net-mgmt/emfor
make makesum                           # fetches the release and every crate
```

`USERS` and `GROUPS` need an `emfor` entry in `/usr/ports/UIDs` and
`/usr/ports/GIDs`; in a private tree, pick ids that are free there. Then:

```sh
make stage check-plist
make install
```

or `poudriere testport -o net-mgmt/emfor`.

## For a new release

`DISTVERSION` and `Makefile.crates` change with each release
([RELEASING.md](../../RELEASING.md)); `clients/rust/tests/versions.rs`
fails until both match the workspace. `Makefile.crates` comes from the
repository's `Cargo.lock`:

```sh
awk '/^name = /{n=$3} /^version = /{v=$3}
     /^source = "registry\+/{gsub(/"/,"",n); gsub(/"/,"",v); print n"-"v}' Cargo.lock |
  awk 'NR==1{printf "CARGO_CRATES=\t%s", $0; next}
       {printf " \\\n\t\t%s", $0} END{print ""}' > examples/freebsd/Makefile.crates
```

When `examples/rc.d/emfor` changes, regenerate `files/emfor.in`
(`clients/rust/tests/versions.rs` fails until it matches):

```sh
sed -e 's|/usr/local|%%PREFIX%%|g' -e 's|emfor_user:=root|emfor_user:=emfor|' \
    -e 's|(default root)|(default emfor)|' \
    examples/rc.d/emfor > examples/freebsd/files/emfor.in
```

On FreeBSD, `make cargo-crates` lists the crates too. It extracts the
source into `work/` first, so run `make clean` after it: a `work/`
extracted before `Makefile.crates` listed the crates has none of them,
and the build stops at `no matching package named ...`.

## What it installs

| Path | |
| --- | --- |
| `${PREFIX}/bin/emfor` | The daemon and `emfor status`. Built with `EMFOR_PREFIX=${PREFIX}`, so its default config is `${PREFIX}/etc/emfor/emfor.toml`. |
| `${PREFIX}/bin/emfor-check` | The redis, MySQL, PostgreSQL and LDAP checks; point a plugin's `command` at it. |
| `${PREFIX}/etc/rc.d/emfor` | `files/emfor.in`, installed by `USE_RC_SUBR`. `daemon(8)` restarts `emfor` 2 seconds after it exits. |
| `${PREFIX}/etc/emfor/emfor.toml.sample` | `examples/emfor.toml`; copied to `emfor.toml` on first install. |
| `/var/lib/emfor` | The store's directory, owned by `emfor`. |

The store stays at `/var/lib/emfor/state.mdb` rather than under
`/var/db`: that is the default of the daemon and of every client
(`EMFOR_STORE`), so applications on the host find it without a setting.

After installing, edit `${PREFIX}/etc/emfor/emfor.toml`, then:

```sh
emfor check-config
sysrc emfor_enable=YES
service emfor start
emfor status
```

`libemfor` and the Go, Python, Perl and PHP clients are not part of this
port.

## Checked

On FreeBSD 15.1-RELEASE (arm64), with the ports tree on the same branch as
the packages (`quarterly`, 2026Q4: its `lang/rust` matched the installed
`rust` 1.98.1 package; a `main` ports tree wants a newer rust and builds
it from source):

- `make makesum`, then `make stage check-plist`: no plist issues. The
  build takes about 2.5 minutes on 4 cores.
- **TLS library.** rustls uses `aws-lc-sys`, which builds AWS-LC with
  cmake. `USES=cargo` adds `cmake:devel/cmake-core` to `BUILD_DEPENDS` by
  itself; `libclang` is not needed.
- `make install`, then `emfor check-config`, `service emfor start` and
  `emfor status` with the sample config: the daemon runs as `emfor`, the
  store is `/var/lib/emfor/state.mdb`, and its log reaches syslog.
- `pkg delete emfor` leaves `/var/lib/emfor` while the store is in it.
- **The rc.d script**: `tests/resolver/freebsd.sh` passes (start, reload,
  an invalid config refused, restart after a kill, stop).

`make test` runs the whole workspace's tests, some of which start daemons
on loopback ports; they are not needed to build the package.
