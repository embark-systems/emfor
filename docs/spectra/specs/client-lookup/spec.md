# client-lookup Specification

## Purpose

Client libraries let an application pick a backend for a service by reading the daemon's local state store directly, without a network call. This capability defines the lookup contract that every language binding implements identically, and the shared conformance data that proves it.

## Requirements

### Requirement: Lookup returns one backend address

Each client library SHALL provide a lookup operation `emfor_lookup(name, affinity_key?)` that takes a service name and an optional affinity key, reads the state store, and returns exactly one backend address on success. The address SHALL be the backend record's `id`: `ip:port` for IPv4 and `[ip]:port` for IPv6.

#### Scenario: IPv4 backend

- **WHEN** service `api.svc.emfor` has one healthy backend with id `10.0.0.11:8080` and the store is fresh
- **THEN** the lookup returns `10.0.0.11:8080`

#### Scenario: IPv6 backend

- **WHEN** service `v6.svc.emfor` has one healthy backend with id `[2001:db8::11]:8080` and the store is fresh
- **THEN** the lookup returns `[2001:db8::11]:8080`


<!-- @trace
source: emfor-clients
updated: 2026-11-12
code:
  - clients/c/example.c
  - clients/rust/src/capi.rs
  - tests/conformance/v1/sticky-v1.json
  - clients/rust/src/store.rs
  - clients/rust/README.md
  - clients/c/README.md
  - clients/rust/src/client.rs
  - clients/rust/examples/lookup.rs
  - tests/conformance/v1/cases.json
  - clients/c/Makefile
  - clients/rust/src/select.rs
  - tools/emfor-fixture/Cargo.toml
  - tools/emfor-fixture/src/lib.rs
  - tests/conformance/v1/README.md
  - tests/spike/probe_read.py
  - tests/spike/RESULTS.md
  - clients/rust/src/config.rs
  - tests/spike/probe/Cargo.toml
  - clients/rust/src/lib.rs
  - Cargo.toml
  - tests/spike/probe/cbindgen.toml
  - tools/emfor-fixture/src/main.rs
  - clients/rust/cbindgen.toml
  - tests/spike/probe_read.php
  - .github/workflows/ci.yml
  - clients/rust/Cargo.toml
  - tests/spike/probe_read.c
  - tests/spike/run.sh
  - tests/spike/probe/src/lib.rs
  - tests/spike/check_symbols.sh
tests:
  - clients/rust/tests/conformance_capi.rs
  - clients/rust/tests/fork.rs
  - clients/rust/tests/lock_format.rs
  - clients/c/tests/check_symbols.sh
  - clients/c/tests/test_fork.c
  - clients/rust/tests/env_default.rs
  - clients/rust/tests/env_invalid.rs
  - clients/rust/tests/threads.rs
  - clients/rust/tests/support/mod.rs
  - clients/c/tests/test_threads.c
  - clients/c/tests/common.h
  - clients/rust/tests/conformance.rs
  - clients/rust/tests/same_lmdb.rs
  - clients/c/tests/fixture.json
  - clients/rust/tests/env_retry.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - clients/c/tests/test_lookup.c
-->

---
### Requirement: Lookup failures use four shared error codes

A lookup that cannot return an address SHALL fail with exactly one of these codes, decided by the first step that fails, in this order:

