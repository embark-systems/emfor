## Purpose

Client libraries let an application pick a backend for a service by reading the daemon's local state store directly, without a network call. This capability defines the lookup contract that every language binding implements identically, and the shared conformance data that proves it.

## ADDED Requirements

### Requirement: Lookup returns one backend address

Each client library SHALL provide a lookup operation `emfor_lookup(name, affinity_key?)` that takes a service name and an optional affinity key, reads the state store, and returns exactly one backend address on success. The address SHALL be the backend record's `id`: `ip:port` for IPv4 and `[ip]:port` for IPv6.

#### Scenario: IPv4 backend

- **WHEN** service `api.svc.emfor` has one healthy backend with id `10.0.0.11:8080` and the store is fresh
- **THEN** the lookup returns `10.0.0.11:8080`

#### Scenario: IPv6 backend

- **WHEN** service `v6.svc.emfor` has one healthy backend with id `[2001:db8::11]:8080` and the store is fresh
- **THEN** the lookup returns `[2001:db8::11]:8080`

### Requirement: Lookup failures use four shared error codes

A lookup that cannot return an address SHALL fail with exactly one of these codes, decided by the first step that fails, in this order:

1. `UNAVAILABLE` — the store file cannot be opened (missing, permission denied, LMDB lock or data format different from the client's, any other open error), the key `meta/schema_version` is absent, or `EMFOR_MAX_STALENESS` holds an invalid value.
2. `INCOMPATIBLE` — `meta/schema_version` is not `1`, `meta/heartbeat` is missing or not a decimal integer, or the service record cannot be parsed as a version 1 record.
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
| `svc/api.svc.emfor` has `"balance":"weighted"` | `INCOMPATIBLE` |
| no `svc/nope.svc.emfor` key, lookup `nope.svc.emfor` | `NOT_FOUND` |
| every backend `down`, `on_all_down` = `closed` | `ALL_DOWN` |

#### Scenario: Schema check wins over a missing service

- **WHEN** `meta/schema_version` is `2` and the looked-up name has no record
- **THEN** the lookup fails with `INCOMPATIBLE`, not `NOT_FOUND`

#### Scenario: Missing store is retried

- **WHEN** a lookup fails with `UNAVAILABLE` because the store file does not exist, and the daemon then creates the store
- **THEN** the next lookup in the same process opens the store and returns an address

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

### Requirement: Candidates follow health and on_all_down

When the store is fresh, the candidates SHALL be the backends whose `state` is `healthy`, in config order. When no backend is a candidate (all unhealthy, or the store is stale), a service with `on_all_down = "closed"` SHALL fail with `ALL_DOWN`, and a service with `on_all_down = "open"` SHALL use every backend, in config order, as a candidate. The lookup SHALL NOT tell the caller that the result came from an all-down service.

#### Scenario: Unhealthy backends are skipped

- **WHEN** a fresh failover service lists `10.0.1.5:5432` (`down`) then `10.0.1.6:5432` (`healthy`)
- **THEN** the lookup returns `10.0.1.6:5432`

#### Scenario: Open service with every backend down

- **WHEN** a fresh failover service with `on_all_down = "open"` lists `10.0.1.5:5432` (`down`) then `10.0.1.6:5432` (`unknown`)
- **THEN** the lookup returns `10.0.1.5:5432`

### Requirement: Round-robin spreads lookups across candidates

For a service with `balance = "round-robin"`, or with no `balance` field, the client SHALL keep one counter per service name per process, starting at a random value on the first lookup of that service, and SHALL return `candidates[counter % len]` and increment the counter atomically on each lookup. The affinity key SHALL be ignored. Concurrent lookups from different threads SHALL each take a distinct counter value.

#### Scenario: Consecutive lookups cover every candidate

- **WHEN** a fresh round-robin service has healthy backends `10.0.0.11:8080`, `10.0.0.12:8080` and `10.0.0.13:8080`, and one process looks it up 3 times in a row
- **THEN** the 3 results are exactly the set {`10.0.0.11:8080`, `10.0.0.12:8080`, `10.0.0.13:8080`}

#### Scenario: Unhealthy backend is never returned

- **WHEN** the same service has `10.0.0.12:8080` marked `down` and is looked up 4 times
- **THEN** every result is `10.0.0.11:8080` or `10.0.0.13:8080`, and each appears twice

### Requirement: Sticky uses rendezvous hashing version 1

For a service with `balance = "sticky"` and a non-empty affinity key, the client SHALL compute, for each candidate, `MD5(key bytes followed by id bytes)` over the UTF-8 bytes with no separator, read the first 4 bytes of the digest as a big-endian unsigned 32-bit integer, and return the candidate with the largest value. On equal values the candidate earlier in config order SHALL win. With no affinity key, or an empty one, the client SHALL pick by round-robin as defined above.

#### Scenario: Same key gives the same backend

- **WHEN** a fresh sticky service is looked up repeatedly with the same affinity key and its candidates do not change
- **THEN** every lookup returns the same address

#### Scenario: Hash values and picks

- **WHEN** a fresh sticky service has backends `10.0.0.11:8080`, `10.0.0.12:8080`, `10.0.0.13:8080`
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
| `user-42` | `.11`, `.13` (`.12` down) | `10.0.0.11:8080` |
| `user-7` | all three | `10.0.0.11:8080` |
| `user-7` | `.12`, `.13` (`.11` down) | `10.0.0.13:8080` |

#### Scenario: Empty key falls back to round-robin

- **WHEN** a fresh sticky service with three healthy backends is looked up 3 times with affinity key `""`
- **THEN** the 3 results are exactly the set of its three addresses

### Requirement: Failover returns the first candidate

For a service with `balance = "failover"`, the client SHALL return the first candidate in config order and SHALL ignore the affinity key.

#### Scenario: First healthy backend wins

- **WHEN** a fresh failover service lists `10.0.1.5:5432` (`healthy`) then `10.0.1.6:5432` (`healthy`), and is looked up with affinity key `user-42`
- **THEN** the lookup returns `10.0.1.5:5432`

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

### Requirement: Clients are safe across threads and fork

The client SHALL open the store read-only, at most once per process, and reuse it for later lookups. Lookups from several threads at once SHALL each return a valid result. When a lookup runs in a process whose pid differs from the pid that opened the store (after `fork`), the client SHALL discard the inherited handle without closing it, open the store again, re-read the environment variables, and reset the round-robin counters. Each lookup SHALL end its read transaction before returning.

#### Scenario: Lookup after fork

- **WHEN** a process performs a successful lookup, forks, and the child performs a lookup of the same service
- **THEN** the child's lookup returns a valid address of that service, and the parent's next lookup also returns one

#### Scenario: Concurrent lookups

- **WHEN** 8 threads each perform 1000 lookups of a fresh round-robin service with 2 healthy backends
- **THEN** every lookup returns one of the 2 addresses and none fails

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
| `meta/schema_version` = `2` | `EMFOR_INCOMPATIBLE` (2) | `INCOMPATIBLE` |
| no such service | `EMFOR_NOT_FOUND` (3) | `NOT_FOUND` |
| closed service, all down | `EMFOR_ALL_DOWN` (4) | `ALL_DOWN` |
| `name` is NULL, or `out_len` is 8 for `10.0.0.11:8080` | `EMFOR_INVALID_ARGUMENT` (5) | `INVALID_ARGUMENT` |

### Requirement: Shared conformance data defines the contract

The repository SHALL contain conformance data under `tests/conformance/v1/`: `cases.json`, a list of cases each giving store contents and lookups with their expected address, expected set of addresses, or expected error code; and `sticky-v1.json`, the rendezvous hash and pick vectors. A fixture tool SHALL write a case's store contents to an LMDB file in the daemon's format. Every client interface (the Rust API, the C API, and each later language wrapper) SHALL have a test runner that executes every case and vector, and SHALL pass all of them.

#### Scenario: Client passes the shared cases

- **WHEN** a client's conformance runner runs over `tests/conformance/v1/cases.json` and `sticky-v1.json`
- **THEN** every lookup result matches its expectation and the runner reports zero failures

#### Scenario: Data covers every requirement

- **WHEN** the cases are listed by the requirement they exercise
- **THEN** each requirement in this capability except thread and fork safety and the C library sharing the daemon's LMDB has at least one case, and the four error codes, the three balance methods, both `on_all_down` settings and all four heartbeat forms (fresh, stale, zero, missing) each appear in at least one case
