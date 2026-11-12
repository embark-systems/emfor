## Why

Services today can only pick backends by `round-robin`, `sticky` or `failover`, so operators cannot send more traffic to bigger machines or keep traffic inside the local data centre. These two methods, `weighted` and `locality`, are the next balance methods the project has decided to build. Before any new `balance` value is written to the store, clients must stop treating an unknown value as an error: today a client that reads an unknown `balance` returns `INCOMPATIBLE` for that service, so a daemon upgraded ahead of a running application would break its lookups.

## What Changes

- **Unknown balance falls back to round-robin.** A client that reads a `balance` value it does not know picks by `round-robin` and ignores the affinity key, instead of failing with `INCOMPATIBLE`. Unknown fields in backend records and unknown `meta/` keys are already ignored and stay ignored; this becomes a written rule.
- **`balance = "weighted"`.** A backend entry may be written as a table `{ addr = "10.0.0.11:8080", weight = 3 }` besides the plain string; `weight` is a whole number from 1 to 100, default 1. Over any run of W consecutive choices among an unchanged candidate set, where W is the sum of the candidates' weights, each candidate is chosen exactly `weight` times. Clients map their per-process counter onto cumulative weight ranges; the DNS server uses smooth weighted round-robin.
- **`balance = "locality"`.** The daemon config gains a top-level `site` (the data centre this host is in), and a backend table entry may set `site`. Among the candidates chosen by health and `on_all_down`, those whose `site` equals the daemon's `site` are preferred; when none does, or the daemon has no `site`, all candidates are used. The choice inside that set is round-robin.
- **Store format version 1 gains fields, not a new version.** Backend records gain `weight` (number) and `site` (string or `null`); the store gains the key `meta/site` (absent when the daemon has no `site`). Clients read `meta/site`, so the DNS server and every client use the same site without extra client configuration.
- **Conformance data** gains cases for the fallback, `weighted` and `locality`, and the `emfor-conformance` driver gains an expectation of exact counts per address over a run of lookups.
- **Vocabulary and project context.** The planned balance method `zone` is renamed `locality` (it clashed with the DNS `zone`), the term `site` is added, `random` is removed from the plans (round-robin with a random start already behaves the same in clients), and `fastest` stays planned but deferred.

## Non-Goals (optional)

Recorded in design.md.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `service-config`: `site` key, backend table form with `weight` and `site`, `balance` accepts `weighted` and `locality`, new validation rules.
- `state-store`: backend records carry `weight` and `site`; new key `meta/site`.
- `backend-selection`: `weighted` and `locality` choice in the daemon (DNS answers). This capability is added by the in-progress change `emfor-dns`, which must be archived before this change is archived.
- `client-lookup`: unknown `balance` falls back to round-robin; `weighted` and `locality` in clients; conformance data covers both and the driver checks counts.

## Impact

- Affected specs: service-config, state-store, backend-selection, client-lookup.
- Affected code:
  - crates/emfor-format/src/lib.rs: `Balance` gains `Weighted`, `Locality` and an unknown-value variant; `BackendRecord` gains `weight` and `site`; new key constant for `meta/site`.
  - crates/emfor-daemon/src/config.rs: `site`, backend table entries, new `balance` values and validation.
  - crates/emfor-daemon/src/select.rs and crates/emfor-daemon/src/live.rs: weighted and locality choice for DNS, with smooth weighted round-robin state per service and family.
  - crates/emfor-daemon/src/writer.rs and crates/emfor-daemon/src/store.rs: write `weight`, `site` and `meta/site`.
  - clients/rust/src/select.rs, clients/rust/src/client.rs and clients/rust/src/store.rs: fallback, weighted and locality choice, reading `meta/site`.
  - tools/emfor-fixture/src/lib.rs and tools/emfor-fixture/src/bin/emfor-conformance.rs: count expectation; tests/conformance/v1/cases.json and tests/conformance/v1/README.md.
  - examples/emfor.toml, clients/rust/README.md, clients/c/README.md.
  - docs/spectra/LANGUAGE.md and docs/spectra/config.yaml.
- No change to the C API, the language wrappers' code, the DNS record types, or `SCHEMA_VERSION`. libemfor must be rebuilt, which the existing rule (libemfor and emfor upgrade together) already requires.
- Ordering: `emfor-dns` must be archived first, because this change modifies requirements that `emfor-dns` adds or modifies.