1. `UNAVAILABLE` — the store file cannot be opened (missing, permission denied, LMDB lock or data format different from the client's, any other open error), the key `meta/schema_version` is absent, or `EMFOR_MAX_STALENESS` holds an invalid value.
2. `INCOMPATIBLE` — `meta/schema_version` is neither `1` nor `2`, `meta/heartbeat` is missing or not a decimal integer, or the service record cannot be parsed as a version 2 record. A `balance` string the client does not know SHALL NOT count as a parse failure (see "Unknown balance falls back to round-robin").
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
| `meta/schema_version` = `3` | `INCOMPATIBLE` |
| `meta/heartbeat` absent | `INCOMPATIBLE` |
| `svc/api.svc.emfor` = `{not json` | `INCOMPATIBLE` |
| `svc/api.svc.emfor` has `"balance":5` (not a string) | `INCOMPATIBLE` |
| `svc/api.svc.emfor` has `"balance":"some-future-method"` | no error: chosen as `round-robin` |
| no `svc/nope.svc.emfor` key, lookup `nope.svc.emfor` | `NOT_FOUND` |
| every backend `unhealthy`, `on_all_down` = `closed` | `ALL_DOWN` |

#### Scenario: Schema check wins over a missing service

- **WHEN** `meta/schema_version` is `3` and the looked-up name has no record
- **THEN** the lookup fails with `INCOMPATIBLE`, not `NOT_FOUND`

#### Scenario: Missing store is retried

- **WHEN** a lookup fails with `UNAVAILABLE` because the store file does not exist, and the daemon then creates the store
- **THEN** the next lookup in the same process opens the store and returns an address

#### Scenario: Unopenable store is retried after a pause

- **WHEN** a lookup fails with `UNAVAILABLE` because the store file exists but cannot be opened, and the file is then replaced by a valid store
- **THEN** lookups in the same process within 1 second of the failed open fail with the same error without opening the file, and the first lookup after that opens the store and returns an address


<!-- @trace
source: emfor-backup-backends
updated: 2026-11-14
code:
  - docs/manual.zh_TW/05-backend-selection.md
  - clients/go/emfor.go
  - crates/emfor-daemon/src/status.rs
  - tests/conformance/v1/README.md
  - clients/python/emfor/__init__.py
  - crates/emfor-daemon/src/writer.rs
  - docs/manual.zh_TW/07-dns.md
  - examples/emfor.toml
  - tests/conformance/v1/cases.json
  - README.md
  - clients/rust/src/lib.rs
  - clients/perl/README.md
  - clients/rust/src/client.rs
  - crates/emfor-format/src/lib.rs
  - docs/manual.zh_TW/03-configuration.md
  - crates/emfor-daemon/src/state.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/select.rs
  - crates/emfor-daemon/src/config.rs
  - clients/python/README.md
  - clients/rust/README.md
  - docs/manual/06-clients.md
  - docs/manual/07-dns.md
  - CHANGELOG.md
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/dns.rs
  - docs/manual.zh_TW/06-clients.md
  - docs/manual/05-backend-selection.md
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/metrics.rs
  - crates/emfor-format/src/rules.rs
  - clients/go/README.md
  - clients/php/README.md
tests:
  - crates/emfor-daemon/tests/daemon.rs
  - clients/rust/tests/conformance_capi.rs
-->

---
### Requirement: Service names are normalized before lookup

The lookup SHALL normalize the given name by ASCII-lowercasing it and removing one trailing dot, the same normalization the daemon applies to configured service names, and SHALL then read key `svc/<normalized name>`.

#### Scenario: Name forms that find the same record

- **WHEN** the store has `svc/api.svc.emfor` and the lookup is called with each name below
- **THEN** the result is as listed

##### Example: name normalization

| Name passed | Result |
| --- | --- |
| `api.svc.emfor` | found |
| `API.Svc.Emfor` | found |
| `api.svc.emfor.` | found |
| `api.svc.emfor..` | `NOT_FOUND` |


<!-- @trace
source: emfor-clients
updated: 2026-11-12
code:
  - clients/c/example.c
  - clients/rust/src/capi.rs
  - tests/conformance/v1/sticky-v1.json
  - clients/rust/src/store.rs
  - clients/rust/README.md
  - clients/c/README.md
  - clients/rust/src/client.rs
  - clients/rust/examples/lookup.rs
  - tests/conformance/v1/cases.json
  - clients/c/Makefile
  - clients/rust/src/select.rs
  - tools/emfor-fixture/Cargo.toml
  - tools/emfor-fixture/src/lib.rs
  - tests/conformance/v1/README.md
  - tests/spike/probe_read.py
  - tests/spike/RESULTS.md
  - clients/rust/src/config.rs
  - tests/spike/probe/Cargo.toml
  - clients/rust/src/lib.rs
  - Cargo.toml
  - tests/spike/probe/cbindgen.toml
  - tools/emfor-fixture/src/main.rs
  - clients/rust/cbindgen.toml
  - tests/spike/probe_read.php
  - .github/workflows/ci.yml
  - clients/rust/Cargo.toml
  - tests/spike/probe_read.c
  - tests/spike/run.sh
  - tests/spike/probe/src/lib.rs
  - tests/spike/check_symbols.sh
tests:
  - clients/rust/tests/conformance_capi.rs
  - clients/rust/tests/fork.rs
  - clients/rust/tests/lock_format.rs
  - clients/c/tests/check_symbols.sh
  - clients/c/tests/test_fork.c
  - clients/rust/tests/env_default.rs
  - clients/rust/tests/env_invalid.rs
  - clients/rust/tests/threads.rs
  - clients/rust/tests/support/mod.rs
  - clients/c/tests/test_threads.c
  - clients/c/tests/common.h
  - clients/rust/tests/conformance.rs
  - clients/rust/tests/same_lmdb.rs
  - clients/c/tests/fixture.json
  - clients/rust/tests/env_retry.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - clients/c/tests/test_lookup.c
-->

---
### Requirement: Stale heartbeat makes every backend unknown

The store SHALL be treated as stale when `meta/heartbeat` is `0`, or when the reader's current Unix time in milliseconds minus the heartbeat is greater than the maximum staleness (default 3 seconds). A heartbeat later than the reader's clock SHALL count as fresh. When the store is stale, the lookup SHALL ignore every backend's `state` and treat no backend as healthy, so the service's `on_all_down` decides the result. There SHALL be no separate error code for staleness.

#### Scenario: Stale store with each on_all_down setting

- **WHEN** the heartbeat is older than the maximum staleness and every backend is recorded `healthy`
- **THEN** a service with `on_all_down = "closed"` fails with `ALL_DOWN`, and a service with `on_all_down = "open"` returns one of its backends

#### Scenario: Heartbeat boundaries

- **WHEN** the maximum staleness is 3000 ms and the reader's clock reads 1700000010000
- **THEN** freshness is decided as listed

##### Example: heartbeat values

| `meta/heartbeat` | Store is |
| --- | --- |
| `1700000007000` (3000 ms old) | fresh |
| `1700000006999` (3001 ms old) | stale |
| `1700000015000` (in the future) | fresh |
| `0` | stale |


<!-- @trace
source: emfor-clients
updated: 2026-11-12
code:
  - clients/c/example.c
  - clients/rust/src/capi.rs
  - tests/conformance/v1/sticky-v1.json
  - clients/rust/src/store.rs
  - clients/rust/README.md
  - clients/c/README.md
  - clients/rust/src/client.rs
  - clients/rust/examples/lookup.rs
  - tests/conformance/v1/cases.json
  - clients/c/Makefile
  - clients/rust/src/select.rs
  - tools/emfor-fixture/Cargo.toml
  - tools/emfor-fixture/src/lib.rs
  - tests/conformance/v1/README.md
  - tests/spike/probe_read.py
  - tests/spike/RESULTS.md
  - clients/rust/src/config.rs
  - tests/spike/probe/Cargo.toml
  - clients/rust/src/lib.rs
  - Cargo.toml
  - tests/spike/probe/cbindgen.toml
  - tools/emfor-fixture/src/main.rs
  - clients/rust/cbindgen.toml
  - tests/spike/probe_read.php
  - .github/workflows/ci.yml
  - clients/rust/Cargo.toml
  - tests/spike/probe_read.c
  - tests/spike/run.sh
  - tests/spike/probe/src/lib.rs
  - tests/spike/check_symbols.sh
tests:
  - clients/rust/tests/conformance_capi.rs
  - clients/rust/tests/fork.rs
  - clients/rust/tests/lock_format.rs
  - clients/c/tests/check_symbols.sh
  - clients/c/tests/test_fork.c
  - clients/rust/tests/env_default.rs
  - clients/rust/tests/env_invalid.rs
  - clients/rust/tests/threads.rs
  - clients/rust/tests/support/mod.rs
  - clients/c/tests/test_threads.c
  - clients/c/tests/common.h
  - clients/rust/tests/conformance.rs
  - clients/rust/tests/same_lmdb.rs
  - clients/c/tests/fixture.json
  - clients/rust/tests/env_retry.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - clients/c/tests/test_lookup.c
-->

---
### Requirement: Candidates follow health and on_all_down

A backend is passing when its `state` is `healthy` and its `reason` is `null`, failing when its `state` is `healthy` and its `reason` is not `null`, a backup when its `backup` is true, and primary otherwise. When the store is fresh, the candidates SHALL be, in config order, the first of these sets that is not empty: the passing primary backends (`state` `healthy`, `reason` `null`, `backup` false); when there are none, the passing backup backends (`backup` true); when there are none, every `healthy` primary backend, failing ones included; when there are none, every `healthy` backup backend, failing ones included. When the store is stale, `reason` SHALL NOT affect the candidates. When no backend is a candidate (none healthy, or the store is stale), a service with `on_all_down = "closed"` SHALL fail with `ALL_DOWN`. A service with `on_all_down = "open"` SHALL then use, in config order: when the store is fresh and at least one backend is unknown and passing (`state` `unknown` and `reason` `null`), only those backends; otherwise, when the store is fresh and at least one backend's `state` is `unknown`, every `unknown` backend; otherwise every backend. When the store is stale, every backend SHALL be a candidate of an open service. The lookup SHALL NOT tell the caller that the result came from an all-down service or from a service whose healthy backends are all failing.

#### Scenario: Unhealthy backends are skipped

- **WHEN** a fresh failover service lists `10.0.1.5:5432` (`unhealthy`) then `10.0.1.6:5432` (`healthy`)
- **THEN** the lookup returns `10.0.1.6:5432`

#### Scenario: Open service with every backend down

- **WHEN** a fresh failover service with `on_all_down = "open"` lists `10.0.1.5:5432` (`unhealthy`) then `10.0.1.6:5432` (`unknown`)
- **THEN** the lookup returns `10.0.1.6:5432`

#### Scenario: Open service prefers a passing unknown backend

- **WHEN** a fresh round-robin service with `on_all_down = "open"` lists `10.0.1.5:5432` (`unhealthy`, reason `refused`), `10.0.1.6:5432` (`unknown`, reason `timeout`) and `10.0.1.7:5432` (`unknown`, reason `null`)
- **THEN** consecutive lookups return only `10.0.1.7:5432`

#### Scenario: Open service with every unknown backend failing

- **WHEN** a fresh round-robin service with `on_all_down = "open"` lists `10.0.1.5:5432` (`unhealthy`, reason `refused`), `10.0.1.6:5432` (`unknown`, reason `timeout`) and `10.0.1.7:5432` (`unknown`, reason `refused`)
- **THEN** consecutive lookups return `10.0.1.6:5432` and `10.0.1.7:5432` and never `10.0.1.5:5432`

#### Scenario: Open service with only unhealthy backends

- **WHEN** a fresh round-robin service with `on_all_down = "open"` lists `10.0.1.5:5432` and `10.0.1.6:5432`, both `unhealthy`
- **THEN** consecutive lookups return both backends

#### Scenario: Failing backend is skipped while another passes

- **WHEN** a fresh failover service lists `10.0.1.5:5432` (`healthy`, reason `connect: Connection refused`) then `10.0.1.6:5432` (`healthy`, reason `null`)
- **THEN** the lookup returns `10.0.1.6:5432`

#### Scenario: Every healthy backend failing

- **WHEN** a fresh round-robin service with `on_all_down = "closed"` lists `10.0.1.5:5432` (`healthy`, reason `timeout`), `10.0.1.6:5432` (`healthy`, reason `timeout`) and `10.0.1.7:5432` (`unhealthy`)
- **THEN** consecutive lookups return `10.0.1.5:5432` and `10.0.1.6:5432` and never `10.0.1.7:5432`

#### Scenario: Stale store ignores reason

- **WHEN** a stale failover service with `on_all_down = "open"` lists `10.0.1.5:5432` (`healthy`, reason `timeout`) then `10.0.1.6:5432` (`healthy`, reason `null`)
- **THEN** the lookup returns `10.0.1.5:5432`

#### Scenario: Backup takes over while no primary passes

- **WHEN** a fresh round-robin service with `on_all_down = "closed"` lists `10.0.1.5:5432` (`healthy`, reason `timeout`), backup `10.0.2.5:5432` (`healthy`, reason `null`) and backup `10.0.2.6:5432` (`healthy`, reason `null`)
- **THEN** consecutive lookups return `10.0.2.5:5432` and `10.0.2.6:5432` and never `10.0.1.5:5432`

#### Scenario: Primary passes again

- **WHEN** the same service's `10.0.1.5:5432` has reason `null` again
- **THEN** every lookup returns `10.0.1.5:5432`

##### Example: candidates by state, reason and heartbeat

| Backends in config order (state, reason) | Heartbeat | `on_all_down` | Candidates |
| --- | --- | --- | --- |
| A (healthy, null), B (healthy, "refused"), C (unhealthy, "refused") | fresh | closed | A |
| A (healthy, "refused"), B (healthy, "timeout"), C (unhealthy, "refused") | fresh | closed | A, B |
| A (unhealthy, "refused"), B (unknown, null) | fresh | closed | none: `ALL_DOWN` |
| A (unhealthy, "refused"), B (unknown, null) | fresh | open | B |
| A (unhealthy, "refused"), B (unknown, "timeout"), C (unknown, null) | fresh | open | C |
| A (unhealthy, "refused"), B (unknown, "timeout"), C (unknown, "refused") | fresh | open | B, C |
| A (unhealthy, "refused"), B (unhealthy, "timeout") | fresh | open | A, B |
| A (unhealthy, "refused"), B (unknown, "timeout"), C (unknown, null) | fresh | closed | none: `ALL_DOWN` |
| A (unhealthy, "refused"), B (unknown, null) | stale | open | A, B |
| A (healthy, "refused"), B (healthy, null) | stale | open | A, B |
| A (healthy, "refused"), B (healthy, null) | stale | closed | none: `ALL_DOWN` |
| A (healthy, "timeout"), B backup (healthy, null) | fresh | closed | B |
| A (healthy, "timeout"), B backup (healthy, "refused") | fresh | closed | A |
| A (unhealthy, "refused"), B backup (healthy, "refused") | fresh | closed | B |
| A (unhealthy, "refused"), B backup (unknown, null) | fresh | open | B |
| A (healthy, null), B backup (healthy, null) | stale | open | A, B |


<!-- @trace
source: emfor-backup-backends
updated: 2026-11-14
code:
  - docs/manual.zh_TW/05-backend-selection.md
  - clients/go/emfor.go
  - crates/emfor-daemon/src/status.rs
  - tests/conformance/v1/README.md
  - clients/python/emfor/__init__.py
  - crates/emfor-daemon/src/writer.rs
  - docs/manual.zh_TW/07-dns.md
  - examples/emfor.toml
  - tests/conformance/v1/cases.json
  - README.md
  - clients/rust/src/lib.rs
  - clients/perl/README.md
  - clients/rust/src/client.rs
  - crates/emfor-format/src/lib.rs
  - docs/manual.zh_TW/03-configuration.md
  - crates/emfor-daemon/src/state.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/select.rs
  - crates/emfor-daemon/src/config.rs
  - clients/python/README.md
  - clients/rust/README.md
  - docs/manual/06-clients.md
  - docs/manual/07-dns.md
  - CHANGELOG.md
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/dns.rs
  - docs/manual.zh_TW/06-clients.md
  - docs/manual/05-backend-selection.md
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/metrics.rs
  - crates/emfor-format/src/rules.rs
  - clients/go/README.md
  - clients/php/README.md
tests:
  - crates/emfor-daemon/tests/daemon.rs
  - clients/rust/tests/conformance_capi.rs
-->

---
### Requirement: Round-robin spreads lookups across candidates

For a service with `balance = "round-robin"`, or with no `balance` field, the client SHALL keep one counter per service name per process, starting at a random value on the first lookup of that service, and SHALL return `candidates[counter % len]` and increment the counter atomically on each lookup. The affinity key SHALL be ignored. Concurrent lookups from different threads SHALL each take a distinct counter value.

#### Scenario: Consecutive lookups cover every candidate

- **WHEN** a fresh round-robin service has healthy backends `10.0.0.11:8080`, `10.0.0.12:8080` and `10.0.0.13:8080`, and one process looks it up 3 times in a row
- **THEN** the 3 results are exactly the set {`10.0.0.11:8080`, `10.0.0.12:8080`, `10.0.0.13:8080`}

#### Scenario: Unhealthy backend is never returned

- **WHEN** the same service has `10.0.0.12:8080` marked `unhealthy` and is looked up 4 times
- **THEN** every result is `10.0.0.11:8080` or `10.0.0.13:8080`, and each appears twice


<!-- @trace
source: emfor-clients
updated: 2026-11-12
code:
  - clients/c/example.c
  - clients/rust/src/capi.rs
  - tests/conformance/v1/sticky-v1.json
  - clients/rust/src/store.rs
  - clients/rust/README.md
  - clients/c/README.md
  - clients/rust/src/client.rs
  - clients/rust/examples/lookup.rs
  - tests/conformance/v1/cases.json
  - clients/c/Makefile
  - clients/rust/src/select.rs
  - tools/emfor-fixture/Cargo.toml
  - tools/emfor-fixture/src/lib.rs
  - tests/conformance/v1/README.md
  - tests/spike/probe_read.py
  - tests/spike/RESULTS.md
  - clients/rust/src/config.rs
  - tests/spike/probe/Cargo.toml
  - clients/rust/src/lib.rs
  - Cargo.toml
  - tests/spike/probe/cbindgen.toml
  - tools/emfor-fixture/src/main.rs
  - clients/rust/cbindgen.toml
  - tests/spike/probe_read.php
  - .github/workflows/ci.yml
  - clients/rust/Cargo.toml
  - tests/spike/probe_read.c
  - tests/spike/run.sh
  - tests/spike/probe/src/lib.rs
  - tests/spike/check_symbols.sh
tests:
  - clients/rust/tests/conformance_capi.rs
  - clients/rust/tests/fork.rs
  - clients/rust/tests/lock_format.rs
  - clients/c/tests/check_symbols.sh
  - clients/c/tests/test_fork.c
  - clients/rust/tests/env_default.rs
  - clients/rust/tests/env_invalid.rs
  - clients/rust/tests/threads.rs
  - clients/rust/tests/support/mod.rs
  - clients/c/tests/test_threads.c
  - clients/c/tests/common.h
  - clients/rust/tests/conformance.rs
  - clients/rust/tests/same_lmdb.rs
  - clients/c/tests/fixture.json
  - clients/rust/tests/env_retry.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - clients/c/tests/test_lookup.c
-->

---
### Requirement: Failover returns the first candidate

For a service with `balance = "failover"`, the client SHALL return the first candidate in config order and SHALL ignore the affinity key.

#### Scenario: First healthy backend wins

- **WHEN** a fresh failover service lists `10.0.1.5:5432` (`healthy`) then `10.0.1.6:5432` (`healthy`), and is looked up with affinity key `user-42`
- **THEN** the lookup returns `10.0.1.5:5432`


<!-- @trace
source: emfor-clients
updated: 2026-11-12
code:
  - clients/c/example.c
  - clients/rust/src/capi.rs
  - tests/conformance/v1/sticky-v1.json
  - clients/rust/src/store.rs
  - clients/rust/README.md
  - clients/c/README.md
  - clients/rust/src/client.rs
  - clients/rust/examples/lookup.rs
  - tests/conformance/v1/cases.json
  - clients/c/Makefile
  - clients/rust/src/select.rs
  - tools/emfor-fixture/Cargo.toml
  - tools/emfor-fixture/src/lib.rs
  - tests/conformance/v1/README.md
  - tests/spike/probe_read.py
  - tests/spike/RESULTS.md
  - clients/rust/src/config.rs
  - tests/spike/probe/Cargo.toml
  - clients/rust/src/lib.rs
  - Cargo.toml
  - tests/spike/probe/cbindgen.toml
  - tools/emfor-fixture/src/main.rs
  - clients/rust/cbindgen.toml
  - tests/spike/probe_read.php
  - .github/workflows/ci.yml
  - clients/rust/Cargo.toml
  - tests/spike/probe_read.c
  - tests/spike/run.sh
  - tests/spike/probe/src/lib.rs
  - tests/spike/check_symbols.sh
tests:
  - clients/rust/tests/conformance_capi.rs
  - clients/rust/tests/fork.rs
  - clients/rust/tests/lock_format.rs
  - clients/c/tests/check_symbols.sh
  - clients/c/tests/test_fork.c
  - clients/rust/tests/env_default.rs
  - clients/rust/tests/env_invalid.rs
  - clients/rust/tests/threads.rs
  - clients/rust/tests/support/mod.rs
  - clients/c/tests/test_threads.c
  - clients/c/tests/common.h
  - clients/rust/tests/conformance.rs
  - clients/rust/tests/same_lmdb.rs
  - clients/c/tests/fixture.json
  - clients/rust/tests/env_retry.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - clients/c/tests/test_lookup.c
-->

---
### Requirement: Clients are configured only by environment variables

The client SHALL read `EMFOR_STORE` (store path, default `/var/lib/emfor/state.mdb`) and `EMFOR_MAX_STALENESS` (default `3s`) when it first opens the store in a process. `EMFOR_MAX_STALENESS` SHALL accept a positive integer followed by `ms` or `s`. Any other value SHALL make every lookup fail with `UNAVAILABLE` and a message naming the variable; the client SHALL NOT replace an invalid value with the default. The public API SHALL NOT include a configuration function.

#### Scenario: Staleness values

- **WHEN** `EMFOR_MAX_STALENESS` is set to each value below before the first lookup
- **THEN** the maximum staleness is as listed

##### Example: EMFOR_MAX_STALENESS parsing

| Value | Result |
| --- | --- |
| unset | 3000 ms |
| `3s` | 3000 ms |
| `1500ms` | 1500 ms |
| `0s` | `UNAVAILABLE` |
| `1.5s` | `UNAVAILABLE` |
| `3` | `UNAVAILABLE` |
| `fast` | `UNAVAILABLE` |

#### Scenario: Custom store path

- **WHEN** `EMFOR_STORE` names a store file in a test directory
- **THEN** the lookup reads that file and not the default path


<!-- @trace
source: emfor-clients
updated: 2026-11-12
code:
  - clients/c/example.c
  - clients/rust/src/capi.rs
  - tests/conformance/v1/sticky-v1.json
  - clients/rust/src/store.rs
  - clients/rust/README.md
  - clients/c/README.md
  - clients/rust/src/client.rs
  - clients/rust/examples/lookup.rs
  - tests/conformance/v1/cases.json
  - clients/c/Makefile
  - clients/rust/src/select.rs
  - tools/emfor-fixture/Cargo.toml
  - tools/emfor-fixture/src/lib.rs
  - tests/conformance/v1/README.md
  - tests/spike/probe_read.py
  - tests/spike/RESULTS.md
  - clients/rust/src/config.rs
  - tests/spike/probe/Cargo.toml
  - clients/rust/src/lib.rs
  - Cargo.toml
  - tests/spike/probe/cbindgen.toml
  - tools/emfor-fixture/src/main.rs
  - clients/rust/cbindgen.toml
  - tests/spike/probe_read.php
  - .github/workflows/ci.yml
  - clients/rust/Cargo.toml
  - tests/spike/probe_read.c
  - tests/spike/run.sh
  - tests/spike/probe/src/lib.rs
  - tests/spike/check_symbols.sh
tests:
  - clients/rust/tests/conformance_capi.rs
  - clients/rust/tests/fork.rs
  - clients/rust/tests/lock_format.rs
  - clients/c/tests/check_symbols.sh
  - clients/c/tests/test_fork.c
  - clients/rust/tests/env_default.rs
  - clients/rust/tests/env_invalid.rs
  - clients/rust/tests/threads.rs
  - clients/rust/tests/support/mod.rs
  - clients/c/tests/test_threads.c
  - clients/c/tests/common.h
  - clients/rust/tests/conformance.rs
  - clients/rust/tests/same_lmdb.rs
  - clients/c/tests/fixture.json
  - clients/rust/tests/env_retry.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - clients/c/tests/test_lookup.c
-->

---
### Requirement: Clients are safe across threads and fork

The client SHALL open the store read-only once per process and reuse it for later lookups. When a lookup finds the store stale, or fails with an error other than `ALL_DOWN` from a fresh store, the client SHALL check whether the file at the store path is now a different file (device and inode) than the open one, at most once every 100 ms in a process; a missing file SHALL NOT count as a different file. When it is different, the client SHALL switch to it once no lookup in the process uses the open store, and repeat the lookup once. When the lock file (`<path>-lock`) is also a different file or missing, the client SHALL open the new store before closing the open one and SHALL keep using the open one when the new one cannot be opened; when the lock file is the same, it SHALL close the open store first, since LMDB allows one environment per lock file in a process. Lookups from several threads at once SHALL each return a valid result. When a lookup runs in a process whose pid differs from the pid that opened the store (after `fork`), the client SHALL discard the inherited handle without closing it, open the store again, re-read the environment variables, and reset the round-robin counters; no lock that another thread of the parent held at the fork SHALL keep the child's lookup waiting. Each lookup SHALL end its read transaction before returning. When the store has grown past the size the client mapped it with (the daemon was restarted with a larger `map_size`), the client SHALL map it with the new size once no lookup in the process uses it, and read again. When the store file exists but cannot be opened, the client SHALL try to open it again at most once a second, and SHALL fail lookups in between with the error of the last attempt; a missing store file SHALL be tried on every lookup.

#### Scenario: Lookup after fork

- **WHEN** a process performs a successful lookup, forks, and the child performs a lookup of the same service
- **THEN** the child's lookup returns a valid address of that service, and the parent's next lookup also returns one

#### Scenario: Fork while other threads look up

- **WHEN** a process has 4 threads looking up a service in a loop, and another thread forks 300 times while they run
- **THEN** each child's lookup of that service returns within 5 seconds

#### Scenario: Store grown past the client's map size

- **WHEN** a client has looked up a service in a store mapped with 4 MiB, and another process then opens the store with a 16 MiB map and grows it past 4 MiB
- **THEN** the client's next lookups of that service return an address

#### Scenario: Store replaced while the client runs

- **WHEN** a client has looked up a service in a stale store, and the store path is then replaced by a fresh store whose record of that service names another backend
- **THEN** the first lookup in the same process made at least 100 ms after the previous check returns the backend of the new store

#### Scenario: Replacement that cannot be opened yet

- **WHEN** a client reads a stale store whose service is `on_all_down = "open"`, the daemon moves that file aside and removes its lock file, and the store path then holds a file that is not an LMDB store
- **THEN** lookups go on returning the backend from the open store, and once the path holds a valid store the client switches to it

#### Scenario: ALL_DOWN from a fresh store

- **WHEN** a lookup in a fresh store returns `ALL_DOWN`
- **THEN** the client does not check the store path for another file

#### Scenario: Concurrent lookups

- **WHEN** 8 threads each perform 1000 lookups of a fresh round-robin service with 2 healthy backends
- **THEN** every lookup returns one of the 2 addresses and none fails


<!-- @trace
source: emfor-clients
updated: 2026-11-12
code:
  - clients/c/example.c
  - clients/rust/src/capi.rs
  - tests/conformance/v1/sticky-v1.json
  - clients/rust/src/store.rs
  - clients/rust/README.md
  - clients/c/README.md
  - clients/rust/src/client.rs
  - clients/rust/examples/lookup.rs
  - tests/conformance/v1/cases.json
  - clients/c/Makefile
  - clients/rust/src/select.rs
  - tools/emfor-fixture/Cargo.toml
  - tools/emfor-fixture/src/lib.rs
  - tests/conformance/v1/README.md
  - tests/spike/probe_read.py
  - tests/spike/RESULTS.md
  - clients/rust/src/config.rs
  - tests/spike/probe/Cargo.toml
  - clients/rust/src/lib.rs
  - Cargo.toml
  - tests/spike/probe/cbindgen.toml
  - tools/emfor-fixture/src/main.rs
  - clients/rust/cbindgen.toml
  - tests/spike/probe_read.php
  - .github/workflows/ci.yml
  - clients/rust/Cargo.toml
  - tests/spike/probe_read.c
  - tests/spike/run.sh
  - tests/spike/probe/src/lib.rs
  - tests/spike/check_symbols.sh
tests:
  - clients/rust/tests/conformance_capi.rs
  - clients/rust/tests/fork.rs
  - clients/rust/tests/lock_format.rs
  - clients/c/tests/check_symbols.sh
  - clients/c/tests/test_fork.c
  - clients/rust/tests/env_default.rs
  - clients/rust/tests/env_invalid.rs
  - clients/rust/tests/threads.rs
  - clients/rust/tests/support/mod.rs
  - clients/c/tests/test_threads.c
  - clients/c/tests/common.h
  - clients/rust/tests/conformance.rs
  - clients/rust/tests/same_lmdb.rs
  - clients/c/tests/fixture.json
  - clients/rust/tests/env_retry.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - clients/c/tests/test_lookup.c
-->

---
### Requirement: C library shares the daemon's LMDB

The C library `libemfor` SHALL be built from the Rust client crate in the daemon's workspace, so that it reads the store with the same LMDB source and build options as the daemon. Its shared library SHALL export only symbols whose names start with `emfor_`. When the store's LMDB lock or data format differs from the library's, the lookup SHALL fail with `UNAVAILABLE` and a message telling the operator to install `libemfor` from the same emfor release as the daemon. No C function SHALL let a Rust panic unwind into the caller.

#### Scenario: Library from the same release reads the daemon's store

- **WHEN** a C program linked with `libemfor` through `pkg-config emfor` looks up a service in the store of a running daemon from the same workspace build
- **THEN** the lookup returns one of the service's addresses

#### Scenario: Only emfor symbols are exported

- **WHEN** the dynamic symbol table of the shared library is listed
- **THEN** every defined exported symbol starts with `emfor_`, so none starts with `mdb_`

#### Scenario: Another LMDB in the same process

- **WHEN** a process has already opened a different store through the system's LMDB library and then calls `emfor_lookup`
- **THEN** both stores remain readable and the lookup returns an address

#### Scenario: Store written by a different LMDB format

- **WHEN** the store is not in this library's LMDB format: a lock file created by an LMDB build with a different lock format (as with Debian's liblmdb 0.9.31, `MDB_VERSION_MISMATCH`), or a data file LMDB does not recognize (`MDB_INVALID`)
- **THEN** the lookup fails with `UNAVAILABLE`, and the message names the store path and says to install `libemfor` from the same emfor release as the daemon

