## Why

Through DNS an application gets only an IP address, so services whose backends listen on different ports, or applications that want to fail over themselves, cannot use the DNS path. SRV records carry the port, a priority and a weight for every backend, and many libraries (Go `net.LookupSRV`, gRPC, Consul-style clients) already understand them. With `weighted` and `locality` now in place (emfor-balance-2), every balance method has a natural SRV form.

## What Changes

- The DNS server answers **SRV queries** on a service name, and on `_<label>._tcp.<service name>` for any first label starting with `_`, with the same records.
- An SRV answer lists **every candidate backend** of the service (both address families), chosen by the health and `on_all_down` rules, and maps the service's `balance` onto SRV fields:
  - `failover`: priority = position in config order (0, 1, 2, …), weight 1.
  - `weighted`: priority 0, weight = backend `weight`.
  - `locality`: priority 0 for backends in the host's `site`, 1 for the others (all 0 when the host has no site or none matches), weight 1, order rotated per query.
  - `round-robin`, `sticky` and unknown values: priority 0, weight 1, order rotated per query.
- Each SRV target is a **name derived from the backend's IP** under the service name, for example `10-0-0-11.api.svc.emfor` or `2001-db8--11.api.svc.emfor`. The server answers A or AAAA for these names with that backend's address, and puts the matching A/AAAA records in the additional section of SRV answers.
- **Size limit:** over UDP the reply must fit the client's advertised size (512 bytes without EDNS, at most 1232). Additional records that do not fit are left out without setting TC; only when the SRV answers themselves do not fit is TC set, so the resolver retries over TCP.
- Name classification changes: `_tcp.<service>` is an empty non-terminal (NODATA), target names exist, and other types on service names stay NODATA.
- Resolver guide gains an SRV section with `dig SRV` checks; vocabulary and project context record the target-name form.

## Non-Goals (optional)

Recorded in design.md.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `dns-server`: answers for service names (SRV added), names that are not services (SRV owner names, `_tcp` empty non-terminals and target names), and new requirements for SRV answers, target names and the UDP size limit. This capability is added by the in-progress change `emfor-dns`, which must be archived first; `emfor-balance-2` must be archived before this change too, because the SRV mapping uses `weighted`, `locality` and `site`.

## Impact

- Affected specs: dns-server.
- Affected code:
  - crates/emfor-daemon/src/dns.rs: SRV owner names, target names, additional records, size limit; the handler passes the protocol and the client's size to `answer`.
  - crates/emfor-daemon/src/select.rs: candidates over all families and the SRV ordering per `balance`.
  - crates/emfor-daemon/src/live.rs: one rotation counter per service for SRV order.
  - docs/deploy/dns-resolver.md: SRV section.
  - docs/spectra/LANGUAGE.md and docs/spectra/config.yaml.
- No change to the store format, the config file, the clients, or libemfor.
- Ordering: archive `emfor-dns`, then `emfor-balance-2`, then this change.
