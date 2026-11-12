## ADDED Requirements

### Requirement: Unknown balance falls back to round-robin

When a service record's `balance` is a value this client does not know, the client SHALL choose as for `round-robin`, SHALL ignore the affinity key, and SHALL NOT fail the lookup with `INCOMPATIBLE` for that reason. Unknown fields in service and backend records, and unknown keys in the store, SHALL be ignored.

#### Scenario: Future balance value

- **WHEN** a fresh service record has `"balance": "some-future-method"` and healthy backends `10.0.0.11:8080` and `10.0.0.12:8080`, and one process looks it up twice with affinity key `user-42`
- **THEN** both lookups succeed and the two results are exactly the set {`10.0.0.11:8080`, `10.0.0.12:8080`}

#### Scenario: Unknown fields ignored

- **WHEN** a backend object carries an extra field `"zone_hint": "x"` and the store has an extra key `meta/future`
- **THEN** the lookup returns that backend as if the extra field and key were absent

### Requirement: Weighted lookups follow backend weights

For a service with `balance = "weighted"`, the client SHALL take the next value of the service's round-robin counter (defined in "Round-robin spreads lookups across candidates"), compute `r = counter % W` where W is the sum of the candidates' `weight`, and return the first candidate in config order whose cumulative weight exceeds `r`. A backend object without `weight` SHALL count as weight 1. The affinity key SHALL be ignored. Over any W consecutive lookups in one process among an unchanged candidate set, each candidate SHALL be returned exactly `weight` times.

#### Scenario: Three to one

- **WHEN** a fresh weighted service has healthy backends `10.0.0.11:8080` (weight 3) and `10.0.0.12:8080` (weight 1), and one process looks it up 4 times in a row
- **THEN** `10.0.0.11:8080` is returned 3 times and `10.0.0.12:8080` once

##### Example: counter to backend

| `counter % 4` | Result |
| --- | --- |
| 0 | `10.0.0.11:8080` |
| 1 | `10.0.0.11:8080` |
| 2 | `10.0.0.11:8080` |
| 3 | `10.0.0.12:8080` |

#### Scenario: Unhealthy heavy backend

- **WHEN** the same service has `10.0.0.11:8080` marked `unhealthy` and is looked up 3 times
- **THEN** every result is `10.0.0.12:8080`

#### Scenario: Record from an older daemon

- **WHEN** a weighted service's backend objects have no `weight` field and two healthy backends, and the service is looked up twice
- **THEN** each backend is returned once

### Requirement: Locality prefers backends in the host's site

For a service with `balance = "locality"`, the client SHALL read `meta/site` in the same read transaction as the service record. When `meta/site` exists and at least one candidate's `site` equals it, the client SHALL narrow the candidates to those; otherwise it SHALL keep all candidates. It SHALL then choose by the round-robin counter as for `round-robin`. The affinity key SHALL be ignored.

#### Scenario: Local site healthy

- **WHEN** the store has `meta/site` = `dc1` and a fresh locality service has healthy `10.0.0.11:8080` (site dc1), `10.0.0.12:8080` (site dc1) and `10.0.0.13:8080` (site dc2), and one process looks it up 4 times
- **THEN** `10.0.0.11:8080` and `10.0.0.12:8080` are each returned twice and `10.0.0.13:8080` never

##### Example: locality outcomes

| `meta/site` | Backends (site, state) | `on_all_down` | Lookup results |
| --- | --- | --- | --- |
| dc1 | `10.0.0.11:8080` (dc1, unhealthy), `10.0.0.13:8080` (dc2, healthy) | open | `10.0.0.13:8080` |
| dc1 | `10.0.0.11:8080` (dc1, unhealthy), `10.0.0.13:8080` (dc2, unhealthy) | open | `10.0.0.11:8080` |
| dc1 | `10.0.0.11:8080` (dc1, unhealthy), `10.0.0.13:8080` (dc2, unhealthy) | closed | error `ALL_DOWN` |
| missing | `10.0.0.11:8080` (dc1, healthy), `10.0.0.13:8080` (dc2, healthy) | open | both, in turn |
| dc3 | `10.0.0.11:8080` (dc1, healthy), `10.0.0.13:8080` (no site, healthy) | open | both, in turn |

## MODIFIED Requirements

### Requirement: Lookup failures use four shared error codes

A lookup that cannot return an address SHALL fail with exactly one of these codes, decided by the first step that fails, in this order:

