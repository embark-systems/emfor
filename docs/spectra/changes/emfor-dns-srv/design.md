## Context

The DNS server (emfor-dns) answers A and AAAA for service names with exactly one address picked by `balance`; every other type on a service name is NODATA. The pure function `answer(op_code, questions, view, settings) -> Reply` in crates/emfor-daemon/src/dns.rs holds all behavior; the hickory-server `Handler` only decodes the request, builds the response with `MessageResponseBuilder`, and always advertises an EDNS size of 1232 when the request had EDNS.

Selection lives in crates/emfor-daemon/src/select.rs: `candidate_backends(record, family)` filters by family, keeps healthy backends, and applies `on_all_down` per family; `select(record, family, &FamilyChoice, site)` picks one. Per-service choice state (`RoundRobin`, with one `FamilyChoice` per family) lives in crates/emfor-daemon/src/live.rs and survives reloads for services that stay.

emfor-balance-2 (committed, not archived) added `weight` and `site` to backend records and `site` to `LiveView`, so every balance method now has an SRV form. Service names and the zone are validated as labels of `[a-z0-9-]`, so no service name contains `_`.

hickory-server 0.26.3 behavior that matters here (read from its source):
- The response is encoded with a size limit: the response's own EDNS max payload over UDP, 512 bytes when the request had no EDNS, 65535 over TCP.
- When answers do not fit it emits as many as fit and sets TC. When only additional records are cut it also sets TC.

## Goals / Non-Goals

**Goals:**

- Answer SRV for every service in the zone, on the service name and on `_<label>._tcp.<service>`.
- Express each `balance` value through SRV priority, weight and order, so SRV-aware clients get the same behavior the A/AAAA path gives.
- Give SRV-aware clients addresses in the same reply (A/AAAA in the additional section) and make the target names resolvable on their own.
- Never set TC just because glue does not fit.

**Non-Goals:**

- `_udp` or other protocol labels: only `_tcp` owner names are answered; `_<label>._udp.<service>` stays NXDOMAIN.
- Validating the `_<label>` service label against a list (any label starting with `_` is accepted).
- Client libraries (libemfor and wrappers): no SRV lookup function; the store format and config file do not change.
- The DNS port-consistency warning (separate follow-up in config.yaml).
- PTR records for target names, DNSSEC, zone transfers.

## Decisions

### SRV owner names: the service name and `_<label>._tcp.<service>`

An SRV query is answered when the name is a service name, or exactly `_<label>._tcp.<service name>` where `<label>` is one label starting with `_` followed by at least one character. Both get the same records; the owner name of each answer is the query name as received.

`_tcp.<service>` is an empty non-terminal: every type gets NODATA. Any other type on `_<label>._tcp.<service>` (A, AAAA, TXT, …) also gets NODATA, because the name exists.

Alternatives: answering only `_<label>._tcp` names (RFC 2782 style) would break `dig SRV api.svc.emfor` and simple tools that ask the plain name; answering only the plain name would break gRPC and Go's `net.LookupSRV("http", "tcp", name)`, which build the `_http._tcp.` name. Answering both costs one name check.

### SRV candidates cover both address families with one health rule

SRV lists backends of both families. Candidates are the healthy backends of the service in config order; when none is healthy, `on_all_down = "open"` lists every backend and `"closed"` answers SERVFAIL. This differs from A/AAAA, where health is judged per family, because an SRV client may connect over either family.

Implement as a new function in select.rs, for example `srv_candidates(record) -> Result<Vec<&BackendRecord>, Selection>`, sharing the health filter with `candidate_backends` rather than copying it. Backends whose `ip` does not parse are skipped, as `candidate_backends` does.

### Balance maps to SRV priority, weight and order

| balance | priority | weight | answer order |
| --- | --- | --- | --- |
| `failover` | position among candidates in config order (0, 1, 2, …) | 1 | config order |
| `weighted` | 0 | backend `weight` | config order |
| `locality` | 0 for backends whose `site` equals the host's `site`; 1 for the rest. All 0 when the host has no site or no candidate matches | 1 | rotated, then stably grouped by priority |
| `round-robin`, `sticky`, unknown | 0 | 1 | rotated |