#### Scenario: C status values

- **WHEN** the C function `emfor_lookup` ends in each outcome below
- **THEN** it returns the listed status, and `emfor_status_name` returns the listed name

##### Example: C status mapping

| Outcome | Status | `emfor_status_name` |
| --- | --- | --- |
| address written to `out` | `EMFOR_OK` (0) | `OK` |
| store cannot be opened | `EMFOR_UNAVAILABLE` (1) | `UNAVAILABLE` |
| `meta/schema_version` = `3` | `EMFOR_INCOMPATIBLE` (2) | `INCOMPATIBLE` |
| no such service | `EMFOR_NOT_FOUND` (3) | `NOT_FOUND` |
| closed service, all down | `EMFOR_ALL_DOWN` (4) | `ALL_DOWN` |
| `name` is NULL, or `out_len` is 8 for `10.0.0.11:8080` | `EMFOR_INVALID_ARGUMENT` (5) | `INVALID_ARGUMENT` |


<!-- @trace
source: emfor-clients
updated: 2026-11-12
code:
  - clients/c/example.c
  - clients/rust/src/capi.rs
  - tests/conformance/v1/sticky-v1.json
  - clients/rust/src/store.rs
  - clients/rust/README.md
  - clients/c/README.md
  - clients/rust/src/client.rs
  - clients/rust/examples/lookup.rs
  - tests/conformance/v1/cases.json
  - clients/c/Makefile
  - clients/rust/src/select.rs
  - tools/emfor-fixture/Cargo.toml
  - tools/emfor-fixture/src/lib.rs
  - tests/conformance/v1/README.md
  - tests/spike/probe_read.py
  - tests/spike/RESULTS.md
  - clients/rust/src/config.rs
  - tests/spike/probe/Cargo.toml
  - clients/rust/src/lib.rs
  - Cargo.toml
  - tests/spike/probe/cbindgen.toml
  - tools/emfor-fixture/src/main.rs
  - clients/rust/cbindgen.toml
  - tests/spike/probe_read.php
  - .github/workflows/ci.yml
  - clients/rust/Cargo.toml
  - tests/spike/probe_read.c
  - tests/spike/run.sh
  - tests/spike/probe/src/lib.rs
  - tests/spike/check_symbols.sh
