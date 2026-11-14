## MODIFIED Requirements

### Requirement: Damaged store is replaced at start

When the daemon, while starting, opening the store or reading it before the first check fails with an LMDB error that means the file is damaged (`MDB_CORRUPTED`, `MDB_PAGE_NOTFOUND` or `MDB_INVALID`), or finds right after opening it, before reading any data page, that the data file is shorter than (last page number + 1) × page size as recorded by LMDB (the warning then names the cause as `truncated: <length> bytes, LMDB records <expected> bytes`), it SHALL log a warning naming the store path and the error, rename the data file to `<path>.corrupt` (replacing an existing file of that name), delete `<path>-lock`, and create a new store at `<path>` as on a first start. Backend state history is lost with the damaged file. The writer lock (`<path>.writer`) SHALL be held throughout. Any other open error, and a damaged store found after start, SHALL keep their existing handling (exit 1 at start, a failed commit afterwards). A data file short only by pages nobody reads cannot be told apart from a truncated one without reading past its end, so it SHALL be replaced the same way. After every commit, holding LMDB's write lock, the daemon SHALL extend the data file with zero bytes to (last page number + 1) × page size when it is shorter, so that a store it wrote is never shorter than LMDB records (LMDB does not write pages that were allocated and freed again within one transaction, yet counts them in the last page number). It SHALL never shorten the file.

#### Scenario: Damaged store after a power loss

- **WHEN** `/tmp/t/state.mdb` exists but its meta pages are overwritten with zero bytes, and the daemon starts with `path = "/tmp/t/state.mdb"`
- **THEN** the daemon logs a warning containing `/tmp/t/state.mdb`, `/tmp/t/state.mdb.corrupt` holds the damaged file, and the new `/tmp/t/state.mdb` has a fresh heartbeat and a record for every configured service

#### Scenario: Another daemon's store is not replaced

- **WHEN** a daemon is running on `/tmp/t/state.mdb` and a second daemon starts with the same path
- **THEN** the second daemon exits 1 with the writer lock error and `/tmp/t/state.mdb.corrupt` does not exist

#### Scenario: Truncated store at start

- **WHEN** `/tmp/t/state.mdb` holds 200 records and is then cut short by its last page, and the daemon starts with `path = "/tmp/t/state.mdb"`
- **THEN** the daemon does not end with `SIGBUS`; it logs a warning containing `/tmp/t/state.mdb` and `truncated`, `/tmp/t/state.mdb.corrupt` holds the truncated file, and the new store accepts commits

#### Scenario: Commits keep the file as long as LMDB records

- **WHEN** a reader holds an old read transaction and the daemon commits, many times, transactions that each add several hundred records and delete them again
- **THEN** after every commit the data file is at least (last page number + 1) × page size long

#### Scenario: Store left short by pages nobody reads is set aside

- **WHEN** a store written by plain LMDB (as by an older daemon) was left shorter than LMDB records only by pages allocated and freed within one transaction, and the daemon starts with it
- **THEN** the daemon moves it to `<path>.corrupt` and starts with a new store that accepts commits
