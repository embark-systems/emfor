## ADDED Requirements

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

## MODIFIED Requirements

### Requirement: Shared conformance data defines the contract

The repository SHALL contain conformance data under `tests/conformance/v1/`: `cases.json`, a list of cases each giving store contents and lookups with their expected address, expected set of addresses, or expected error code; and `sticky-v1.json`, the rendezvous hash and pick vectors. A fixture tool SHALL write a case's store contents to an LMDB file in the daemon's format. Every client interface (the Rust API, the C API, and the Go, Python, Perl and PHP wrappers) SHALL have a test runner that executes every case and every pick vector, and SHALL pass all of them; the Rust runner SHALL also check every hash vector, which no other interface exposes. The wrappers and the C library SHALL be checked by one shared driver, `emfor-conformance`, which writes each case's store and runs a small lookup program of the interface under test once per lookup, with `EMFOR_STORE` and `EMFOR_MAX_STALENESS` set for that case.

#### Scenario: Client passes the shared cases

- **WHEN** a client's conformance runner runs over `tests/conformance/v1/cases.json` and `sticky-v1.json`
- **THEN** every lookup result matches its expectation and the runner reports zero failures

#### Scenario: Data covers every requirement

- **WHEN** the cases are listed by the requirement they exercise
- **THEN** each requirement in this capability except thread and fork safety, the C library sharing the daemon's LMDB, and the language wrappers has at least one case, and the four error codes, the three balance methods, both `on_all_down` settings and all four heartbeat forms (fresh, stale, zero, missing) each appear in at least one case

#### Scenario: Driver detects a wrong answer

- **WHEN** `emfor-conformance` runs with a lookup program that always prints `OK 10.9.9.9:1`
- **THEN** it reports at least one failure and exits with a non-zero status

##### Example: lookup program protocol

| Item | Value |
| --- | --- |
| Invocation | `<program> <name> <affinity-key> <count>`; the key argument is `""` for no key |
| Output | `count` lines, one per lookup in the same process: `OK <address>` or `ERR <CODE> <message>` |
| Environment | `EMFOR_STORE` = the case's store path; `EMFOR_MAX_STALENESS` = the case's `max_staleness`, or `<max_staleness_ms>ms` |
| Exit status | `0` when every line was printed; any other status counts as a failure of every lookup in that run |