tests:
  - clients/rust/tests/conformance_capi.rs
  - clients/rust/tests/fork.rs
  - clients/rust/tests/lock_format.rs
  - clients/c/tests/check_symbols.sh
  - clients/c/tests/test_fork.c
  - clients/rust/tests/env_default.rs
  - clients/rust/tests/env_invalid.rs
  - clients/rust/tests/threads.rs
  - clients/rust/tests/support/mod.rs
  - clients/c/tests/test_threads.c
  - clients/c/tests/common.h
  - clients/rust/tests/conformance.rs
  - clients/rust/tests/same_lmdb.rs
  - clients/c/tests/fixture.json
  - clients/rust/tests/env_retry.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - clients/c/tests/test_lookup.c
-->

---
### Requirement: Shared conformance data defines the contract

The repository SHALL contain conformance data under `tests/conformance/v1/`: `cases.json`, a list of cases each giving store contents and lookups with their expected address, expected set of addresses, expected count per address, or expected error code; and `sticky-v1.json`, the rendezvous hash and pick vectors. A fixture tool SHALL write a case's store contents to an LMDB file in the daemon's format. Every client interface (the Rust API, the C API, and the Go, Python, Perl and PHP wrappers) SHALL have a test runner that executes every case and every pick vector, and SHALL pass all of them; the Rust runner SHALL also check every hash vector, which no other interface exposes. The wrappers and the C library SHALL be checked by one shared driver, `emfor-conformance`, which writes the case's store anew just before each run of a small lookup program of the interface under test, once per lookup, with `EMFOR_STORE` and `EMFOR_MAX_STALENESS` set for that case. An expected error SHALL pass only with a non-empty message. A lookup with an expected count per address (`expect_counts`) SHALL run the sum of the counts as lookups in one process and SHALL pass only when every address appears exactly its count and no other result appears.

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

