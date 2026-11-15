## MODIFIED Requirements

### Requirement: Clients are safe across threads and fork

The client SHALL open the store read-only once per process and reuse it for later lookups. When a lookup finds the store stale, or fails with an error other than `ALL_DOWN` from a fresh store, the client SHALL check whether the file at the store path is now a different file (device and inode) than the open one, at most once every 100 ms in a process; a missing file SHALL NOT count as a different file. When it is different, the client SHALL switch to it once no lookup in the process uses the open store, and repeat the lookup once. When the lock file (`<path>-lock`) is also a different file or missing, the client SHALL open the new store before closing the open one and SHALL keep using the open one when the new one cannot be opened; when the lock file is the same, it SHALL close the open store first, since LMDB allows one environment per lock file in a process. Lookups from several threads at once SHALL each return a valid result. When a lookup runs in a process whose pid differs from the pid that opened the store (after `fork`), the client SHALL discard the inherited handle without closing it, open the store again, re-read the environment variables, and reset the round-robin counters; no lock that another thread of the parent held at the fork SHALL keep the child's lookup waiting. On FreeBSD, whose threading library keeps the process-shared mutexes of LMDB's lock file in a table that stays locked in the child when a thread of the parent was inside one at the fork, the client SHALL make `fork` wait until no lookup of the process is inside the store, SHALL keep lookups from entering it until `fork` has returned in the parent and in the child, and SHALL do the same for a first lookup that is opening the store; this holds for lookups through the C library, and so for the Go, Python, PHP and Perl clients, as for the Rust client. A `fork` from a signal handler that interrupted a lookup of the same thread SHALL return in the parent, even though the child is then not guarded. Each lookup SHALL end its read transaction before returning. When the store has grown past the size the client mapped it with (the daemon was restarted with a larger `map_size`), the client SHALL map it with the new size once no lookup in the process uses it, and read again. When the store file exists but cannot be opened, the client SHALL try to open it again at most once a second, and SHALL fail lookups in between with the error of the last attempt; a missing store file SHALL be tried on every lookup.

#### Scenario: Lookup after fork

- **WHEN** a process performs a successful lookup, forks, and the child performs a lookup of the same service
- **THEN** the child's lookup returns a valid address of that service, and the parent's next lookup also returns one

#### Scenario: Fork while other threads look up

- **WHEN** a process has 4 threads looking up a service in a loop, and another thread forks 300 times while they run
- **THEN** each child's lookup of that service returns within 5 seconds, on Linux and on FreeBSD

#### Scenario: Fork while other threads look up through the C library

- **WHEN** a process has 4 threads calling `emfor_lookup` for a service in a loop, and another thread forks 300 times while they run
- **THEN** each child's `emfor_lookup` of that service returns `EMFOR_OK` within 5 seconds

#### Scenario: Fork from a signal handler during a lookup

- **WHEN** a thread looks up a service in a loop for 3 seconds, and a signal handler running on that thread runs the fork handlers (prepare, then release) about every 2 milliseconds
- **THEN** every call returns, and the thread's lookups go on returning an address

#### Scenario: Fork during the first lookup

- **WHEN** a process has not looked up yet, one thread starts its first lookup, and another thread forks while that lookup opens the store
- **THEN** the child's lookup of the service returns an address within 5 seconds, and so does the lookup of the parent

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
