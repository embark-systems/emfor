# Client compatibility spike results

Run `tests/spike/run.sh`: it starts emfor with a throwaway store and checks
that C (the system liblmdb, through `pkg-config lmdb`) and PHP (the `dba`
`lmdb` handler) can open the store read-only. See design "Compatibility spike
before any client" in docs/spectra/changes/emfor-clients/design.md.

The daemon writes the store with heed 0.22, which bundles lmdb-master-sys
0.2.6: liblmdb 0.9.70 from the `mdb.master` branch (lock version 2, data
version 1).

## Results (2026-11-12)

| Platform | Packages | C check | PHP check |
| --- | --- | --- | --- |
| macOS 27.2 (Darwin 27.2.0, arm64) | Homebrew `lmdb` 1.0.3 (`mdb_version`: LMDB 1.0.3, Oct 21 2026), `php` 8.5.12 | FAIL: `MDB_VERSION_MISMATCH` | FAIL: dba has no `lmdb` handler (ndbm, cdb, cdb_make, inifile, flatfile only) |
| Debian 13.7 (container `rust:1-trixie`, arm64) | `liblmdb-dev`/`liblmdb0` 0.9.31-1+b2 (`mdb_version`: LMDB 0.9.31, July 10 2023), `php8.4-cli`/`php8.4-dba` 8.4.27-1~deb13u1 | FAIL: `MDB_VERSION_MISMATCH` | FAIL: `dba_open()` lmdb handler: `MDB_VERSION_MISMATCH` |
| FreeBSD | not run (CI job added, manual dispatch only) | | |

## Cause

Extra runs separated the lock file from the data file. In each, the daemon
was running, and the data file was copied to a new directory, where the
system liblmdb created a fresh lock file:

| Platform | Live store (daemon's lock file) | Copied data file, fresh lock file |
| --- | --- | --- |
| Debian, liblmdb 0.9.31 | `MDB_VERSION_MISMATCH` | opens; `meta/schema_version` = `1`, heartbeat read |
| macOS, liblmdb 1.0.3 | `MDB_VERSION_MISMATCH` | `MDB_INVALID: File is not an LMDB file` |

- **Debian:** the data format is compatible, but the lock file format is not.
  liblmdb's lock format word (`MDB_LOCK_FORMAT` in mdb.c) mixes the lock
  version with a description of the lock layout. The 0.9.70 `mdb.master`
  build in heed and the 0.9.31 release build disagree on it, so every reader
  fails while the daemon's lock file exists. The lock file stays behind after
  the daemon exits, so this is not limited to a running daemon.
- **macOS:** LMDB 1.0 changed the data file format, so it cannot read a file
  written by 0.9.x at all, and its lock format differs as well.
- **PHP:** Debian's dba `lmdb` handler links the same system liblmdb and fails
  the same way. Homebrew's PHP is built without the handler.

## Decision

Both exit triggers in the original "LMDB exit rule" fire for clients that use
the system's LMDB:

- Trigger 1 (PHP): Homebrew's `dba` has no `lmdb` handler, and Debian's cannot
  open the daemon's store.
- Trigger 2 (locks): the system liblmdb returns `MDB_VERSION_MISMATCH` on both
  platforms. The platforms ship different LMDB lines (0.9.31 and 1.0.3), and
  1.0.3 cannot read 0.9 data, so no single daemon build matches every
  distribution's library.

Instead of leaving LMDB, the clients stop using the system's LMDB
(2026-11-12). The Rust crate `emfor-client` reads the store through heed, in
the same workspace and `Cargo.lock` as the daemon, so it always builds the
daemon's own LMDB. cargo-c turns it into the C library `libemfor`, and every
other language wraps that shared library. The Rust `cdylib` exports only the
`emfor_*` functions, so its bundled `mdb_*` symbols cannot clash with another
LMDB in the same process. `libemfor` and `emfor` are released and upgraded
together. A second spike checks this build before any client work; SQLite
(change `emfor-sqlite-store`) is now the fallback only if that spike fails.

## Read speed, LMDB vs SQLite (2026-11-12)

To check that the choice is not a performance question, a throwaway benchmark
measured the lookup's read pattern: one read transaction reading
`meta/schema_version`, `meta/heartbeat` and one of 100 service records, with
a writer committing every 1 ms. LMDB used heed as the daemon does; SQLite used
rusqlite (bundled) in WAL mode with a read-only connection and one `SELECT`.
Times are per lookup, not including JSON parsing.

| Case | Linux (Debian 13 container on Apple M4 Max) | macOS (M4 Max) |
| --- | --- | --- |
| LMDB, thread-local reader slot, 1 process | 185 ns | 233 ns |
| LMDB, thread-local reader slot, 8 processes | 207 ns | 259 ns |
| LMDB, `MDB_NOTLS`, 1 process | 192 ns | 575 ns |
| LMDB, `MDB_NOTLS`, 8 processes | 1,700 ns | 21,058 ns |
| SQLite WAL, 1 process | 1,801 ns | 2,062 ns |
| SQLite WAL, 8 processes | 6,015 ns | 22,329 ns |
| SQLite open plus first read | 48 µs | 252 µs |

Both stores answer in microseconds, far below the cost of the connection the
application makes next. Clients keep `MDB_NOTLS`. Without it each thread keeps
its reader slot until the thread exits, and the slot table is sized once by
the daemon when it creates the lock file (126 by default, shared by every
process on the host), so many worker processes would run out of slots.
