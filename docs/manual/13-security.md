# 13. Security model

[Contents](README.md) · Previous: [12. Operating emfor](12-operating.md) · Next: [14. Integrations](14-integrations.md)

This chapter says what emfor trusts, what it protects, and what you must
protect yourself. It follows [SECURITY.md](../../SECURITY.md) and the 0.7.0
code.

## What is trusted, and what is not

| Thing | Trusted? | Why it matters |
| --- | --- | --- |
| The config files and `include_dir` | Yes | They name programs the daemon runs (`exec` commands, plugins). |
| Plugin and `exec` programs | Yes | They run with the daemon's rights. |
| Backends | **No** | Anything a backend or a plugin sends is bounded and escaped before it reaches the store, logs, status or metrics. |
| Peers | Only with the secret | Every message is authenticated (and by default encrypted) with the shared secret, and checked for replay, sender and site. |
| Local users | Partly | Readers of the store share its lock file ([below](#the-store-and-local-users)). |
| The network to DNS and `/metrics` | No authentication; `/metrics` can limit sources with `[metrics] allow` | Both listen on loopback by default. `allow` checks the address of the direct peer only (no `X-Forwarded-For`), so it does not replace a firewall. |

## The config files

The daemon and `emfor check-config` refuse to load the main config file,
each file in `include_dir` and that directory when:

- group or others can write them:
  `/etc/emfor/emfor.toml: writable by group or others (mode 0664); refusing to load it; to fix it: chmod g-w,o-w /etc/emfor/emfor.toml`;
- a user other than root or the daemon's own user owns them:
  `... owned by uid 1001, not root or the user running emfor (uid 998); refusing to load it; to fix it: chown root ...`.

Before 0.7.0 only the `include_dir` files were checked this way. Keep the
files `root:root 0644` (or `0640` with group `emfor`, if they name
things you prefer to keep from other users). Passwords never go into the
config: they go into `password_file`s.

To know when the config changed, and to what, read the audit lines of the
log ([chapter 12](12-operating.md#what-a-reload-changed)): each file read has a
SHA-256 and each reload lists its differences. They do not say who made the
change; for that, watch the config directory with auditd.

## Secrets

| File | Rules | Warnings |
| --- | --- | --- |
| `[peers] secret_file`, `previous_secret_file` | Absolute; owned by root or the daemon's user; not readable by others; not writable by group or others; 32 bytes to 64 KiB. Errors otherwise. | |
| `password_file`, `secret_file` (radius), `tls_key_file` (diameter) of `emfor-check` | Absolute; non-empty. | A warning when every user can read it, or others can change it. |
| A password sent without TLS (redis, ldap, mysql, pgsql, amqp) | | A warning ([chapter 7](07-plugins.md#passwords)). |

Recommended:

```sh
# install -d -m 0750 -o root -g emfor /etc/emfor/secrets
# install -m 0640 -o root -g emfor /dev/null /etc/emfor/secrets/pg.pass
```

The record `<store>-peers` holds SHA-256 fingerprints of the peer secrets,
never the secrets themselves.

## What check programs get

`exec` checks and plugins:

- get only `PATH`, `LANG`, `TZ`, `LC_*` and the names in `env_pass` from the
  daemon's environment (and, for `exec`, `EMFOR_IP`, `EMFOR_PORT`,
  `EMFOR_SERVICE`). A secret in the daemon's environment does not reach
  them unless you list it. In 0.6 and earlier they got everything;
- run in their own process group, which is killed when the check ends or
  the plugin stops;
- run as the daemon's user, under the same systemd sandbox.

Run `emfor check-config` as the daemon's user, not as root, where you can:
it starts every plugin to validate options, and as root they would run
outside the sandbox. The example scripts in `examples/deploy/` do this.

## Untrusted input from backends

Everything a backend says can end up in a reason. emfor:

- cuts reasons at 512 characters (ending in `…`) and escapes control
  characters, so a backend cannot add log lines or send terminal escape
  sequences to `emfor status`;
- reads at most 16384 bytes for `expect` (tcp) and 65536 for `expect_body`
  (http), limits plugin lines to 65536 bytes, and logs at most 1000
  characters of a plugin's standard error line;
- does not follow HTTP redirects and ignores proxy variables.

Reasons received from peers are cut to 256 characters and escaped before
they are logged.

## The store and local users

LMDB readers register in the lock file, so every application user must be
able to **write** `<store>-lock`. Whoever can write it can also stop the
daemon's commits or disturb other readers. The defaults (0640 and 0660, owner
and group of the daemon) limit that to the daemon's group. With
`lock_file_mode = "0666"` it is every local user, and the daemon warns:

```text
[store] lock_file_mode = "0666" lets every user on this host write the lock file, ...
Use "0660" (the default) and put the users of your applications in the group of the emfor daemon.
```

Members of the `emfor` group can therefore affect lookups on the host. Add
only application users to it.

The store holds service names, backend addresses, states and reasons, but no
secrets.

## DNS and metrics

Neither has authentication. Both listen on `127.0.0.1` by default; the daemon
warns when DNS listens elsewhere, and when `/metrics` does and `[metrics]
allow` is not set. `allow` lists the networks that may read `/metrics`; the
others get `403`. If you expose them, firewall them to the
hosts that need them. The DNS server has a per-network rate limit for UDP
([chapter 10](10-dns.md#rate-limit)) and answers never exceed the query's
offered size.

## Peer messages

- Authenticated with HMAC-SHA256 (`EMP1`) or encrypted with
  ChaCha20-Poly1305 (`EMP2`) or XChaCha20-Poly1305 (`EMP3`), with keys
  derived by HKDF-SHA256 from the secret.
- Since 0.7.0 a daemon with `encrypt = true` drops plaintext messages by
  default (`accept_plaintext = false`).
- Replays, old messages, unknown senders and other sites are dropped and
  counted.
- A peer cannot change a backend's state by itself: a `down` notice only
  triggers local rechecks, and votes need two of three assigned checkers.
  Someone who holds the secret, though, can make every member mark a backend
  down by sending votes. Protect the secret like a password.

## The systemd unit

The packaged unit runs the daemon as `emfor` with no capabilities, a
read-only file system except the store directory, `NoNewPrivileges`, a
system call filter, `UMask=0027`, `ProtectProc=invisible` and more
([chapter 4](04-installing.md#the-systemd-unit)). `systemd-analyze security
emfor` shows the result; it was not run for this manual.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting on the repository (Security,
then Report a vulnerability), not a public issue. Until 1.0 only the latest
release gets security fixes.
