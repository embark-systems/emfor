## MODIFIED Requirements

### Requirement: LMDB file layout

The daemon SHALL store state in an LMDB environment opened with `MDB_NOSUBDIR` at `[store] path` (default `/var/lib/emfor/state.mdb`), producing one data file and one lock file named `<path>-lock`. It SHALL use only the unnamed default database. Keys and values SHALL be UTF-8 strings. The data file SHALL be created with `[store] file_mode` (default `0644`) and the lock file with `[store] lock_file_mode` (default `0666`). The map size SHALL be `[store] map_size` (default `64MiB`). `[store] sync` SHALL be a boolean, default `false`: with `false` the environment SHALL be opened with `MDB_NOSYNC`, so a commit does not wait for the disk; with `true` it SHALL be opened without `MDB_NOSYNC`, so every commit is flushed to disk before it completes. Commits SHALL run outside the async worker threads, so a slow disk does not stall the DNS server, the metrics endpoint or the checks.

#### Scenario: Files created on first start

- **WHEN** the daemon starts with `path = "/tmp/t/state.mdb"` and no existing files
- **THEN** `/tmp/t/state.mdb` and `/tmp/t/state.mdb-lock` exist with modes 0644 and 0666

#### Scenario: Commits do not wait for the disk by default

- **WHEN** the daemon opens the store with no `sync` key in `[store]`
- **THEN** the environment flags include `MDB_NOSYNC`

##### Example: sync setting and environment flags

| `[store] sync` | `MDB_NOSYNC` set |
| --- | --- |
| absent | yes |
| `false` | yes |
| `true` | no |

## ADDED Requirements

### Requirement: Damaged store is replaced at start

When the daemon, while starting, opening the store or reading it before the first check fails with an LMDB error that means the file is damaged (`MDB_CORRUPTED`, `MDB_PAGE_NOTFOUND` or `MDB_INVALID`), it SHALL log a warning naming the store path and the error, rename the data file to `<path>.corrupt` (replacing an existing file of that name), delete `<path>-lock`, and create a new store at `<path>` as on a first start. Backend state history is lost with the damaged file. The writer lock (`<path>.writer`) SHALL be held throughout. Any other open error, and a damaged store found after start, SHALL keep their existing handling (exit 1 at start, a failed commit afterwards).

#### Scenario: Damaged store after a power loss

- **WHEN** `/tmp/t/state.mdb` exists but its meta pages are overwritten with zero bytes, and the daemon starts with `path = "/tmp/t/state.mdb"`
- **THEN** the daemon logs a warning containing `/tmp/t/state.mdb`, `/tmp/t/state.mdb.corrupt` holds the damaged file, and the new `/tmp/t/state.mdb` has a fresh heartbeat and a record for every configured service

#### Scenario: Another daemon's store is not replaced

- **WHEN** a daemon is running on `/tmp/t/state.mdb` and a second daemon starts with the same path
- **THEN** the second daemon exits 1 with the writer lock error and `/tmp/t/state.mdb.corrupt` does not exist