"Rotated" means the candidate list starts at index `n % len` and wraps, where `n` comes from a new per-service counter `srv: AtomicU64` in `RoundRobin` (live.rs), incremented once per SRV answer. It is separate from the A/AAAA counters so SRV queries do not shift A/AAAA answers. Rotation keeps clients that take the first record (many do, despite RFC 2782) spread out; SRV-correct clients ignore order within a priority.

`failover` priority is the position among the candidates, not among all backends, so the first healthy backend always has priority 0. `weighted` keeps config order because RFC 2782 clients do their own weighted random pick.

Alternatives: one shared counter with A/AAAA — rejected because it would make the A answer sequence depend on SRV traffic, breaking the "each W picks contain each backend exactly weight times" property tested in emfor-balance-2.

### Target names are derived from the backend IP

Each SRV record's target is `<ip label>.<service name>.`:
- IPv4: the dotted address with `.` replaced by `-`: `10.0.0.11` → `10-0-0-11.api.svc.emfor.`
- IPv6: all eight groups as four lowercase hex digits joined by `-`: `2001:db8::11` → `2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor.`

The IPv6 form is the full, uncompressed form because compressed forms can start or end with `-` (`::1` → `--1`, `2001:db8::` → `2001-db8--`), and some SRV clients (Go's resolver among them) reject targets whose labels start or end with `-`. The full form is always 39 characters, inside the 63-character label limit.

Backends that share an IP but not a port share one target name, each with its own SRV record and port.

The server answers A (IPv4 target) or AAAA (IPv6 target) for a target name with that backend's address, regardless of health, with the service's TTL. Other types on a target name get NODATA. A name under a service whose first label is not the target label of any backend of that service is NXDOMAIN, as today. Matching computes the target label of each backend and compares it with the query's first label (case-insensitive); it never parses the label, so `010-0-0-11` or a compressed IPv6 form does not match.

Names are classified in this order: exact service name, SRV owner name, `_tcp` empty non-terminal, target name, proper ancestor of a service (existing empty non-terminal rule), NXDOMAIN. A service whose name looks like a target name of another service (for example a service named `10-0-0-11.api.svc.emfor`) wins because exact service names are checked first.

Alternatives: the service name itself as the target — rejected because one A/AAAA name cannot carry a different address per SRV record; a fixed host label like `b0`, `b1` — rejected because indexes change when config order changes and caches would map them to the wrong address.

### Glue records go in the additional section, all or none

An SRV answer carries one A or AAAA record per distinct target, in answer order, with the service TTL, in the additional section. When the response with glue does not fit the size limit, all glue is dropped and TC stays clear; the resolver can resolve the target names separately. When the SRV answers alone do not fit, emfor drops SRV records from the end until the response fits and sets TC itself, so the resolver retries over TCP.

emfor trims the answers itself instead of leaving it to hickory-server because hickory, when it cuts answers, also leaves out the OPT record (found while implementing: a 600-byte EDNS request got a truncated reply with no OPT). RFC 6891 requires the OPT record in a response to an EDNS request, and keeping it tells the client the server's size.

Dropping all glue rather than a prefix keeps the rule simple and the reply deterministic for tests; partial glue gives a client no guarantee anyway.

`Reply` gains `additionals: Vec<Record>`. The handler calls the pure helper `fit_response(&mut Reply, &Queries, edns, limit) -> bool` (true when answers were dropped, which sets TC). It measures the response by building it with hickory-server's own `MessageResponseBuilder` and encoder, because hickory does not compress names against the question section; an approximation through hickory-proto's `Message` came out 13 bytes short for `api.svc.emfor`.

### UDP size limit follows the client's advertised size

Over UDP the limit is the request's EDNS max payload clamped to 512…1232, or 512 without EDNS; over TCP it is 65535. The response's EDNS max payload is set to the same clamped value (instead of the fixed 1232), because hickory uses the response's value as the encoder limit; with the fixed 1232 a client that advertised 512 could receive a 1232-byte UDP reply.

1232 stays the ceiling (DNS flag day 2020 value) to avoid IP fragmentation.

### Sizes are measured by a test

Measured by the unit test `srv_size_boundaries` in crates/emfor-daemon/src/dns.rs, for IPv4 targets `10-0-x-y.api.svc.emfor` on one port, round-robin:

| Limit | SRV records with glue | SRV records without glue |
| --- | --- | --- |
| 512 bytes, no EDNS | 8 | 11 |
| 1232 bytes, with EDNS | 19 | 27 |

IPv6 records are larger (target label 39 characters, AAAA 28 bytes), so their counts are lower. No decision depends on these numbers: the rules above work by encoding the actual response. They replace the estimates written at proposal time (8 / 11 and 20 / 27).

## Implementation Contract

**Behavior (observable with `dig`):**
- `dig @127.0.0.1 -p 5300 SRV api.svc.emfor` and `dig … SRV _http._tcp.api.svc.emfor` return NOERROR, AA set, one SRV record per candidate backend with priority, weight, port and target as in the mapping table, TTL = the service's DNS TTL, and A/AAAA glue in the additional section when it fits.
- `dig … A 10-0-0-11.api.svc.emfor` returns that backend's address even when it is down.
- `dig … SRV _tcp.api.svc.emfor` and `dig … TXT _http._tcp.api.svc.emfor` return NODATA with SOA; `dig … SRV _http._udp.api.svc.emfor` and `dig … A 10-0-0-99.api.svc.emfor` (no such backend) return NXDOMAIN with SOA.
- All backends down with `on_all_down = "closed"`: SRV returns SERVFAIL; target-name A/AAAA still answers.
- A request without EDNS for a service with many backends returns a response of at most 512 bytes: glue dropped first, TC set only when SRV records were cut.

**Interface / data shape:**
- `Reply` gains `additionals: Vec<Record>`; existing callers set it empty.
- `answer` keeps its signature; it returns glue in `additionals` and the handler trims it.
- select.rs gains SRV candidate and ordering functions returning, per candidate, the backend (ip, port) and its priority and weight; it stays free of I/O and of DNS types.
- live.rs `RoundRobin` gains an SRV rotation counter kept across reloads with the other counters.
- No change to emfor-format, the config file, the LMDB store, or the clients.

**Failure modes:** startup (`ready = false`) stays SERVFAIL for every name; names outside the zone stay REFUSED; malformed requests keep their existing rcodes. A response that cannot be encoded at all keeps the current fallback (SERVFAIL header).

**Acceptance criteria:**
- Unit tests in crates/emfor-daemon/src/select.rs for each row of the mapping table, including rotation across two queries, locality with and without host site, and candidates across both families.
- Unit tests in crates/emfor-daemon/src/dns.rs for SRV on both owner forms, NODATA/NXDOMAIN classification of the new names, target-name A/AAAA including a down backend, IPv6 target labels, glue contents, and the size rules (512 without EDNS, client size clamped, TCP unlimited, glue dropped without TC, TC when SRV records alone overflow).
- An end-to-end `dig SRV` run against a live daemon in the Debian container.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass.

**In scope:** dns.rs, select.rs, live.rs, docs/deploy/dns-resolver.md, docs/spectra/LANGUAGE.md, docs/spectra/config.yaml.
**Out of scope:** see Non-Goals.

## Risks / Trade-offs

- [Clients that take only the first SRV record] → rotation for round-robin-like methods spreads them; `failover` and `weighted` lists are in config order, which is correct for priority-aware clients.
- [Resolvers that cache SRV and target A records with different lifetimes] → both use the service TTL (1 second by default), and target names answer regardless of health, so a cached target never turns into NXDOMAIN while the backend is configured.
- [A target name that outlives its backend in a downstream cache] → NXDOMAIN after removal is cached for at most the SOA minimum (the `[dns] ttl`).
- [Large services overflow 512-byte UDP without EDNS] → glue is dropped first, then SRV records are cut with TC set, which triggers a TCP retry that emfor already serves.
- [emfor's size check and hickory's encoder disagree after a hickory upgrade] → the size is computed with hickory's own builder and encoder, and the over-the-wire test `srv_over_the_wire_fits_the_transport` checks the reply size, TC and the OPT record on real UDP and TCP sockets.
- [Changing the response EDNS size from fixed 1232 to the client's size] → affects A/AAAA responses too, which are far below 512 bytes, so no observable change there.

## Migration Plan

No data or config migration. Deploy by upgrading the daemon; resolvers that already forward the zone (dnsmasq, unbound, systemd-resolved, macOS /etc/resolver) forward SRV queries without config changes. Rollback restores NODATA for SRV.

Archive order: `emfor-dns`, then `emfor-balance-2`, then this change, because its delta modifies requirements those changes add.

## Open Questions

(none)
