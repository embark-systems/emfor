## Context

`emfor-core` (archived) built the daemon: config, checkers, the health state machine, and the LMDB writer. Health state lives in the writer task (crates/emfor-daemon/src/writer.rs, `Writer::records`), which is fed check results by per-backend tasks and layout changes by the `Supervisor`.

This change adds the second read path. Applications in languages without an emfor client resolve `<service>.<zone>` through the host's normal resolver; the resolver forwards only the emfor zone to the daemon's DNS server on a loopback port. emfor never becomes the host's general DNS server, so a stopped daemon only breaks its own zone.

It also introduces `balance`, the per-service choice of how one backend is picked. DNS is its first user; `emfor-clients` will reuse the same definitions and add the affinity-key path for `sticky`.

FreeBSD is now an official platform next to Linux, and the repository has a GitHub remote (`origin`, github.com/vanillahsu/emfor), so CI can run on both.

Terms follow docs/spectra/LANGUAGE.md: zone (負責網域), resolver (本機 resolver), balance (分配方式).

## Goals / Non-Goals

**Goals:**

- A DNS server inside the daemon that answers A and AAAA for services under the zone, from live in-memory health state, honoring `balance` and `on_all_down`.
- A precise answer for every query shape, so resolvers cache correctly and applications see the right `getaddrinfo` error.
- `balance` in config and in the LMDB record, so clients can follow the same choice later.
- A tested guide for forwarding the zone from common resolvers on Linux, FreeBSD, and macOS.
- CI that runs the full test suite on Linux and FreeBSD.

**Non-Goals:**

- Forwarding or recursing for names outside the zone; emfor is not a general resolver.
- SRV records (planned later; needs synthesized per-backend target names).
- `sticky` with an affinity key, and per-process round-robin in clients (change `emfor-clients`).
- `random`, `fastest`, `weighted`, `zone` balance methods (planned later).
- Docker and Kubernetes DNS deployment.
- DNSSEC signing of the zone, zone transfers (AXFR/IXFR), dynamic updates, EDNS Client Subnet.
- Packaging (.deb, FreeBSD port); systemd unit files.

## Decisions

### Balance type in the format crate

`emfor_format::Balance` is an enum serialized as `"round-robin"`, `"sticky"`, `"failover"`. `ServiceRecord` gains `balance: Balance`, written after `on_all_down`. The store stays at format version 1: no reader of the format has shipped, so adding a field now costs nothing, whereas bumping the version would force every future client to handle a version that never existed in the wild.

Alternative considered: bump to version 2. Rejected for the reason above.

### Backend selection as a pure function

A new module crates/emfor-daemon/src/select.rs holds `select(record, family, counter) -> Selection` with no I/O:

1. Take the backends of the requested address family (IPv4 for A, IPv6 for AAAA), in config order. None → `Selection::NoFamily`.
2. Candidates are the `healthy` ones among them. If there are none: with `on_all_down = "open"` the candidates are all backends of that family; with `"closed"` the result is `Selection::AllDown`.
3. Pick by `balance`: `round-robin` takes `candidates[counter % len]` and advances the counter; `failover` takes `candidates[0]`; `sticky` (no key is available in this change) behaves as `round-robin`.

The all-down decision is per address family: an A query for a service whose IPv4 backends are all down but whose IPv6 backends are healthy is treated as all-down for A. This keeps each record type self-consistent.

Round-robin counters are kept per service and per family in the DNS module (`AtomicU64`), so rotation is global across all DNS clients of the daemon. A reload keeps counters; their value only shifts the starting point.

Alternative considered: put selection inside the DNS handler. Rejected because `emfor-clients` must implement exactly the same rules in six languages, and a pure function with a table test is the reference for them.

### Live state shared with the DNS server

The writer already holds the current `ServiceRecord` for every service. It gains a `LiveState` handle (`Arc<RwLock<LiveView>>`) and updates it on every `Update` and `SetLayout` before committing to LMDB, so DNS answers do not depend on LMDB commits succeeding. `LiveView` maps the lowercased service name to its record plus `dns_ttl`. `ServiceLayout` gains `balance` and `dns_ttl` so the supervisor passes them through on start and reload.

Alternative considered: DNS reads LMDB. Rejected: same process, extra copy and parse per query, and a failed commit would make DNS stale.

### DNS server on hickory-server

Use hickory-server 0.26 with a custom request handler (not its authority/catalog machinery, which is built around static zone files). The handler is a thin adapter around a pure function `answer(query, &LiveView, &DnsSettings, counters) -> Reply`, which is where all behavior lives and is unit-tested without sockets.

