## Why

The daemon publishes backend health to a local LMDB file, but applications still have no library to read it. Today they can only use the DNS server, which returns an IP without a port and cannot use `affinity_key`. The client libraries are the main way applications are meant to use emfor.

The first compatibility spike (task 1.1, tests/spike/RESULTS.md) showed that clients cannot use the system's LMDB:
- Debian 13's liblmdb 0.9.31 reads the data file, but the daemon's lock file has a different format, so every open fails with `MDB_VERSION_MISMATCH`;
- Homebrew's liblmdb 1.0.3 cannot read the 0.9 data file at all;
- PHP's `dba` either has no `lmdb` handler (Homebrew) or fails the same way (Debian).

So the clients must use the daemon's own LMDB. Writing the lookup once in Rust, in the same workspace as the daemon, and exporting it as a C library with cargo-c gives every language that exact LMDB and one implementation of the lookup rules. A second spike checks this build before any client work. This change also fixes the lookup rules and test data, so the four remaining languages can follow in a second change as thin wrappers.

## What Changes

- **Compatibility spikes:**
  - Spike 1 (done): system liblmdb and PHP `dba` cannot read the store. Results and the read-speed comparison with SQLite are in tests/spike/RESULTS.md.
  - Spike 2 (first open task): a minimal cargo-c library built with heed opens the running daemon's store from C (through `pkg-config`), Python (`ctypes`) and PHP (FFI, in a process that has also loaded `dba` with the system's LMDB), on macOS and in a Debian container. Its shared library exports no `mdb_*` symbols.
  - If spike 2 fails, work stops and the separate change `emfor-sqlite-store` moves the store to SQLite WAL.
- **Lookup contract** (new spec `client-lookup`):
  - `emfor_lookup(name, affinity_key?)` returns one backend as an `ip:port` string, the backend record's `id`.
  - Errors use four shared codes: `UNAVAILABLE`, `INCOMPATIBLE`, `NOT_FOUND`, `ALL_DOWN`.
  - A stale or zeroed heartbeat makes every backend unknown, and `on_all_down` decides.
  - The contract also covers the three `balance` methods (round-robin, sticky, failover), the sticky hash version 1, name normalization, configuration through `EMFOR_STORE` and `EMFOR_MAX_STALENESS`, fork and thread safety, and the C library sharing the daemon's LMDB.
- **Shared test data:**
  - `tests/conformance/v1/` holds JSON cases: store contents plus a query, mapped to the expected `ip:port` or error code. It also holds sticky hash vectors.
  - A small Rust tool, `emfor-fixture`, writes a case's store contents into an LMDB file.
- **Rust client** `emfor-client` in `clients/rust/`: a workspace member that reads with heed and uses the `emfor-format` types.
- **C library `libemfor`**, built from `emfor-client` with cargo-c (feature `capi`): header `emfor.h`, a shared library, a static library and a `pkg-config` file named `emfor`. It replaces the hand-written C client of the first plan (embedded cJSON and MD5, system liblmdb, Makefile).
- Both the Rust API and the C API pass every case in `tests/conformance/v1/`. C programs in `clients/c/` check linking, fork and threads.

## Non-Goals

- Go, Python, Perl and PHP wrappers. They come in the next change, `emfor-clients-2`, which also picks the PHP route (extension or FFI). The Python and PHP calls in spike 2 are only compatibility tests.
- Publishing packages to language registries or distributions. Installation is documented as `cargo cinstall`.
- Any change to the store format or the daemon. If spike 2 fails, that work belongs to `emfor-sqlite-store`.
- Telling the caller that a returned backend was picked while all backends were down.
- A configuration API in code; only environment variables and defaults are supported.
- The `random`, `fastest`, `weighted` and `zone` balance methods.
- Running the FreeBSD part of the spikes. It needs the manually started CI, which stays off for now.

## Capabilities

### New Capabilities

- `client-lookup`: how a client library looks up a backend from the local store. This covers the result format, error codes, staleness handling, balance methods, the sticky hash, configuration, fork and thread behavior, the C library's LMDB, and the shared conformance data.

### Modified Capabilities

(none)

## Impact

- **New directories:**
  - clients/rust/ (crate emfor-client, with the `capi` feature and cargo-c metadata);
  - clients/c/ (C test programs and an example, built against the installed `libemfor`; no library source);
  - tests/conformance/v1/;
  - tools/emfor-fixture/ (crate emfor-fixture);
  - tests/spike/ (spike scripts, the spike 2 probe crate and the recorded results).
- **Changed files:**
  - Cargo.toml (new workspace members);
  - .github/workflows/ci.yml (spike 2 in the `freebsd` job, not run);
  - docs/spectra/LANGUAGE.md and docs/spectra/config.yaml (already updated with the `libemfor` decision).
- **New dependencies:**
  - Rust: md-5 (sticky hash) and heed, already used by the daemon.
  - Build tool: cargo-c (`cargo install cargo-c`), which generates the header with cbindgen.
  - Spike: Python 3, and PHP CLI with FFI and the dba extension.
  - No system liblmdb.
- **Unchanged:** the daemon (crates/emfor-daemon/) and the store format (crates/emfor-format/).
