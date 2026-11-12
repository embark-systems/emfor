## MODIFIED Requirements

### Requirement: Shared conformance data defines the contract

The repository SHALL contain conformance data under `tests/conformance/v1/`: `cases.json`, a list of cases each giving store contents and lookups with their expected address, expected set of addresses, expected count per address, or expected error code; and `sticky-v1.json`, the rendezvous hash and pick vectors. A fixture tool SHALL write a case's store contents to an LMDB file in the daemon's format. Every client interface (the Rust API, the C API, and the Go, Python, Perl and PHP wrappers) SHALL have a test runner that executes every case and every pick vector, and SHALL pass all of them; the Rust runner SHALL also check every hash vector, which no other interface exposes. The wrappers and the C library SHALL be checked by one shared driver, `emfor-conformance`, which writes each case's store and runs a small lookup program of the interface under test once per lookup, with `EMFOR_STORE` and `EMFOR_MAX_STALENESS` set for that case. A lookup with an expected count per address (`expect_counts`) SHALL run the sum of the counts as lookups in one process and SHALL pass only when every address appears exactly its count and no other result appears.

#### Scenario: Client passes the shared cases

- **WHEN** a client's conformance runner runs over `tests/conformance/v1/cases.json` and `sticky-v1.json`
- **THEN** every lookup result matches its expectation and the runner reports zero failures

#### Scenario: Data covers every requirement

- **WHEN** the cases are listed by the requirement they exercise
- **THEN** each requirement in this capability except thread and fork safety, the C library sharing the daemon's LMDB, and the language wrappers has at least one case, and the four error codes, the six balance methods, an unknown balance value, both `on_all_down` settings and all four heartbeat forms (fresh, stale, zero, missing) each appear in at least one case

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

## ADDED Requirements

### Requirement: Fastest lookups rotate over the daemon's pool

For a service with `balance = "fastest"`, when the client sees the store as fresh and the record's `pool` lists at least one `id` of a backend in the record, the client SHALL choose among those backends, in config order, by the round-robin counter as for `round-robin`. Otherwise (stale heartbeat, missing or empty `pool`, or no listed `id` matching a backend) the client SHALL choose exactly as for `round-robin`, including the all-down rules. The client SHALL NOT read `latency_us` or judge backends itself, and the affinity key SHALL be ignored.

#### Scenario: Lookups stay in the pool

- **WHEN** a fresh `fastest` service has healthy `10.0.0.11:8080`, `10.0.0.12:8080` and `10.0.0.13:8080` and `pool` `["10.0.0.11:8080", "10.0.0.12:8080"]`, and one process makes four lookups
- **THEN** `10.0.0.11:8080` and `10.0.0.12:8080` are each returned twice and `10.0.0.13:8080` never

#### Scenario: Stale heartbeat ignores the pool

- **WHEN** the same service's heartbeat is stale and `on_all_down = "open"`, and one process makes three lookups
- **THEN** each of the three backends is returned once

#### Scenario: Missing or empty pool

- **WHEN** a fresh `fastest` service with two healthy backends has no `pool` key, or has `"pool":[]`, and one process makes two lookups
- **THEN** the two lookups return different backends and neither fails
