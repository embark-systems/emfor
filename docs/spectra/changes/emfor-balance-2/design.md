## Context

`balance` is chosen per service (or in `[defaults]`) and written by the daemon into every service record in the LMDB store; the DNS server and every client pick backends by it. The values today are `round-robin`, `sticky` and `failover`, defined by the `emfor-format` crate's `Balance` enum, which both the daemon and the Rust client (and so libemfor and every wrapper) deserialize with serde. An unknown value therefore fails `serde_json::from_str` in the client's record parsing and the lookup returns `INCOMPATIBLE`. Unknown object fields are already ignored, because no record struct uses `deny_unknown_fields`.

Backends are configured as strings (`"10.0.0.11:8080"`); the daemon's `Backend` config struct holds only `id` and `addr`, and `BackendRecord` in the store holds `id`, `ip`, `port`, `state`, `checked_at`, `reason`.

The daemon chooses for DNS in `crates/emfor-daemon/src/select.rs` (candidates per address family, then `balance`, with one round-robin counter per service and family kept in the live state). Clients choose in `clients/rust/src/select.rs` (candidates over all backends, then `balance`, with a per-process counter per service starting at a random value).

The change `emfor-dns` is still open (waiting for CI) and owns the `backend-selection` capability and the current text of the `service-config` and `state-store` requirements that mention `balance`. This change's delta specs are written against the text `emfor-dns` produces.

## Goals / Non-Goals

**Goals:**

- A client that reads a `balance` it does not know keeps working, using round-robin.
- `weighted`: per-backend `weight`, exact proportions over a cycle, in both DNS and clients.
- `locality`: one `site` per host in the daemon config, per-backend `site`, same-site preference in both DNS and clients, with no new client configuration.
- Conformance data and the shared driver prove the client behaviour for all six interfaces.
- Vocabulary and project context match the decisions (`zone` → `locality`, `site` added, `random` removed, `fastest` deferred).

**Non-Goals:**

- `fastest`, `random`, and any other balance method.
- Combining methods: weights inside `locality`, weights or sites in `sticky`, weighted rendezvous hashing.
- A client-side override of the site (for example an `EMFOR_SITE` variable); the daemon's `site` is the only source.
- DNS SRV records (the later change `emfor-dns-srv`); new DNS record types.
- Weight 0 or draining a backend through weight.
- A change to `SCHEMA_VERSION`, the C API, or the wrapper code in Go, Python, Perl and PHP.

## Decisions

### Unknown balance falls back to round-robin

`Balance` gains a catch-all variant (`#[serde(other)] Unknown`). Clients treat it exactly as `round-robin` with the affinity key ignored. The daemon never produces it: its config parser accepts only named values. The fallback is written into the `client-lookup` spec and covered by a conformance case whose record has `"balance": "some-future-method"`.