#### Scenario: Driver detects an error without a message

- **WHEN** a lookup expects `UNAVAILABLE` and the lookup program prints `ERR UNAVAILABLE` with no message
- **THEN** the driver reports that lookup as a failure

#### Scenario: Slow lookups do not turn a fresh store stale

- **WHEN** a case with a `fresh` heartbeat and a 500 ms staleness limit has 4 lookups, and each run of the lookup program takes 200 ms
- **THEN** every run reads a heartbeat less than 500 ms old

##### Example: lookup program protocol

| Item | Value |
| --- | --- |
| Invocation | `<program> <name> <affinity-key> <count>`; the key argument is `""` for no key |
| Output | `count` lines, one per lookup in the same process: `OK <address>` or `ERR <CODE> <message>` |
| Environment | `EMFOR_STORE` = the case's store path; `EMFOR_MAX_STALENESS` = the case's `max_staleness`, or `<max_staleness_ms>ms` |
| Exit status | `0` when every line was printed; any other status counts as a failure of every lookup in that run |
| Count | the length of `expect_all_of`, the sum of `expect_counts`, or 1 |


<!-- @trace
source: emfor-balance-fastest
updated: 2026-11-12
code:
  - crates/emfor-format/src/lib.rs
  - crates/emfor-daemon/src/select.rs
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/dns.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/config.rs
  - examples/emfor.toml
  - docs/deploy/dns-resolver.md
  - crates/emfor-daemon/src/checker/mod.rs
  - clients/rust/src/client.rs
  - .spectra.yaml
  - tests/conformance/v1/cases.json
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/schedule.rs
  - clients/rust/README.md
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Go, Python, Perl and PHP wrap libemfor