1. `UNAVAILABLE` — the store file cannot be opened (missing, permission denied, LMDB lock or data format different from the client's, any other open error), the key `meta/schema_version` is absent, or `EMFOR_MAX_STALENESS` holds an invalid value.
2. `INCOMPATIBLE` — `meta/schema_version` is not `1`, `meta/heartbeat` is missing or not a decimal integer, or the service record cannot be parsed as a version 1 record. A `balance` string the client does not know SHALL NOT count as a parse failure (see "Unknown balance falls back to round-robin").
3. `NOT_FOUND` — no `svc/<name>` key exists for the normalized name.
4. `ALL_DOWN` — no backend is usable and the service's `on_all_down` is `closed`.

Each failure SHALL carry a human-readable message naming the cause. Each language SHALL expose the code in its own idiom (a status value in the C library, an error value in Rust) with the code's name available as the exact strings above. A malformed store SHALL NOT make the lookup crash, panic or abort.

#### Scenario: Error code by store condition

- **WHEN** a lookup runs against each store condition below
- **THEN** it fails with the listed code

##### Example: store conditions

| Store condition | Code |
| --- | --- |
| store file does not exist | `UNAVAILABLE` |
| store exists, `meta/schema_version` absent | `UNAVAILABLE` |
| `EMFOR_MAX_STALENESS=fast` | `UNAVAILABLE` |
| `meta/schema_version` = `2` | `INCOMPATIBLE` |
| `meta/heartbeat` absent | `INCOMPATIBLE` |
| `svc/api.svc.emfor` = `{not json` | `INCOMPATIBLE` |
| `svc/api.svc.emfor` has `"balance":5` (not a string) | `INCOMPATIBLE` |
| `svc/api.svc.emfor` has `"balance":"some-future-method"` | no error: chosen as `round-robin` |
| no `svc/nope.svc.emfor` key, lookup `nope.svc.emfor` | `NOT_FOUND` |
| every backend `unhealthy`, `on_all_down` = `closed` | `ALL_DOWN` |

#### Scenario: Schema check wins over a missing service

- **WHEN** `meta/schema_version` is `2` and the looked-up name has no record
- **THEN** the lookup fails with `INCOMPATIBLE`, not `NOT_FOUND`

#### Scenario: Missing store is retried

- **WHEN** a lookup fails with `UNAVAILABLE` because the store file does not exist, and the daemon then creates the store
- **THEN** the next lookup in the same process opens the store and returns an address

### Requirement: Shared conformance data defines the contract

The repository SHALL contain conformance data under `tests/conformance/v1/`: `cases.json`, a list of cases each giving store contents and lookups with their expected address, expected set of addresses, expected count per address, or expected error code; and `sticky-v1.json`, the rendezvous hash and pick vectors. A fixture tool SHALL write a case's store contents to an LMDB file in the daemon's format. Every client interface (the Rust API, the C API, and the Go, Python, Perl and PHP wrappers) SHALL have a test runner that executes every case and every pick vector, and SHALL pass all of them; the Rust runner SHALL also check every hash vector, which no other interface exposes. The wrappers and the C library SHALL be checked by one shared driver, `emfor-conformance`, which writes each case's store and runs a small lookup program of the interface under test once per lookup, with `EMFOR_STORE` and `EMFOR_MAX_STALENESS` set for that case. A lookup with an expected count per address (`expect_counts`) SHALL run the sum of the counts as lookups in one process and SHALL pass only when every address appears exactly its count and no other result appears.

#### Scenario: Client passes the shared cases

- **WHEN** a client's conformance runner runs over `tests/conformance/v1/cases.json` and `sticky-v1.json`
- **THEN** every lookup result matches its expectation and the runner reports zero failures

#### Scenario: Data covers every requirement

- **WHEN** the cases are listed by the requirement they exercise
- **THEN** each requirement in this capability except thread and fork safety, the C library sharing the daemon's LMDB, and the language wrappers has at least one case, and the four error codes, the five balance methods, an unknown balance value, both `on_all_down` settings and all four heartbeat forms (fresh, stale, zero, missing) each appear in at least one case

#### Scenario: Driver detects a wrong answer

- **WHEN** `emfor-conformance` runs with a lookup program that always prints `OK 10.9.9.9:1`
- **THEN** it reports at least one failure and exits with a non-zero status

#### Scenario: Driver detects wrong counts

- **WHEN** a lookup expects `{"10.0.0.11:8080": 3, "10.0.0.12:8080": 1}` and the lookup program prints `10.0.0.11:8080` twice and `10.0.0.12:8080` twice
- **THEN** the driver reports that lookup as a failure

##### Example: lookup program protocol

| Item | Value |
| --- | --- |
| Invocation | `<program> <name> <affinity-key> <count>`; the key argument is `""` for no key |
| Output | `count` lines, one per lookup in the same process: `OK <address>` or `ERR <CODE> <message>` |
| Environment | `EMFOR_STORE` = the case's store path; `EMFOR_MAX_STALENESS` = the case's `max_staleness`, or `<max_staleness_ms>ms` |
| Exit status | `0` when every line was printed; any other status counts as a failure of every lookup in that run |
| Count | the length of `expect_all_of`, the sum of `expect_counts`, or 1 |
