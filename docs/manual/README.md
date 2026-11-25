# The emfor manual

![emfor logo](../logo/emfor-wordmark.svg)

This manual is for people who have never used emfor: the administrator who
installs and runs it, and the developer whose application asks it which
backend to connect to. It describes **emfor 0.8.0**.

Start with chapter 1 to see whether emfor fits your problem, then do the
quick start in chapter 2. It takes about ten minutes on one Linux host.
After that, read the chapters you need. Chapter 18 has every config key in
one table.

## Chapters

| # | Chapter | Read it when you want to |
| --- | --- | --- |
| 1 | [What emfor is, and what it is not](01-what-emfor-is.md) | decide whether emfor fits |
| 2 | [Quick start](02-quick-start.md) | see it work on one host in ten minutes |
| 3 | [Concepts](03-concepts.md) | understand services, backends, health states, tiers and the store |
| 4 | [Installing](04-installing.md) | install the daemon, the packages, the service unit and the client libraries |
| 5 | [Configuring emfor](05-configuring.md) | lay out the config files, check them and reload them |
| 6 | [Health checks: tcp, http and exec](06-health-checks.md) | choose and tune the built-in checks |
| 7 | [Plugins and emfor-check](07-plugins.md) | check databases, LDAP, DNS, RADIUS, SIP, Diameter, SMTP, AMQP, MongoDB, gRPC or QUIC, or write your own check |
| 8 | [How a backend is chosen](08-choosing-a-backend.md) | pick a `balance` method and use backups, drain and slow start |
| 9 | [Client libraries](09-clients.md) | look up a backend from Rust, C, Go, Python, Perl or PHP |
| 10 | [DNS](10-dns.md) | serve programs that cannot use a client library |
| 11 | [Peer groups](11-peer-groups.md) | let the hosts of a data centre share check results |
| 12 | [Operating emfor](12-operating.md) | use `emfor status` and `emfor lookup`, reload, read metrics and logs, look after the store |
| 13 | [Security model](13-security.md) | know who can do what, and what to lock down |
| 14 | [Integrations](14-integrations.md) | feed nginx, HAProxy, Caddy, Prometheus or pf, or monitor emfor from Nagios |
| 15 | [Upgrading](15-upgrading.md) | upgrade from 0.7 (or older) to 0.8.0 |
| 16 | [Troubleshooting](16-troubleshooting.md) | fix something, starting from the error message |
| 17 | [Migrating from HAProxy or Consul](17-migrating.md) | translate an existing setup |
| 18 | [Configuration reference](18-config-reference.md) | look up a key, its default and its limits |
| 19 | [Glossary](19-glossary.md) | look up a term |

## Conventions

- `$` starts a command you type as a normal user, `#` one you type as root.
  Lines without either are output.
- The paths are the defaults: the config is `/etc/emfor/emfor.toml`, the store
  is `/var/lib/emfor/state.mdb`, and the plugin program is
  `/usr/bin/emfor-check` (where the deb and rpm packages put it).
- Service names end in `.svc.emfor`, the default DNS zone. Any host-name-like
  name works, but only names under the DNS zone can be resolved over DNS.
- The examples use private addresses such as `10.0.0.11`. The quick start uses
  `127.0.0.1` and `127.0.0.2` so that it runs on one machine.

## What "tested" means in this manual

Every complete TOML example in this manual was checked with
`emfor check-config` of 0.7.0 in a Debian 13 container. The command output
shown was produced by emfor 0.7.0 in such a container, except where the text
says otherwise; what 0.8.0 added says what it was run with, and the output of
`emfor --version` is the one of 0.8.0. Some statements come from emfor's own test suite (for example
the nginx and HAProxy recipes) and say so. Where something has not been run
(macOS, FreeBSD on amd64, real hardware, real database servers), the text says
so where it matters.

The more detailed reference documents in the repository are linked where
they apply: [docs/deploy/](../deploy/) for daemon, DNS and plugin details,
[docs/plugins/protocol-v1.md](../plugins/protocol-v1.md) for the plugin
protocol, and each client's README under [clients/](../../clients/).