The repository SHALL provide lookup wrappers for Go, Python, Perl and PHP that call `emfor_lookup` in the shared library `libemfor` and add no lookup logic of their own: name normalization, staleness, candidates, balance and error decisions SHALL all come from `libemfor`. Each wrapper SHALL expose one lookup call taking a service name and an optional affinity key and returning the backend address as a string, and SHALL report failures in the language's idiom with the shared code name (`UNAVAILABLE`, `INCOMPATIBLE`, `NOT_FOUND`, `ALL_DOWN`) and the message from `emfor_last_error`, as listed below. An empty affinity key SHALL be passed to `libemfor` as no key. A name or key that `libemfor` cannot accept (one containing a NUL byte, or not valid UTF-8) SHALL be rejected with the language's argument error, not with one of the four codes. No wrapper SHALL add a configuration function; the store path and staleness come only from `EMFOR_STORE` and `EMFOR_MAX_STALENESS`.

The Python wrapper SHALL load the shared library from the path in the environment variable `EMFOR_LIBRARY` when it is set and non-empty, and otherwise by the name `emfor` through the system's library search; when neither loads, importing the package SHALL fail with `ImportError` whose message names `EMFOR_LIBRARY`. The Go, Perl and PHP wrappers SHALL link `libemfor` at build time through `pkg-config emfor`.

#### Scenario: Each wrapper returns an address

- **WHEN** each wrapper looks up `api.svc.emfor` in a fresh store where that round-robin service has the healthy backends `10.0.0.11:8080` and `10.0.0.12:8080`
- **THEN** it returns one of those two strings

#### Scenario: Each wrapper reports the shared code