Each `listen` address gets one UDP socket and one TCP listener (TCP idle timeout 5s). Binding happens at startup after the store is opened; a bind failure exits with status 1 and an error naming the address.

The default port is 5300, not 5353: 5353 is the mDNS port, held by mDNSResponder on macOS and by avahi on many Linux hosts, so a 5353 default would fail to bind there.

`DnsSettings` (`zone`, `ttl`) is published through a `tokio::sync::watch` channel so reload can change it without touching sockets.

### Response rules

| Query | Reply |
| --- | --- |
| Opcode not QUERY | NOTIMP |
| Question count not 1 | FORMERR |
| Name not equal to or under the zone | REFUSED |
| A or AAAA for a service name, candidate found | NOERROR, AA, one A or AAAA record, TTL = service `dns_ttl` or `[dns] ttl` |
| A or AAAA for a service name, no backend of that family | NOERROR, AA, no answer, SOA in authority (NODATA) |
| A or AAAA for a service name, all backends of that family down, `on_all_down = "closed"` | SERVFAIL |
| Any other type for a service name | NODATA |
| SOA at the zone apex | NOERROR, AA, the SOA record |
| Any other type at the zone apex | NODATA |
| Name under the zone that is a proper ancestor of a service name (an "empty non-terminal", e.g. `svc.emfor` is not one, but `b.svc.emfor` is when `a.b.svc.emfor` is a service) | NODATA |
| Any other name under the zone | NXDOMAIN, AA, SOA in authority |

Name matching is case-insensitive and ignores a trailing dot; the reply echoes the question as received.

The synthesized SOA is `<zone>. <ttl> IN SOA ns.<zone>. hostmaster.<zone>. <serial> 3600 600 86400 <ttl>`, with serial = daemon start time in Unix seconds (truncated to u32). The SOA minimum and the SOA record's own TTL both equal `[dns] ttl`, so negative answers are cached no longer than positive ones.

SERVFAIL is used for the closed all-down case rather than NXDOMAIN, because the name exists; NXDOMAIN would be cached as "does not exist" and keep failing after backends recover.

### DNS config and reload rules

```toml
[dns]
listen = ["127.0.0.1:5300"]   # default; non-empty list of ip:port
zone = "svc.emfor"            # default; same label rules as service names
ttl = "1s"                    # default; whole seconds, 0s to 3600s

[defaults]
balance = "round-robin"       # default; "round-robin" | "sticky" | "failover"

[services."api.svc.emfor"]
balance = "failover"          # optional override
dns_ttl = "5s"                # optional override of [dns] ttl; whole seconds, 0s to 3600s
```

The DNS server runs only when `[dns]` is present. `dns_ttl` is valid without `[dns]` and then has no effect. On start and reload the daemon logs one `warn` line per service whose name is not under the zone ("service X is outside DNS zone Y and cannot be resolved via DNS").

Reload applies `zone`, `ttl`, `dns_ttl`, and `balance`. A change to `[dns] listen`, including adding or removing the `[dns]` table, is a reload error "dns listen settings require restart", reported together with all other errors like the existing `[store]` rule. Rebinding sockets would open a window where queries fail; an explicit restart makes that window visible to the operator.

### Resolver deployment guide with container tests

docs/deploy/dns-resolver.md has one section per resolver: dnsmasq (`server=/<zone>/127.0.0.1#5300`), unbound including FreeBSD `local_unbound` (`forward-zone` to `127.0.0.1@5300`, `do-not-query-localhost: no`, and `domain-insecure: "<zone>"` because the zone is unsigned under a signed or nonexistent parent and validating resolvers would otherwise fail every answer), systemd-resolved, and macOS `/etc/resolver/<zone>` (`nameserver 127.0.0.1`, `port 5300`). Each section lists the file, the lines, the reload command, and two checks: a direct query to emfor (`dig @127.0.0.1 -p 5300 api.svc.emfor`, or `drill` on FreeBSD) and a query through the system resolver (`getent hosts api.svc.emfor`). A closing section covers resolver caching: minimum cache TTL settings (for example dnsmasq `min-cache-ttl`) defeating the 1s TTL, and negative caching.

tests/resolver/ holds a Dockerfile and a script `tests/resolver/run.sh <dnsmasq|unbound>` that builds emfor in a Rust image, runs it with a test config against a local TCP backend inside a Debian container, configures the resolver exactly as the guide says, points the container's resolv.conf at that resolver, and asserts that `getent hosts api.svc.emfor` returns the backend address. The guide's snippets and the test configs are kept identical by having the test script read the snippet files from docs/deploy/resolver/ that the guide includes verbatim.

