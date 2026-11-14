# emfor user manual

English | [繁體中文](../manual.zh_TW/README.md)

This manual is for the people who install emfor, configure it, and write
the applications that use it. It explains how the pieces fit together and
walks through each task; the documents under [docs/deploy/](../deploy/) and
the client READMEs hold the full detail and are linked where they apply.

The manual describes emfor 0.4.

| Chapter | What it covers |
| --- | --- |
| [1. Concepts](01-concepts.md) | The daemon, the state store, clients, DNS, health states, and what happens when something fails |
| [2. Installation](02-installation.md) | Building, install prefixes, systemd, FreeBSD, and the client libraries |
| [3. Configuration](03-configuration.md) | Every table and key of `emfor.toml`, validation, and what a reload can change |
| [4. Health checks](04-health-checks.md) | `tcp`, `http`, `exec`, plugins and `emfor-check`; rise, fall, scheduling and reasons |
| [5. Choosing a backend](05-backend-selection.md) | The `balance` methods, `on_all_down`, and a stale store |
| [6. Client libraries](06-clients.md) | Looking up a backend from Rust, C, Go, Python, Perl and PHP |
| [7. DNS](07-dns.md) | The DNS server, its answers, SRV records, and forwarding from the host resolver |
| [8. Operations](08-operations.md) | Commands, signals and reload, `emfor status`, metrics, logs, exit codes, the store on disk, upgrading |
| [9. Troubleshooting](09-troubleshooting.md) | Symptoms, their causes, and what to do |

New to emfor? Read chapter 1, then follow the quick start in the
[README](../../README.md#quick-start), then chapter 3.

## Conventions

- Service names in the examples end in `.svc.emfor`, the default DNS zone.
  Any host-name-like name works; only names under the zone resolve over DNS.
- Paths are the defaults: the config at `/etc/emfor/emfor.toml`, the store
  at `/var/lib/emfor/state.mdb`.
- `$` starts a command you type; lines without it are output.
