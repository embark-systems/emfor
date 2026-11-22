# Packages

Packaging of emfor for the systems it is deployed on.

| Directory | What |
| --- | --- |
| [`freebsd/`](freebsd/) | An example FreeBSD port (`net-mgmt/emfor`), see its [README](freebsd/README.md). |
| `deb/`, `rpm/`, `common/` | The deb and rpm packages: the maintainer scripts of each, and the files they share (the systemd unit, the packaged config, the `conf.d` README). The package metadata (name, assets, dependencies) is in `[package.metadata.deb]` and `[package.metadata.generate-rpm]` of [`crates/emfor-daemon/Cargo.toml`](../crates/emfor-daemon/Cargo.toml). |
| `Dockerfile`, `build.sh` | The builder image and the script that builds both packages into `dist/`. |
| `dist/` | The built `.deb` and `.rpm`. Build output: do not commit it. |

## Building the deb and rpm

```sh
packages/build.sh                        # for this machine's architecture
PLATFORM=linux/amd64 packages/build.sh   # for another one (slow if emulated)
```

It builds the image `emfor-packages` (Debian 11 with Rust, `cargo-deb` and
`cargo-generate-rpm`), builds `emfor` and `emfor-check` in release mode with
`--locked`, and writes `dist/emfor_<version>-1_<arch>.deb` and
`dist/emfor-<version>-1.<arch>.rpm`. The build container has glibc 2.31, so
the binaries run on Debian 11 and later, Ubuntu 20.04 and later, and RHEL 9
and its relatives (glibc 2.34). The version is the one in `Cargo.toml`.
The packages that `build.sh` writes are not signed.

## Signing

`tools/sign-packages.sh sign` signs the packages of `packages/dist/` with the
key in `SIGN_KEY` (a fingerprint), and `tools/sign-packages.sh verify` checks
the result in clean containers that hold only the public key. An rpm gets
its signature inside the file (`rpmsign --addsign`; it needs a Linux host or
container). A deb gets a detached `NAME.deb.asc`: apt verifies a repository,
not a single file. Both go into `SHA256SUMS`, which is signed as
`SHA256SUMS.asc`, and the public key is written as `emfor-release.asc` and
`emfor-release.gpg`. A user checks with `rpm --import emfor-release.asc` and
`rpm -K emfor-*.rpm`, and with `gpg --import emfor-release.asc`,
`gpg --verify emfor_*.deb.asc emfor_*.deb` and `sha256sum -c SHA256SUMS`.

**Status**: the script was run only with a throwaway key, in Rocky Linux 9
(rpm 4.16.1.3), for RSA 4096 and for ed25519: both verify, a changed byte
fails, and an rpm that is not signed, or whose key is not imported, does not
report `signatures OK`. There is no release key yet, no release has been
signed, and the key's fingerprint is published nowhere; until then the
packages stay unsigned. Not tried: a hardware key, a passphrase-protected key,
other distributions, an apt or dnf repository.

## What a package installs

| Path | |
| --- | --- |
| `/usr/bin/emfor`, `/usr/bin/emfor-check` | The daemon and the plugin binary with the protocol checks. Built without `EMFOR_PREFIX`, so the config path is `/etc/emfor/emfor.toml`. |
| `/usr/lib/systemd/system/emfor.service` | [`common/emfor.service`](common/emfor.service): the hardened unit of [`examples/systemd/`](../examples/systemd/emfor.service) with the binary under `/usr/bin`. |
| `/etc/emfor/emfor.toml` | A config that only sets `include_dir = "conf.d"`: no service until you add one. Kept on upgrade (a conffile). |
| `/etc/emfor/conf.d/` | For the services, one file per group (`01-`, `10-`, `99-` prefixes); a `README` explains it. |
| `/usr/share/doc/emfor/` | The commented example config and the manual (`manual/`); the deb also has the Prometheus rules. |

Both packages depend on `ca-certificates`: an http check with
`scheme = "https"` and certificate verification on builds its client with the
system's root certificates, and without them it fails with `cannot build http
client` (plain `scheme = "http"` and `tls_verify = false` need none); the TLS
checks of `emfor-check` use them too when `ca_file` is not set.

The store is in `/var/lib/emfor`, which the unit creates (`StateDirectory`).
The scripts create the user and group `emfor`, and make `/etc/emfor` and
`/etc/emfor/conf.d` owned by root and mode 0755: emfor refuses a config
directory that group or others can write, or that a user other than root or
the user emfor runs as owns.

- **deb**: the service is enabled and started when the package is installed
  (the Debian convention); `dpkg -r` keeps the config, `dpkg -P` removes the
  config and `/var/lib/emfor`; the user stays.
- **rpm**: the service is not enabled: run `systemctl enable --now emfor`
  when the config is ready. An upgrade restarts a running daemon.

## What has been tested

On an arm64 machine, in containers (Debian 12 and Rocky Linux 9): the deb
and the rpm install; the user, the directories and their modes are right;
`emfor check-config` passes as the `emfor` user; the daemon starts as that
user and creates the store; remove, purge and upgrade over a modified config
behave as above. The deb was also run under systemd 252 as PID 1 in a Debian
12 container: the postinst enables and starts the unit, the daemon runs as
`emfor` under the unit's hardening with tcp, http, exec and `emfor-check dns`
checks, `systemctl reload` refuses a conf.d file that others can write and
the daemon keeps running, `restart` with an invalid config leaves the unit
retrying until its start limit (`systemctl reset-failed` after the fix),
and a killed daemon is started again. **Not tested**: the rpm under systemd,
x86-64 packages, a real host (journald, other cgroup limits), an upgrade
between two different versions, and other distributions than the two.