- **WHEN** each wrapper looks up `nope.svc.emfor`, which the store does not contain
- **THEN** the failure carries the code name `NOT_FOUND` and a non-empty message, in the form listed below

##### Example: wrapper call and failure form

| Language | Call | Failure | Code name |
| --- | --- | --- | --- |
| Go | `emfor.Lookup(name, affinityKey string) (string, error)`; `""` is no key | returns `*emfor.Error` | `err.Code.String()`; the constants are `emfor.Unavailable`, `emfor.Incompatible`, `emfor.NotFound`, `emfor.AllDown` |
| Python | `emfor.lookup(name, affinity_key=None) -> str` | raises `emfor.UnavailableError`, `emfor.IncompatibleError`, `emfor.NotFoundError` or `emfor.AllDownError`, all subclasses of `emfor.Error` | `exc.code` |
| Perl | `Emfor::lookup($name, $affinity_key)`; `undef` is no key | dies with an `Emfor::Error` object that stringifies as `"CODE: message"` | `$err->code` |
| PHP | `Emfor\lookup(string $name, ?string $affinity_key = null): string` | throws `Emfor\LookupException` (extends `RuntimeException`) | `$e->errorCode` |

#### Scenario: Invalid argument

- **WHEN** a wrapper is called with the name `"api\0svc"` (a NUL byte inside)
- **THEN** it fails with the language's argument error and carries none of the four code names

##### Example: argument error by language

| Language | Argument error |
| --- | --- |
| Go | a non-nil `error` that is not an `*emfor.Error` |
| Python | `ValueError` |
| Perl | dies with a plain string starting `Emfor::lookup: invalid argument` |
| PHP | `ValueError` |

#### Scenario: Python cannot find the library

- **WHEN** the Python package is imported with `EMFOR_LIBRARY` set to a path that does not exist
- **THEN** the import raises `ImportError` whose message contains `EMFOR_LIBRARY`

#### Scenario: Lookup after fork in a wrapper

- **WHEN** a Python, Perl or PHP CLI process looks up `api.svc.emfor`, forks, and the child looks it up again
- **THEN** both lookups return one of the service's addresses

<!-- @trace
source: emfor-clients-2
updated: 2026-11-12
code:
  - clients/php/emfor_arginfo.h
  - clients/libemfor.mk
  - clients/python/README.md
  - clients/perl/lib/Emfor.pm
  - tests/conformance/v1/README.md
  - clients/go/go.mod
  - clients/perl/Makefile
  - .github/workflows/ci.yml
  - clients/perl/t/conformance_lookup.pl
  - clients/perl/README.md
  - clients/php/Makefile
  - tools/emfor-fixture/Cargo.toml
  - clients/php/config.m4
  - clients/go/README.md
  - clients/go/emfor.go
  - clients/php/emfor.c
  - clients/perl/lib/Emfor/Error.pm
  - clients/c/Makefile
  - clients/php/emfor.stub.php
  - clients/php/README.md
  - clients/python/emfor/__init__.py
  - clients/perl/Emfor.xs
  - clients/Makefile
  - tools/emfor-fixture/src/bin/emfor-conformance.rs
  - clients/go/Makefile
  - clients/perl/t/emfor.t
  - clients/go/cmd/emfor-conformance-lookup/main.go
  - clients/python/pyproject.toml
  - clients/python/Makefile
  - clients/perl/Makefile.PL
tests:
  - tools/emfor-fixture/tests/conformance_driver.rs
  - clients/python/tests/conformance_lookup.py
  - clients/php/tests/emfor_test.php
  - clients/go/emfor_test.go
  - clients/c/tests/conformance_lookup.c
  - clients/python/tests/test_emfor.py
  - clients/php/tests/conformance_lookup.php
-->

---
### Requirement: Unknown balance falls back to round-robin

When a service record's `balance` is a value this client does not know, the client SHALL choose as for `round-robin`, SHALL ignore the affinity key, and SHALL NOT fail the lookup with `INCOMPATIBLE` for that reason. Unknown fields in service and backend records, and unknown keys in the store, SHALL be ignored.

#### Scenario: Future balance value

- **WHEN** a fresh service record has `"balance": "some-future-method"` and healthy backends `10.0.0.11:8080` and `10.0.0.12:8080`, and one process looks it up twice with affinity key `user-42`
- **THEN** both lookups succeed and the two results are exactly the set {`10.0.0.11:8080`, `10.0.0.12:8080`}

#### Scenario: Unknown fields ignored

- **WHEN** a backend object carries an extra field `"zone_hint": "x"` and the store has an extra key `meta/future`
- **THEN** the lookup returns that backend as if the extra field and key were absent


<!-- @trace
source: emfor-balance-2
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/freebsd/files/emfor.in
-->

---
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


<!-- @trace
source: emfor-balance-2
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/freebsd/files/emfor.in
-->

---
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

<!-- @trace
source: emfor-balance-2
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/freebsd/files/emfor.in
-->

---
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

<!-- @trace
source: emfor-balance-fastest
updated: 2026-11-12
code:
  - crates/emfor-format/src/lib.rs
  - crates/emfor-daemon/src/select.rs
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/dns.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/config.rs
  - examples/emfor.toml
  - docs/deploy/dns-resolver.md
  - crates/emfor-daemon/src/checker/mod.rs
  - clients/rust/src/client.rs
  - .spectra.yaml
  - tests/conformance/v1/cases.json
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/schedule.rs
  - clients/rust/README.md
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Slow start in lookups

When the store is fresh, after the candidates are found and, for `locality` and `fastest`, narrowed to the preferred ones, the client SHALL compute each remaining candidate's step and effective weight as defined by the backend-selection requirement "Slow start effective weight", using the service's `slow_start_ms`, the backend's `recovered_at` and the client's current Unix time in milliseconds. If no candidate is ramping, the lookup SHALL choose exactly as without slow start. If at least one is and `balance` is `round-robin`, `weighted`, `locality`, `fastest`, `sticky` without an affinity key, or a value the client does not know, the client SHALL choose the first candidate, in order, whose cumulative effective weight exceeds the round-robin counter modulo the sum of the effective weights. `failover` and `sticky` with a non-empty affinity key SHALL choose as without slow start. When the store is stale, no candidate SHALL be ramping.

#### Scenario: Ramping backend gets a smaller share

- **WHEN** a fresh `round-robin` service has healthy `10.0.0.11:8080` (not ramping) and `10.0.0.12:8080` (`recovered_at` 25 seconds ago, `slow_start_ms` 100000, so step 3), and one process looks it up 13 times
- **THEN** `10.0.0.11:8080` is returned 10 times and `10.0.0.12:8080` 3 times

#### Scenario: Weighted ramping backend

- **WHEN** a fresh `weighted` service has healthy `10.0.0.11:8080` (weight 3, step 3) and `10.0.0.12:8080` (weight 1, not ramping), and one process looks it up 19 times
- **THEN** `10.0.0.11:8080` is returned 9 times and `10.0.0.12:8080` 10 times

#### Scenario: Slow start over

