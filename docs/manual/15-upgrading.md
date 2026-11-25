# 15. Upgrading

[Contents](README.md) · Previous: [14. Integrations](14-integrations.md) · Next: [16. Troubleshooting](16-troubleshooting.md)

## The rules

- All components share one version and are released together: the daemon,
  `emfor-check`, `libemfor`, the Rust crates and the Go, Python, Perl and
  PHP clients. **Install them from the same release and upgrade them
  together.** `libemfor` contains the daemon's LMDB build; a mismatch gives
  `UNAVAILABLE`, never wrong data.
- Before 1.0, a minor release (0.6 → 0.7) may change a stable interface; the
  CHANGELOG lists each such change under **Read before you upgrade** or as
  *breaking* / *compatibility change*, with what to do. Patch releases never
  need a change on your side.
- The store is rebuilt at every start, so there is never a store to migrate;
  only the history is carried over.
- The config rejects unknown keys. Before a downgrade, remove the keys the
  older release does not know.
- Stable: the config, commands and exit statuses, `status --json`, the store
  format, the client API, the plugin protocol, DNS answers, metric names.
  Not stable: `emfor status` text, log lines, error message wording. Match on
  codes and exit statuses in scripts. The exceptions are the audit lines of a
  reload ([chapter 12](12-operating.md#what-a-reload-changed)), whose field
  names and `change` values are stable, and the fields of the state change line
  and the `timestamp` and `level` of the JSON log
  ([chapter 12](12-operating.md#json-lines)); the wording around them is not. See
  [docs/compatibility.md](../compatibility.md).

## The order on each host

1. **Clients first.** Install the new `libemfor` and wrappers, and restart
   the applications so they load them (a running process keeps the library it
   started with).
2. **Check the config with the new binary**: `emfor check-config`. Run it as
   the daemon's user if you can. With an invalid config the systemd unit's
   `ExecStartPre` refuses to start, and the daemon stays down.
3. **Install the daemon and restart it.** Restart, rather than stop, install
   and start, to keep the gap short
   ([chapter 12](12-operating.md#what-applications-see-during-a-restart)).
4. **Check**: `emfor status` exits 0, and the log has no warnings you did not
   expect.

Do one host at a time.

## Which releases get fixes

Only the latest release. Before 1.0, a bug fix or a security fix goes into the
next release and is not backported to an older one, so the way to get a fix
is to upgrade (the rules above say what that costs). emfor comes without a
support contract and without a promised response time; how to report a
vulnerability is in [chapter 13](13-security.md#reporting-a-vulnerability).
What 1.0 will promise is not decided yet.

## From 0.7 to 0.8

The store format (version 2) and the plugin protocol (1) did not change. The
shared library did get a new name, so install the 0.8 clients together with
the daemon, as the rules above say.

Work through this list **before** you restart the daemon:

| # | Change in 0.8.0 | What breaks | What to do |
| --- | --- | --- | --- |
| 1 | The shared library is `libemfor.so.0.8` (it was `libemfor.so.0.7`). | A program linked against `libemfor.so.0.7` does not load 0.8. The Python, Perl, PHP and Go clients of 0.8 look for the new name. | Install the new library next to the old one, rebuild or reinstall the programs one by one (PHP and Perl extensions included), and remove the old file after the last one. |
| 2 | A `password_file`, a RADIUS `secret_file`, a `tls_key_file`, a `tls_cert_file` or a `ca_file` must be a regular file of bounded size (64 KiB; `tls_cert_file` and `tls_key_file` 1 MiB; `ca_file` 8 MiB). | A FIFO, a device, a directory or a larger file is refused. A link to a regular file still works. | Check that each such path is a regular file below its limit. systemd `LoadCredential`, Docker secrets and Kubernetes secrets end up as regular files (not tried on each). |
| 3 | A heartbeat far in the future counts as stale. | When the host clock was set back by more than `EMFOR_MAX_STALENESS` (3 s by default) and the daemon then stops writing, a lookup sees every backend as unknown: a `closed` service gives `ALL_DOWN` (exit status 1 of `emfor lookup`), an `open` service gives every backend. `emfor status` exits with 1 where it exited with 0. | Nothing to change if the clock is right. A client of 0.7 does not have this rule. |
| 4 | `heartbeat_age_ms` can be negative, in `emfor status --json` and in `emfor lookup --explain --json` (`store.heartbeat_age_ms`). In the Rust crate `emfor-client`, `Explanation.heartbeat_age_ms` is an `i64` (it was a `u64`). | A parser or a Rust program that expects a number of at least 0 fails on a heartbeat that is later than the reader's clock. | Accept a negative number. |
| 5 | `[peers] cipher = "aes-256-gcm"` sends `EMP4`. | A member of an earlier version counts `EMP4` as `malformed`. | Upgrade every member of the group before you set `cipher`. |

Other changes in 0.8.0 that you may notice:

- **Downgrading.** A config that uses a key added in 0.8 (`[log] format`,
  `[peers] cipher`, `[metrics] allow`, the `ca_file` of an `http` check) fails
  `emfor check-config` on 0.7. Take the key out first.
- **The log is longer.** A reload writes up to 51 lines about what it changed
  (50 and one for the rest), and every start and reload logs the SHA-256 of
  each config file it reads.
- **New lines and series.** `emfor status` and `/metrics` have new lines and
  series: clock offsets and silent checkers of peer members, and
  `emfor_clock_jumps_total`. The daemon also logs a jump of the host clock
  ([chapter 12](12-operating.md)).
- **`[metrics] allow`.** Without it nothing changes, except a warning at start
  and reload when `listen` has a non-loopback address.
- **An empty store.** `emfor status` says `store is empty` for a store the
  daemon never wrote to, instead of a version problem.

A quick check before the restart, on each host:

```sh
# emfor check-config                         # with the 0.8 binary
# stat -L -c '%F %s %n' /path/to/each-credential-file   # "regular file", and a size below the limit?
# ldd /path/to/my-app | grep emfor           # still linked against libemfor.so.0.7?
```

## From 0.6 to 0.7

The store format (version 2) and the plugin protocol (1) did not change:
0.6 clients and plugins keep working with a 0.7 daemon, but install the 0.7
clients anyway.

Work through this list **before** you restart the daemon:

| # | Change in 0.7.0 | What breaks | What to do |
| --- | --- | --- | --- |
| 1 | `exec` checks and plugins get only `PATH`, `LANG`, `TZ`, `LC_*` (and `EMFOR_*` for exec). | A check program that reads `HOME`, `http_proxy`, `LD_LIBRARY_PATH` or another variable runs without it. `check-config` does not warn. | Add the names to `env_pass` in `check_options` (exec) or in `[plugins."<name>"]`. `emfor-check` TLS checks without `ca_file` may need `SSL_CERT_FILE` or `SSL_CERT_DIR`. |
| 2 | Store files default to 0640 and 0660 (were 0644 and 0666); the daemon sets the modes at every start. | Applications whose user is not in the daemon's group get `UNAVAILABLE: ... Permission denied`. | `usermod -aG emfor <user>` and restart the application, or set `file_mode = "0644"` and `lock_file_mode = "0666"` in `[store]`. |
| 3 | The main config file is checked like the `conf.d` files. | A main file writable by group or others, or owned by another user, stops the daemon from starting. | `chmod g-w,o-w /etc/emfor/emfor.toml`, `chown root /etc/emfor/emfor.toml`. The packages install it 0644 root. |
| 4 | `accept_plaintext` defaults to `false` when `encrypt` is `true`. | A 0.7 member drops plaintext messages of 0.6 members that run with `encrypt = false`. | Turn encryption on in the whole group, or set `accept_plaintext = true` on 0.7 members until all are upgraded. |
| 5 | A peer secret file must be owned by root or the daemon's user. | Config error at `peers.secret_file`. | `chown root /etc/emfor/peers.key`. |
| 6 | `previous_secret_file` is used only for the secret the daemon's record says it replaced. | At the first 0.7 start there is no record, so it is ignored. | Do not do the first 0.7 start in the middle of a secret change. |
| 7 | The shared library is `libemfor.so.0.7` (the soname carries the minor version). | A program linked against `libemfor.so.0.6` does not load 0.7. | Rebuild or reinstall what links against it, PHP and Perl extensions included. |
| 8 | Durations are parsed by emfor's own parser. | A rare duration form that only the old parser accepted fails `check-config`. | Use forms like `"500ms"`, `"2s"`, `"1h 30m"`. |

Other changes in 0.7.0 that you may notice:

- **systemd unit.** The packaged unit now has `StartLimitIntervalSec=60`,
  `StartLimitBurst=5`, `UMask=0027`, `ProtectProc=invisible` and
  `ProcSubset=pid`. A copy of `examples/systemd/emfor.service` you installed
  by hand does not change: compare it with the new one. After five failed
  starts in a minute the unit stays `failed` until
  `systemctl reset-failed emfor`.
- **rpm.** `/etc/emfor/emfor.toml` is now `%config(noreplace)`: an upgrade
  keeps your changed file and writes the packaged one as
  `emfor.toml.rpmnew`. (0.6 marked it `%config`, which replaced a changed
  file and saved yours as `.rpmsave`; the packaged file is the same in both.)
- **http checks** with `scheme = "http"` now work on a host without CA
  certificates. The reason of a failed http check no longer contains the URL.
- **Warnings** (not errors) in `check-config` and at start: secret and
  password files that every user can read, passwords sent without TLS, store
  modes that let other users write.
- **redis** hides the AUTH password in reasons; plugin log text is escaped;
  invisible format characters in reasons are escaped.
- **The writer lock file** `<store>.writer` is 0600.

A quick check before the restart, on each host:

```sh
# emfor check-config                         # with the 0.7 binary
# id www-data                                # the application users in group emfor?
# stat -c '%U %a %n' /etc/emfor/emfor.toml /etc/emfor/peers.key
# grep -rn 'check = "exec"' /etc/emfor/      # any check program that needs env_pass?
```

## Older releases

If you skip releases, read every CHANGELOG entry in between. The ones that
need an action:

| Release | Action |
| --- | --- |
| 0.6.0 | Accepts every 0.5 config. Upgrade the clients before you use `drain = true`, so that an older client does not choose a drained backend. |
| 0.5.0 | The config is validated more strictly (an `interval` above `24h`, a `timeout` below `10ms` and more are refused). Run `check-config` first. |
| 0.4.0 | The store format is version 2; clients older than 0.4 fail every lookup with `INCOMPATIBLE`. Peer messages are encrypted by default; upgrade a group host by host with `encrypt = false`, then turn it on. |

0.1 to 0.3 are early releases; see [CHANGELOG.md](../../CHANGELOG.md).