Alternatives: bumping `SCHEMA_VERSION` for new values makes old clients fail for every service instead of one, which is worse; failing with `INCOMPATIBLE` (today's behaviour) breaks lookups when the daemon is upgraded before a running application reloads libemfor. No reader of the format has shipped yet, so the fallback costs nothing to adopt now.

### Backend table entries carry weight and site

A `backends` entry is either the existing string or a table `{ addr = "<ip:port>", weight = <1..100>, site = "<label>" }`; `weight` defaults to 1, `site` defaults to none. Both forms may be mixed in one list. Duplicate detection uses the normalized `addr`. `weight` and `site` are accepted under every `balance` and ignored by the methods that do not use them, so switching `balance` never needs a backend rewrite.

`site` labels and the top-level `site` follow the same rule: 1 to 63 characters of `[a-z0-9-]`, not starting or ending with `-`, after ASCII lowercasing; sites compare equal only when the lowercased labels are identical. The upper bound of 100 for `weight` keeps W (the sum of weights) small; see the risk below.

Alternatives: a separate `[services.x.weights]` map keyed by address splits one backend's settings across two places; a string syntax like `"10.0.0.11:8080 weight=3"` needs its own parser and error messages.

### Store records carry weight and site, and meta/site holds the host's site

`BackendRecord` gains `weight` (number, always written, default 1) and `site` (string or `null`). The daemon writes `meta/site` when the config has a top-level `site` and deletes the key when it does not; a reload that changes `site` rewrites or deletes it. Readers treat a missing `weight` as 1 and a missing `site` as `null`, so records written by an older daemon still parse. The format stays version 1 because readers already ignore unknown fields and keys.

Alternatives: putting the host site in every service record duplicates it and lets records disagree during a reload; an environment variable for clients would let the DNS server and clients disagree.

### Weighted choice by cumulative ranges in clients and smooth weighted round-robin in DNS

Both keep the property the spec states: over any W consecutive choices among an unchanged candidate set, each candidate is chosen exactly `weight` times.

- Clients reuse the per-process round-robin counter: `r = counter % W`, then the candidate whose cumulative weight range (in config order) contains `r`. O(candidates) per lookup, stateless besides the counter, and deterministic for conformance tests. Runs are bursty within one process (weights 3 and 1 give A, A, A, B), which is acceptable because many processes start at random counter values.
- The DNS server keeps per service and family a smooth weighted round-robin state (the nginx algorithm: add each weight to its current value, pick the largest, subtract W from it; earlier in config order wins ties). The state is reset when the candidate list changes. This interleaves answers (A, A, B, A for 3:1), which matters more for DNS because one answer is cached by the whole host for the TTL.

Alternatives: weighted random in clients gives no exact guarantee and cannot be checked by conformance cases; smooth weighted round-robin in clients needs per-service mutable state shared across threads, which the client avoids today.

### Locality narrows the candidates, then round-robin

After the health and `on_all_down` rules produce the candidates (per family in DNS, over all backends in clients), `locality` keeps the candidates whose `site` equals the host's site, if there is at least one; otherwise it keeps all candidates. It then chooses by round-robin with the existing counter. With no host site, `locality` is exactly `round-robin`. Health comes first, so a healthy remote backend is preferred to an unhealthy local one; only when no backend is healthy and `on_all_down` is `open` does locality pick among all backends, still preferring the local site.

Alternatives: preferring local backends before the health filter would send traffic to an unhealthy local backend while healthy remote ones exist.

### Vocabulary renames zone to locality and adds site

`docs/spectra/LANGUAGE.md`: the planned method `zone` becomes `locality` (the word `zone` already means the DNS zone); a new term `site` (the data centre label of a host or backend); `random` is removed from the table of balance values with a correction-log entry; `fastest` stays "planned". `docs/spectra/config.yaml` project context: the follow-up list drops `random` and `zone`, notes `weighted` and `locality` as done by this change, and keeps `fastest` and SRV.

## Implementation Contract

**Config (operator-visible).**

```toml
site = "dc1"                       # optional, top level

[services."api.svc.emfor"]
balance = "weighted"               # or "locality"
check = "tcp"
backends = [
  "10.0.0.10:8080",                                   # weight 1, no site
  { addr = "10.0.0.11:8080", weight = 3, site = "dc1" },
  { addr = "10.0.0.12:8080", site = "dc2" },
]
```

Validation errors, each with its TOML path, reported together with all other errors: `balance` not one of the five names; backend table without `addr`; unknown key inside a backend table; `weight` not an integer from 1 to 100; `site` (top level or backend) not a valid label; duplicate `addr`. `--check-config` and reload use the same rules. A change of `site`, `weight` or backend `site` takes effect on reload.

**Store.** Backend object: `{"id","ip","port","weight","site","state","checked_at","reason"}` with `weight` a number and `site` a string or `null`. Key `meta/site`: the label, present only when the config sets `site`. `meta/schema_version` stays `1`.

**DNS (daemon).** `weighted`: smooth weighted round-robin per service and family over the candidates. `locality`: same-site candidates of the family if any, else all candidates of the family; round-robin with the existing per-family counter.

**Clients.** Per lookup, in the same read transaction as the service record, read `meta/site` (missing → none). Unknown `balance` → round-robin, key ignored. `weighted` → cumulative ranges over `counter % W`, key ignored. `locality` → narrow by site, then `counter % len`, key ignored. Error codes are unchanged.

**Conformance.** `cases.json` lookups may use `expect_counts` (an object `{address: count}`); the driver runs the sum of the counts as one lookup program run and passes when each address appears exactly that many times and nothing else appears. New cases: unknown balance; weighted with weights 3 and 1 (4 lookups → 3 and 1); weighted with an unhealthy heavy backend; locality with local site healthy, local site all unhealthy, no `meta/site`, and `on_all_down` open with all unhealthy; records without `weight`/`site` (older daemon) parse. The "data covers every requirement" check counts five balance methods plus the fallback.

**Acceptance.** `cargo test --workspace --all-features` passes (daemon config, select and writer unit tests; client unit tests; driver tests); `make -C clients test` passes for C, Go, Python, Perl and PHP with zero conformance failures; a daemon started with the config above answers A queries for `api.svc.emfor` in the 1:3:1 proportion over 5 queries under `weighted`, and only `10.0.0.11` under `locality` while it is healthy.

**In scope:** everything above, examples/emfor.toml, the Rust and C client READMEs, LANGUAGE.md and config.yaml. **Out of scope:** the Non-Goals, wrapper source code (they pass the new cases unchanged because they only forward to libemfor), deploy docs.

## Risks / Trade-offs

- [Client weighted runs are bursty within one process (estimated from the algorithm, not measured)] → Many processes start at random counter values, so host-wide load still follows the weights; DNS uses smooth weighted round-robin where one answer is shared host-wide.
- [W grows with weights and backends; with weight ≤ 100 and the client's O(candidates) mapping the per-lookup cost stays linear in the candidate count, and W only bounds the counter modulus] → The weight cap of 100 also keeps the DNS state's integers small; raise it later only with a measured need.
- [The smooth weighted round-robin state resets when health changes the candidate list, so proportions are exact only between changes] → Stated in the spec as "among an unchanged candidate set".
- [This change modifies requirements that `emfor-dns` adds or modifies, and `emfor-dns` is waiting for CI] → Archive `emfor-dns` first; if its requirement text changes before then, re-base this change's deltas on the new text.
- [Old libemfor (before this change) still returns `INCOMPATIBLE` for `weighted` or `locality` services] → No libemfor has been released; the existing rule that libemfor and emfor upgrade together covers installs from this change on.

## Migration Plan

No data migration: the daemon rewrites the store on every start. Existing configs keep working unchanged (string backends, three old values). Rollback is reverting the daemon and libemfor together; a store written by the new daemon is readable by the old client only for services using the old three values.