- **WHEN** a fresh `round-robin` service has healthy `10.0.0.11:8080` and `10.0.0.12:8080` whose `recovered_at` is 200 seconds ago with `slow_start_ms` 100000, and one process looks it up 4 times
- **THEN** each backend is returned twice

#### Scenario: Failover and sticky with a key ignore slow start

- **WHEN** a fresh `failover` service lists healthy `10.0.0.11:8080` (step 3) then `10.0.0.12:8080`
- **THEN** every lookup returns `10.0.0.11:8080`

#### Scenario: Stale store does not ramp

- **WHEN** a stale `round-robin` service with `on_all_down = "open"` has healthy `10.0.0.11:8080` (not ramping) and `10.0.0.12:8080` (step 3 by its `recovered_at`), and one process looks it up 4 times
- **THEN** each backend is returned twice

<!-- @trace
source: emfor-slow-start
updated: 2026-11-14
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/state.rs
  - tools/emfor-fixture/src/lib.rs
  - docs/manual.zh_TW/07-dns.md
  - tests/conformance/v1/README.md
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-format/src/rules.rs
  - tests/conformance/v1/cases.json
  - crates/emfor-format/src/lib.rs
  - docs/manual/07-dns.md
  - crates/emfor-daemon/src/select.rs
  - crates/emfor-daemon/src/writer.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/supervisor.rs
  - docs/manual.zh_TW/05-backend-selection.md
  - crates/emfor-daemon/src/metrics.rs
  - CHANGELOG.md
  - docs/manual/05-backend-selection.md
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/dns.rs
  - docs/manual.zh_TW/03-configuration.md
  - clients/rust/src/client.rs
-->

---
### Requirement: Sticky uses weighted rendezvous hashing

For a service with `balance = "sticky"` and a non-empty affinity key, the client SHALL give each candidate a score and return the candidate with the largest score; on equal scores the candidate earlier in config order SHALL win. For a candidate with weight `w`, the score SHALL be the largest of the values `h_0` to `h_(w-1)`, where each value is the first 4 bytes of an MD5 digest read as a big-endian unsigned 32-bit integer: `h_0` is computed over the key bytes followed by the id bytes, and `h_i` for `i` from 1 is computed over the key bytes, the id bytes, the byte `#` and the decimal digits of `i`, all UTF-8 with no other separator. Each candidate SHALL therefore receive a share of keys equal to its weight divided by the sum of the candidates' weights. When every candidate has weight 1, the result SHALL be the candidate whose `MD5(key bytes followed by id bytes)` value is largest. A weight above 100, which the daemon never writes, SHALL count as 100, and a weight of 0 SHALL count as 1, so a damaged store cannot make one lookup compute more than 100 values per candidate. Slow start SHALL NOT change the weights used here. With no affinity key, or an empty one, the client SHALL pick by round-robin as defined above.

#### Scenario: Same key gives the same backend

- **WHEN** a fresh sticky service is looked up repeatedly with the same affinity key and its candidates and weights do not change
- **THEN** every lookup returns the same address

#### Scenario: Equal weights keep today's picks

- **WHEN** a fresh sticky service has backends `10.0.0.11:8080`, `10.0.0.12:8080`, `10.0.0.13:8080`, all with weight 1
- **THEN** the hash values and picks are as listed

##### Example: rendezvous hash vectors

| Key | Id | Hash (u32) |
| --- | --- | --- |
| `user-42` | `10.0.0.11:8080` | 1022706018 |
| `user-42` | `10.0.0.12:8080` | 3018330109 |
| `user-42` | `10.0.0.13:8080` | 336531056 |
| `user-7` | `10.0.0.11:8080` | 4189533513 |
| `user-7` | `10.0.0.12:8080` | 1529156490 |
| `user-7` | `10.0.0.13:8080` | 3484999621 |

##### Example: rendezvous picks

| Key | Healthy backends | Result |
| --- | --- | --- |
| `user-42` | all three | `10.0.0.12:8080` |
| `user-42` | `.11`, `.13` (`.12` unhealthy) | `10.0.0.11:8080` |
| `user-7` | all three | `10.0.0.11:8080` |
| `user-7` | `.12`, `.13` (`.11` unhealthy) | `10.0.0.13:8080` |

#### Scenario: Weights change the share of keys

- **WHEN** a fresh sticky service has backends `10.0.0.11:8080`, `10.0.0.12:8080` and `10.0.0.13:8080` with the weights listed, all healthy
- **THEN** the virtual node values and picks are as listed

##### Example: weighted hash vectors

| Key | Bytes hashed after the key | Hash (u32) |
| --- | --- | --- |
| `user-42` | `10.0.0.11:8080#1` | 101315482 |
| `user-42` | `10.0.0.11:8080#2` | 3080192789 |
| `user-42` | `10.0.0.13:8080#1` | 491054110 |
| `user-7` | `10.0.0.13:8080#1` | 2987525892 |

##### Example: weighted picks

| Key | Weights of `.11`, `.12`, `.13` | Result |
| --- | --- | --- |
| `user-42` | 1, 1, 1 | `10.0.0.12:8080` |
| `user-42` | 3, 1, 1 | `10.0.0.11:8080` |
| `user-42` | 1, 1, 100 | `10.0.0.13:8080` |
| `user-7` | 1, 1, 1 | `10.0.0.11:8080` |
| `user-7` | 2, 2, 2 | `10.0.0.11:8080` |
| `user-7` | 1, 1, 100 | `10.0.0.13:8080` |

#### Scenario: Raising a weight only moves keys to that backend

- **WHEN** the keys `key-0` to `key-999` are looked up with weights 1, 1, 1 and again with weights 1, 1, 2 for `.11`, `.12`, `.13`
- **THEN** with weights 1, 1, 2 `.13` gets 499 of the keys, and each of the 151 keys whose backend changed now goes to `.13`

#### Scenario: Weights above 100 count as 100

- **WHEN** a fresh sticky service has backends `10.0.0.11:8080` with weight 100 and `10.0.0.12:8080` with the weight listed, both healthy, and is looked up with the key listed
- **THEN** the result is as listed and the lookup returns as quickly as with weight 100

##### Example: weights above 100

| Key | Weight of `.12` | Result |
| --- | --- | --- |
| `key-795` | 100 | `10.0.0.11:8080` |
| `key-795` | 101 | `10.0.0.11:8080` |
| `key-795` | 4294967295 | `10.0.0.11:8080` |

With the weight of `.12` read as 101 the value of `h_100` (4277241900) would beat `.11` (4257715895) and `.12` would be picked.

#### Scenario: Empty key falls back to round-robin

- **WHEN** a fresh sticky service with three healthy backends is looked up 3 times with affinity key `""`
- **THEN** the 3 results are exactly the set of its three addresses

<!-- @trace
source: emfor-weighted-sticky
updated: 2026-11-14
code:
  - docs/manual/03-configuration.md
  - tests/conformance/v1/cases.json
  - clients/rust/src/lib.rs
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual/05-backend-selection.md
  - clients/rust/src/select.rs
  - docs/manual.zh_TW/05-backend-selection.md
  - tests/conformance/v1/README.md
  - CHANGELOG.md
tests:
  - clients/rust/tests/conformance.rs
-->