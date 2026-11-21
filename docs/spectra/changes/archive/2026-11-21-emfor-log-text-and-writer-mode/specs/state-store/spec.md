## MODIFIED Requirements

### Requirement: Single writer per store

Before opening the store the daemon SHALL take an exclusive lock on the file `<path>.writer`. If the lock is already held, the daemon SHALL exit with status 1 and an error message naming that lock file. The daemon SHALL create that file with mode 0600 and SHALL set it to 0600 when it exists with another mode (a failure to do so SHALL be logged at `warn` level and SHALL NOT stop the start), so that a user who is not the daemon's cannot take the lock by opening the file.

#### Scenario: Second daemon refused

- **WHEN** a daemon is running with `[store] path = "/tmp/t/state.mdb"` and a second daemon starts with the same path
- **THEN** the second daemon exits 1 with an error naming `/tmp/t/state.mdb.writer`, and the first keeps running

#### Scenario: Writer lock file mode

- **WHEN** the daemon starts with a store path whose `.writer` file does not exist, or exists with mode 0644
- **THEN** `<path>.writer` has mode 0600 while the daemon runs
