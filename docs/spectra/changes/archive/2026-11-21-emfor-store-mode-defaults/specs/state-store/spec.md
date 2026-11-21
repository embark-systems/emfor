## MODIFIED Requirements

### Requirement: LMDB file layout

The daemon SHALL store state in an LMDB environment opened with `MDB_NOSUBDIR` at `[store] path` (default `/var/lib/emfor/state.mdb`), producing one data file and one lock file named `<path>-lock`. It SHALL use only the unnamed default database. Keys and values SHALL be UTF-8 strings. The data file SHALL have the mode `[store] file_mode` (default `0640`) and the lock file the mode `[store] lock_file_mode` (default `0660`): the daemon SHALL set these modes at every start, on files it creates and on files that already exist, so that a daemon restarted with other modes changes the modes of the existing files. The map size SHALL be `[store] map_size` (default `64MiB`), rounded up by config validation to a multiple of 64KiB so that it is a multiple of the system page size, as LMDB requires. `[store] sync` SHALL be a boolean, default `false`: with `false` the environment SHALL be opened with `MDB_NOSYNC`, so a commit does not wait for the disk; with `true` it SHALL be opened without `MDB_NOSYNC`, so every commit is flushed to disk before it completes. Commits SHALL run outside the async worker threads, so a slow disk does not stall the DNS server, the metrics endpoint or the checks.

#### Scenario: Files created on first start

- **WHEN** the daemon starts with `path = "/tmp/t/state.mdb"`, no `file_mode` or `lock_file_mode`, and no existing files
- **THEN** `/tmp/t/state.mdb` and `/tmp/t/state.mdb-lock` exist with modes 0640 and 0660

#### Scenario: Existing files get the modes at start

- **WHEN** `/tmp/t/state.mdb` and `/tmp/t/state.mdb-lock` exist with modes 0644 and 0666, and the daemon starts with `path = "/tmp/t/state.mdb"` and no `file_mode` or `lock_file_mode`
- **THEN** after the start the two files have modes 0640 and 0660

#### Scenario: Modes set explicitly

- **WHEN** the daemon starts with `file_mode = "0644"` and `lock_file_mode = "0666"`
- **THEN** the data file has mode 0644 and the lock file mode 0666

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

### Requirement: Store file mode warning

When `[store] file_mode` or `[store] lock_file_mode` has the bit that lets other users write (`0002`), `emfor check-config` SHALL write one warning line for each such option to standard error, and the daemon SHALL log one line at `warn` level for each such option at start. Each line SHALL tell the reader, in one line, four things: which option it is and its mode; what that mode lets any local user do (for `lock_file_mode`: write the lock file, which holds LMDB's reader table and the lock the daemon takes for every commit, so that any local user can stop the daemon's commits and make every client see a stale store; for `file_mode`: write the data file that the clients read, so that any local user can change what every lookup returns); what to set instead (the default of that option, `"0660"` for `lock_file_mode` and `"0640"` for `file_mode`, and that the users of the applications have to be in the group of the daemon); and where to read more (the section "Who can use the store" of the operations chapter of the manual). A line SHALL also say that the daemon sets the mode on the existing files at every start, so that the reader of `emfor check-config` knows that the value takes effect at the next start. This SHALL NOT be an error: the exit status of `emfor check-config` and the start of the daemon SHALL be the same as without the warning. The mode bit for other users to read SHALL NOT cause a warning. `[store]` settings are applied only at start, so a reload SHALL NOT repeat the warning.

#### Scenario: World-writable lock file

- **WHEN** the config has `lock_file_mode = "0666"` and an operator runs `emfor check-config`
- **THEN** standard error has one line that names `lock_file_mode` and `0666`, says that any local user can stop the daemon's commits, says to use `"0660"` with the application users in the group of the daemon, and names the section "Who can use the store"; standard output has `config ok: ...`, and the exit status is 0

#### Scenario: Daemon starts with a world-writable mode

- **WHEN** the daemon starts with `file_mode = "0666"`
- **THEN** it logs one `warn` line that names `file_mode` and `0666`, says that any local user can change the data file that the clients read, says to use `"0640"` with the application users in the group of the daemon, and names the section "Who can use the store"; the daemon runs

##### Example: which modes warn

| `file_mode` | `lock_file_mode` | Warning lines |
| --- | --- | --- |
| absent | absent | none |
| `"0640"` | `"0660"` | none |
| `"0644"` | `"0660"` | none |
| `"0644"` | `"0666"` | 1, for `lock_file_mode`, recommending `"0660"` |
| `"0666"` | `"0660"` | 1, for `file_mode`, recommending `"0640"` |
| `"0666"` | `"0666"` | 2, one for each option |
| `"0662"` | `"0640"` | 1, for `file_mode`, recommending `"0640"` |
