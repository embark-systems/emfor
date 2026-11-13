# FreeBSD port of emfor (example)

A port skeleton for `net-mgmt/emfor`: it builds `emfor` and `emfor-check`
from the GitHub release, installs the rc.d script and a sample config from
the source's `examples/`, and creates the `emfor` user and the store
directory.

**Not yet built on FreeBSD.** The cargo commands it runs were checked on
other systems; the port itself has not been through `make` or poudriere.
See [Unverified](#unverified) for what to watch.

## Files

| File | Contents |
| --- | --- |
| `Makefile` | The port. `DISTVERSION` follows the emfor release. |
| `pkg-descr` | Package description. |
| `Makefile.crates` | Every crate in the release's `Cargo.lock`, as `CARGO_CRATES`. |

## Making the port

In a ports tree (or an overlay), as root:

```sh
mkdir -p /usr/ports/net-mgmt/emfor
cp Makefile Makefile.crates pkg-descr /usr/ports/net-mgmt/emfor/
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

On FreeBSD, `make cargo-crates` lists the crates too. It extracts the
source into `work/` first, so run `make clean` after it: a `work/`
extracted before `Makefile.crates` listed the crates has none of them,
and the build stops at `no matching package named ...`.

## What it installs

| Path | |
| --- | --- |
| `${PREFIX}/bin/emfor` | The daemon and `emfor status`. Built with `EMFOR_PREFIX=${PREFIX}`, so its default config is `${PREFIX}/etc/emfor/emfor.toml`. |
| `${PREFIX}/bin/emfor-check` | The redis, MySQL, PostgreSQL and LDAP checks; point a plugin's `command` at it. |
| `${PREFIX}/etc/rc.d/emfor` | `examples/rc.d/emfor` with `${PREFIX}` paths and `emfor_user` defaulting to `emfor`. |
| `${PREFIX}/etc/emfor/emfor.toml.sample` | `examples/emfor.toml`; copied to `emfor.toml` on first install. |
| `/var/lib/emfor` | The store's directory, owned by `emfor`. |

The store stays at `/var/lib/emfor/state.mdb` rather than under
`/var/db`: that is the default of the daemon and of every client
(`EMFOR_STORE`), so applications on the host find it without a setting.

After installing, edit `${PREFIX}/etc/emfor/emfor.toml`, then:

```sh
emfor --check-config
sysrc emfor_enable=YES
service emfor start
emfor status
```

`libemfor` and the Go, Python, Perl and PHP clients are not part of this
port.

## Unverified

- **TLS library.** rustls uses `aws-lc-sys`, which compiles AWS-LC from C.
  If the build stops asking for `cmake` or for `libclang` (bindgen), add
  `cmake:devel/cmake-core` to `BUILD_DEPENDS`, or `USES+= llvm` and
  `CARGO_ENV+= LIBCLANG_PATH=${LLVM_PREFIX}/lib`.
- **The rc.d script** has not run on FreeBSD either;
  `tests/resolver/freebsd.sh` checks it on a FreeBSD host.
- `make test` runs the whole workspace's tests, some of which start
  daemons on loopback ports; they are not needed to build the package.
