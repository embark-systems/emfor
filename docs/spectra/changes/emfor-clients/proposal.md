## Why

The daemon publishes backend health to a local LMDB file, but applications still have no library to read it. Today they can only use the DNS server, which returns an IP without a port and cannot use `affinity_key`. The client libraries are the main way applications are meant to use emfor.

Before any client is written, two risks must be checked, because either would force a different store format:
- whether a client built against the platform's own liblmdb can open the daemon's file;
- whether PHP's `dba` extension can read it.

Doing both in this change, together with the shared lookup rules and test data, lets the four remaining languages follow in a second change with a fixed contract.

## What Changes

- **Compatibility spike (first task):**
  - A C program linked against the system liblmdb, and PHP `dba` with the `lmdb` handler, open the daemon's store read-only on macOS and in a Debian container. The results are recorded.
  - If either fails, work on this change stops. A separate change, `emfor-sqlite-store`, then moves the store to SQLite WAL.
- **Lookup contract** (new spec `client-lookup`):
  - `emfor_lookup(name, affinity_key?)` returns one backend as an `ip:port` string, the backend record's `id`.
  - Errors use four shared codes: `UNAVAILABLE`, `INCOMPATIBLE`, `NOT_FOUND`, `ALL_DOWN`.
  - A stale or zeroed heartbeat makes every backend unknown, and `on_all_down` decides.
  - The contract also covers the three `balance` methods (round-robin, sticky, failover), the sticky hash version 1, name normalization, configuration through `EMFOR_STORE` and `EMFOR_MAX_STALENESS`, fork safety, and thread safety.
- **Shared test data:**
  - `tests/conformance/v1/` holds JSON cases: store contents plus a query, mapped to the expected `ip:port` or error code. It also holds sticky hash vectors.
  - A small Rust tool, `emfor-fixture`, writes a case's store contents into an LMDB file.
- **C client** in `clients/c/`: `emfor.h` and `emfor.c` with embedded cJSON and MD5, linked to the system liblmdb, built by a Makefile into a static and a shared library.
- **Rust client** `emfor-client` in `clients/rust/`, a workspace member that reads with heed and uses the `emfor-format` types.
- Both clients pass every case in `tests/conformance/v1/`.

## Non-Goals

- Go, Python, Perl and PHP clients. They come in the next change, `emfor-clients-2`. The PHP `dba` check in the spike is only a compatibility test.
- Publishing packages to language registries or distributions.
- Any change to the store format or the daemon. If the spike fails, that work belongs to `emfor-sqlite-store`.
- Telling the caller that a returned backend was picked while all backends were down.
- A configuration API in code; only environment variables and defaults are supported.
- The `random`, `fastest`, `weighted` and `zone` balance methods.
- Running the FreeBSD part of the spike. It needs the manually started CI, which stays off for now.

## Capabilities

### New Capabilities

- `client-lookup`: how a client library looks up a backend from the local store. This covers the result format, error codes, staleness handling, balance methods, the sticky hash, configuration, fork and thread behavior, and the shared conformance data.

### Modified Capabilities

(none)

## Impact

- **New directories:**
  - clients/c/ (emfor.h, emfor.c, vendored cJSON and MD5, Makefile, tests);
  - clients/rust/ (crate emfor-client);
  - tests/conformance/v1/;
  - tools/emfor-fixture/ (crate emfor-fixture);
  - tests/spike/ (compatibility scripts and the recorded results).
- **Changed files:**
  - Cargo.toml (new workspace members);
  - docs/spectra/LANGUAGE.md, only if wording drifts.
- **New dependencies:**
  - Rust: md-5 (sticky hash) and heed, already used by the daemon.
  - C: the system liblmdb, found with pkg-config.
  - Spike container: Debian packages for liblmdb, php-cli and the PHP dba extension.
- **Unchanged:** the daemon (crates/emfor-daemon/) and the store format (crates/emfor-format/).
