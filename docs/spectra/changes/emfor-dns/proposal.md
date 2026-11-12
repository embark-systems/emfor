## Why

Programs written in languages without an emfor client can only find a healthy backend through ordinary DNS lookups, so emfor needs a DNS server that answers from its live health state. At the same time, services need to choose how a backend is picked (round-robin, sticky, failover), and that choice must be shared by the DNS server now and the language clients later. FreeBSD is an official deployment platform, so the daemon must be verified there too.

## What Changes

- New per-service setting `balance` with values `round-robin` (default), `sticky`, and `failover`, settable in `[defaults]` or per service, and written into each LMDB service record as a new `"balance"` field (store format version stays 1, since no reader of the format exists yet).
- New `[dns]` config table. When present, the daemon serves DNS over UDP and TCP on the configured `listen` addresses (default `127.0.0.1:5300`) for names under `zone` (default `svc.emfor`), answering A and AAAA queries with one healthy backend chosen by the service's `balance`, with a TTL from `[dns] ttl` (default 1s) or the per-service `dns_ttl`.
- Defined DNS responses for every case: outside the zone (REFUSED), unknown name (NXDOMAIN with SOA), no backend of the requested address family (NODATA), all backends down with `on_all_down = "closed"` (SERVFAIL).
- Reload applies `zone`, `ttl`, `dns_ttl`, and `balance`; a change to `[dns] listen` (including adding or removing `[dns]`) requires a restart, like `[store]`.
- Deployment guide docs/deploy/dns-resolver.md explaining how to forward the zone from dnsmasq, unbound (including FreeBSD `local_unbound`), systemd-resolved, and macOS `/etc/resolver`, with verification commands and caching caveats. The dnsmasq and unbound recipes are tested automatically in containers.
- FreeBSD becomes an official platform: a GitHub Actions workflow runs the full test suite on Linux and in a FreeBSD VM, and an example rc.d script is provided.

## Non-Goals (optional)

Scope exclusions are recorded in design.md under Goals / Non-Goals.

## Capabilities

### New Capabilities

- `backend-selection`: The `balance` methods and how one backend is chosen from a service's candidates (healthy backends, or all backends when all are down and `on_all_down = "open"`); this change defines selection without an affinity key, as used by DNS.
- `dns-server`: The `[dns]` table, listening, the zone, record types, TTLs, response codes for every case, and reload behavior of the DNS server.

### Modified Capabilities

- `service-config`: Adds `balance` and `dns_ttl` to the per-service and `[defaults]` settings, adds validation rules for `balance` and the `[dns]` table, and makes a `[dns] listen` change on reload require a restart.
- `state-store`: Adds the `balance` field to the service record of store format version 1.

## Impact

- Code: crates/emfor-format/src/lib.rs (new `Balance` type and record field), crates/emfor-daemon/src/config.rs (new keys and `[dns]` table), crates/emfor-daemon/src/writer.rs and crates/emfor-daemon/src/supervisor.rs (publish live state to DNS), crates/emfor-daemon/src/cli.rs (start DNS, reload rules), new modules crates/emfor-daemon/src/select.rs and crates/emfor-daemon/src/dns.rs.
- New dependency: hickory-server and hickory-proto 0.26 (DNS); hickory-resolver as a dev-dependency for tests.
- New files: docs/deploy/dns-resolver.md, examples/rc.d/emfor, .github/workflows/ci.yml, tests/resolver/ (container recipes for dnsmasq and unbound).
- examples/emfor.toml gains `balance` and a `[dns]` table.
- Existing LMDB readers: none exist yet; the new `balance` field is additive.