The systemd-resolved recipe is verified on the GitHub Actions Ubuntu runner, which runs systemd-resolved. The FreeBSD `local_unbound` recipe is verified in the FreeBSD CI job. The macOS recipe is verified manually on a developer Mac, and the guide records the macOS version used.

### CI on GitHub Actions with a FreeBSD VM

.github/workflows/ci.yml runs on push and pull request:

- `linux` (ubuntu-latest): `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, then `tests/resolver/run.sh dnsmasq`, `tests/resolver/run.sh unbound`, and the systemd-resolved recipe check.
- `freebsd`: `vmactions/freebsd-vm` on a supported FreeBSD 14 release, installs Rust from packages, runs `cargo test --workspace`, then applies the `local_unbound` recipe and checks resolution with `drill` and `getent`.

Alternative considered: Cirrus CI for native FreeBSD runners. Rejected to keep one CI system with the repository's GitHub remote.

### FreeBSD rc.d example

examples/rc.d/emfor is an rc.subr script using `daemon(8)` to run `emfor --config ${emfor_config}` in the background, with `emfor_enable`, `emfor_config` (default `/usr/local/etc/emfor/emfor.toml`), `emfor_user`, and `extra_commands="reload"` mapped to `SIGHUP`. It is an example only; no port is built.

## Implementation Contract

**Behavior**

- With `[dns]` present, `dig @127.0.0.1 -p 5300 api.svc.emfor A` returns exactly one A record for a healthy IPv4 backend with the configured TTL and the AA flag. Without `[dns]`, nothing listens.
- Every row of the response rules table above is observable with `dig` against a running daemon.
- `balance = "round-robin"`: consecutive A queries for a service with healthy backends 10.0.0.1 and 10.0.0.2 alternate between them. `balance = "failover"`: every query returns the first healthy backend in config order. `balance = "sticky"`: same as round-robin.
- A health change is reflected in the next DNS answer without waiting for an LMDB commit.
- `svc/<name>` records in LMDB carry `"balance"`.

**Interfaces and data shapes**

- Config keys as in "DNS config and reload rules".
- `emfor_format::Balance` and `ServiceRecord.balance`; JSON `"balance": "round-robin" | "sticky" | "failover"`, placed after `on_all_down`.
- `select::select` and `dns::answer` are pure functions with the inputs named above.

**Failure modes**

- Bind failure at startup: exit 1, error names the address (for example "cannot listen on 127.0.0.1:5300: address in use").
- Reload changing `listen`: rejected with "dns listen settings require restart", previous config keeps running, DNS keeps serving.
- Validation errors for `[dns]` and `balance` follow the existing rule: all reported, each with its TOML path.

**Acceptance criteria**

- `cargo test --workspace` passes, including table tests for selection and for every response-rule row, and end-to-end tests that start the `emfor` binary and query it with hickory-resolver.
- `tests/resolver/run.sh dnsmasq` and `tests/resolver/run.sh unbound` pass locally with Docker.
- The GitHub Actions workflow passes on both the `linux` and `freebsd` jobs for the commit that completes this change.
- The macOS section of docs/deploy/dns-resolver.md names the macOS version it was verified on.

**Scope boundaries**

In scope: everything above. Out of scope: everything under Non-Goals, and any change to checkers, the health state machine, or LMDB layout other than the `balance` field.

## Risks / Trade-offs

- [hickory-server's API changes between minor versions] → Keep all behavior in the pure `answer` function; the hickory adapter stays small and is the only code touched on upgrade.
- [Resolvers with a minimum cache TTL keep returning a backend after it fails] → Documented in the guide's caching section with the exact setting per resolver; it is a deployment choice, not something emfor can override.
- [The systemd-resolved recipe depends on the systemd version] → Verified on the CI runner's version, which the guide records; other versions are noted as untested.
- [FreeBSD VM jobs are slow (estimated several minutes per run; not measured)] → Acceptable for correctness; measure on the first runs and cache the cargo registry if it exceeds 15 minutes.
- [Per-family all-down can surprise operators who expect service-wide behavior] → Stated explicitly in the spec and the guide; most services are single-family.
- [`round-robin` over DNS is shaped by resolver caching: with TTL 1s, one cached answer serves a whole host for a second] → Inherent to DNS; the guide explains it. Clients (later change) do not have this limit.

## Migration Plan

Additive. Existing configs without `[dns]` or `balance` behave as before, with `balance` defaulting to `round-robin` in the LMDB record. Rollback is reverting to the previous binary; an LMDB record with the extra field is still valid for it to overwrite on start.

## Open Questions

None blocking. The exact systemd-resolved recipe is settled by the CI check in this change.
