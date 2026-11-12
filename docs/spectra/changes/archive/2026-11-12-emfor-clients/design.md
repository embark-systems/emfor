## Context

The daemon writes health state to a single-file LMDB store; its format is defined in docs/spectra/specs/state-store/spec.md and crates/emfor-format/src/lib.rs.
- **Keys:** `meta/schema_version` = `1`; `meta/heartbeat` = Unix milliseconds, written at least every second and set to `0` on clean shutdown; `meta/started_at`; and one JSON `ServiceRecord` per `svc/<name>`.
- **Record fields:** `name`, `on_all_down`, `balance`, `all_down`, and `backends` in config order. Each backend has `id`, `ip`, `port`, `state`, `checked_at`, `reason`.
- **File lifetime:** the daemon keeps the same file across restarts and never truncates it (`Store::open` in crates/emfor-daemon/src/store.rs). On start it rewrites every `svc/*` key, so a reader that keeps the environment open sees the new run.

**How the daemon builds LMDB** (heed 0.22 bundles lmdb-master-sys 0.2.6, measured on 2026-11-12):
- liblmdb 0.9.70 from the `mdb.master` branch, with `MDB_DEVEL 0`, lock format version 2 and data version 1;
- lmdb-master-sys's build.rs sets only `MDB_IDL_LOGN` by default; the `posix-sem` feature is off, so each platform uses LMDB's default lock;
- a lock file carries a format word (`MDB_LOCK_FORMAT` in mdb.c) that mixes the lock version with the lock layout. A reader whose word differs gets `MDB_VERSION_MISMATCH`; it does not corrupt anything.
- The daemon does not set the reader table size, so it is LMDB's default of 126 slots, fixed when the daemon creates the lock file and shared by every process on the host.

The daemon's DNS path already picks backends (crates/emfor-daemon/src/select.rs) for round-robin and failover. It never computes the sticky hash, because DNS has no affinity key. Terms follow docs/spectra/LANGUAGE.md: 查詢 (`emfor_lookup`), 錯誤碼, 黏著鍵, 分配方式, 心跳, `libemfor`.

## Goals / Non-Goals

**Goals:**

- Confirm, before writing the client, that a cargo-c library built in this workspace reads the daemon's store from C, Python and PHP.
- Define one lookup contract that every language implements identically, checked by shared test data.
- Ship the Rust client and the C library `libemfor` built from it, both passing all of that data.

**Non-Goals:**

- Go, Python, Perl and PHP wrappers (next change, `emfor-clients-2`), including the choice between a PHP extension and FFI.
- Store format or daemon changes, packaging beyond `cargo cinstall`, a configuration API in code, a degraded-result signal to callers, and balance methods beyond the first three.
- The FreeBSD run of the spikes, which waits until the manually started CI is run.

## Decisions

### Compatibility spike before any client

**Spike 1 (task 1.1, done 2026-11-12).** A C program linked with the system liblmdb, and PHP `dba` with the `lmdb` handler, tried to open the running daemon's store read-only. Results are in tests/spike/RESULTS.md:

| Platform | C (system liblmdb) | PHP `dba` |
| --- | --- | --- |
| macOS, Homebrew lmdb 1.0.3, PHP 8.5.12 | `MDB_VERSION_MISMATCH` | no `lmdb` handler |
| Debian 13, liblmdb 0.9.31, php8.4-dba | `MDB_VERSION_MISMATCH` | `MDB_VERSION_MISMATCH` |

On Debian, a copy of the data file with a fresh lock file opened fine, so only the lock format differs. Homebrew's 1.0.3 cannot read 0.9 data at all (`MDB_INVALID`). No daemon build can match every distribution's library, so this plan was replaced (see "One Rust core, exported to C with cargo-c").

**Spike 2 (task 1.2).** A minimal crate, tests/spike/probe/, is a workspace member so it shares the daemon's `Cargo.lock` and LMDB. It uses heed, opens the store with `MDB_RDONLY | MDB_NOSUBDIR | MDB_NOTLS`, and exports `probe_heartbeat(path)`, which returns the heartbeat or a negative LMDB error code. It is built with `cargo cbuild` and installed with `cargo cinstall` into a temporary prefix. Each check runs against a running daemon:

| Check | Passes when |
| --- | --- |
| C, compiled with `pkg-config --cflags --libs` for the probe | reads `meta/heartbeat` three times 1s apart, and the value increases |
| Python 3, `ctypes.CDLL` on the shared library | reads a non-zero heartbeat |
| PHP CLI, `FFI::cdef` on the shared library, after `dba_open` of a separate store made by the system's LMDB (Debian only; Homebrew PHP has no `lmdb` handler) | both reads succeed in the same process |
| Exported symbols (`nm -D --defined-only` on Linux, `nm -gU` on macOS) | no exported symbol starts with `mdb_`, and `probe_heartbeat` is exported |

It runs on macOS and in a Debian container. Results are added to tests/spike/RESULTS.md. FreeBSD is added to the CI `freebsd` job but is not run in this change.

### LMDB exit rule

**Supersedes** the first rule (2026-11-12), which left LMDB when the system's liblmdb or PHP `dba` could not read the store. Spike 1 hit both of those conditions, but they no longer apply, because clients no longer use the system's LMDB.

Leaving LMDB is now decided only from spike 2:
- **Trigger 1:** C or Python, through the cargo-c library, cannot open the running daemon's store on macOS or Debian.
- **Trigger 2:** the shared library exports `mdb_*` symbols that cannot be hidden, or the PHP same-process check fails.
- **Not reasons:** performance (tests/spike/RESULTS.md measured LMDB at 0.2 to 1.7 µs per lookup with `MDB_NOTLS` on Linux, and SQLite WAL at 1.8 to 6 µs), or the choice between a PHP extension and FFI.

If a trigger fires, work on this change stops after the spike and the results are reported. A separate change, `emfor-sqlite-store`, then:
- moves the store to SQLite in WAL mode, with table `kv(key TEXT PRIMARY KEY, value TEXT)`, the same keys and JSON, one transaction per writer batch, and read-only clients;
- keeps the `-wal` and `-shm` files after the daemon exits (persistent WAL), and gives readers write access to `-shm`, as they have to the LMDB lock file today;
- sets `schema_version` to 2;
- gives the daemon a SQLite sink in place of crates/emfor-daemon/src/store.rs.

No client exists yet, so there is nothing to migrate. Afterwards this change resumes with the store read through SQLite.

### One Rust core, exported to C with cargo-c

**Supersedes** the hand-written C client of the first plan (clients/c/ with emfor.c, embedded cJSON and MD5, linked with the system liblmdb, built by a Makefile). Spike 1 showed the system liblmdb cannot open the daemon's store.

- The crate `emfor-client` (clients/rust/) holds the only implementation of the lookup. It is a member of the root workspace, so it builds with the same `Cargo.lock`, the same heed and the same lmdb-master-sys as the daemon. This keeps the lock format identical without copying LMDB sources.
- Its feature `capi` adds the C functions in "C library interface". cargo-c builds, from the same crate, `libemfor.so` (`libemfor.dylib` on macOS), `libemfor.a`, the header `emfor.h` and `emfor.pc`. `[package.metadata.capi]` sets the library name `emfor` and the header name.
- The shared library exports only the functions marked `#[no_mangle] extern "C"`, so the `mdb_*` functions of the bundled LMDB stay private. A process that also loads another LMDB, such as PHP with `dba`, therefore uses each copy for its own files. Wrappers in other languages SHALL link the shared library; the static library exposes every symbol and is for C programs that link nothing else using LMDB.
- **Version rule:** `libemfor` takes the workspace version, and `libemfor` and `emfor` are released and upgraded together. A mismatch makes the open fail with `MDB_VERSION_MISMATCH` or `MDB_INVALID`, which maps to `UNAVAILABLE` (see "Lookup order and error codes").

### Lookup order and error codes

`emfor_lookup(name, affinity_key?)` runs these steps in order. The first one that ends the lookup decides the result.

1. **Open the store** (the first time, or after a fork; see "Opening, fork and threads").
   - The file is missing, permission is denied, or any other open error → `UNAVAILABLE`.
   - The open returns `MDB_VERSION_MISMATCH` or `MDB_INVALID` → `UNAVAILABLE`, with the message: `LMDB format of <path> differs from this emfor client (<LMDB error text>); install libemfor from the same emfor release as the daemon`. The same text serves the Rust crate and `libemfor`, which share the code.
   - `EMFOR_MAX_STALENESS` is invalid → `UNAVAILABLE`.
2. **Read the metadata** in one read-only transaction, which steps 3 to 6 also use.
   - `meta/schema_version` is missing → `UNAVAILABLE`: the daemon has not initialized the store.
   - Any value other than `1` → `INCOMPATIBLE`.
   - `meta/heartbeat` is missing or not a decimal integer → `INCOMPATIBLE`.
3. **Find the service.** Normalize the name: ASCII-lowercase it and remove one trailing dot, as the daemon does. Then read `svc/<name>`.
   - The key is missing → `NOT_FOUND`.
   - The value does not parse as a version 1 record (unknown `balance`, missing fields, bad JSON) → `INCOMPATIBLE`.
4. **Check staleness.** The store is stale when:
   - the heartbeat is `0`; or
   - `now_ms - heartbeat > max_staleness_ms`, where `now_ms` is the reader's wall clock in Unix milliseconds. A heartbeat in the future is fresh.
5. **Choose the candidates.**
   - The store is fresh → the backends with `state == "healthy"`.
   - The store is stale, or no backend is healthy → none are healthy.
   - With none healthy:
     - `on_all_down == "closed"` → `ALL_DOWN`;
     - `"open"` → every backend, in config order, is a candidate.
   - A service with no backends at all has no candidate, so it gives `ALL_DOWN` whatever its `on_all_down`. The daemon never writes such a record (config requires at least one backend); a hand-made store can.
6. **Pick one candidate by `balance`** and return its `id`.

| Code | Meaning |
| --- | --- |
| `UNAVAILABLE` | The store cannot be used: it cannot be opened, its LMDB format differs from this `libemfor`, it is not initialized, or the client's environment setting is invalid. |
| `INCOMPATIBLE` | The store exists but its format is not version 1, or a value cannot be parsed. |
| `NOT_FOUND` | No service has that name. |
| `ALL_DOWN` | No healthy backend, and `on_all_down = "closed"`. |

A stale heartbeat has no code of its own. It makes every backend unknown, and `on_all_down` decides (the user chose this on 2026-11-12). Callers are not told when a backend was picked from an all-down service.

### Result format

The result is the backend record's `id`: `10.0.0.11:8080`, or `[2001:db8::11]:8080` for IPv6. This is the normalized backend string from the config. It carries the port because backends of one service may use different ports. DNS still returns only the IP; DNS SRV is planned for later.

### Balance in the client

`balance` comes from the record; a missing `balance` field reads as `round-robin`. An empty `affinity_key` counts as no key.
- **round-robin:** one counter per service name per process, starting at a random value the first time that service is looked up. Each lookup takes `counter.fetch_add(1)` and picks `candidates[value % len]`. The counter is atomic, so concurrent threads spread across candidates. The key is ignored.
- **sticky, with a key:** rendezvous hashing version 1.
  - For each candidate, compute `MD5(key_bytes ++ id_bytes)`, the UTF-8 bytes of the key followed by the UTF-8 bytes of the `id`, with no separator.
  - Read the first 4 bytes as a big-endian `u32`.
  - Pick the candidate with the largest value. On a tie, pick the one earlier in config order.
- **sticky, without a key:** round-robin, using the same counter.
- **failover:** the first candidate in config order. The key is ignored.

### Configuration through the environment

Only environment variables are read, once, when the process first opens the store; the values apply until the next fork.
- **`EMFOR_STORE`:** path of the store file. Default `/var/lib/emfor/state.mdb`.
- **`EMFOR_MAX_STALENESS`:** a positive integer followed by `ms` or `s`, such as `3s` or `1500ms`. Default `3s`.
  - Anything else makes every lookup fail with `UNAVAILABLE`, with a message naming the variable.
  - A wrong value is never silently replaced by the default.

There is no configuration function. A test entry point that takes the path and staleness directly exists behind the cargo feature `testing`, for the crate's own tests and the conformance runners; it is not part of the public Rust or C API.

### Opening, fork and threads

**Opening:**
- The environment is opened with `MDB_RDONLY | MDB_NOSUBDIR | MDB_NOTLS`, once per process, and reused.
- `MDB_NOTLS` lets read transactions be created on any thread, and returns the reader slot when each transaction ends. Without it, every thread that ever looked up would keep a slot until it exits. The table has 126 slots for the whole host (see Context), so many worker processes could run out. With `MDB_NOTLS`, each lookup takes a shared lock to claim a slot. On Linux this costs about 1.7 µs per lookup with 8 processes reading at once (tests/spike/RESULTS.md), which is acceptable.
- A failed open is not remembered: the next lookup tries again, so a client started before the daemon recovers by itself.
- Before calling LMDB, the client checks that the store file exists. LMDB would otherwise create `<path>-lock` even when the data file is missing, and a lock file owned by an application user could stop the daemon from setting its modes when it starts.

**Fork:** each lookup compares the current pid with the pid that opened the environment. If they differ:
- the inherited handle is dropped without closing it, because LMDB forbids using it after a fork;
- the store is opened again;
- the environment variables are read again;
- the round-robin counters start afresh.

**Threads:**
- `emfor_lookup` may be called from many threads at once.
- Opening is guarded by a mutex.
- Each lookup uses its own read transaction and ends it before returning.
- Counters are atomics in a map guarded by a mutex.

### Shared conformance data

tests/conformance/v1/ is the contract that every client must pass, now and in later languages.

`cases.json` is an array of cases:

```json
{
  "case": "closed service with stale heartbeat",
  "store": {
    "schema_version": "1",
    "heartbeat": "stale",
    "services": [{"name":"db.svc.emfor","on_all_down":"closed","balance":"failover","all_down":false,
                  "backends":[{"id":"10.0.1.5:5432","ip":"10.0.1.5","port":5432,"state":"healthy","checked_at":1,"reason":null}]}],
    "raw": {}
  },
  "max_staleness_ms": 3000,
  "lookups": [
    {"name": "DB.svc.emfor.", "expect_error": "ALL_DOWN"}
  ]
}
```

Fields:
- **`heartbeat`** is one of:
  - `"fresh"`: the time the fixture is written;
  - `"stale"`: that time minus `2 × max_staleness_ms`;
  - `"zero"`;
  - `"missing"`.
- **`max_staleness`** (optional) is a raw `EMFOR_MAX_STALENESS` value. When present, the runner passes it through the client's own parser instead of using `max_staleness_ms`, so invalid values are checked through the shared data.
- **`schema_version`** may be omitted, so the key is absent.
- **`raw`** holds extra keys written verbatim, for malformed records.
- **Each lookup** has `name`, an optional `affinity_key`, and exactly one of:
  - `expect`: one `ip:port`;
  - `expect_error`: one of the four codes;
  - `expect_all_of`: a list of `ip:port`. The runner calls the lookup that many times in a row and the results, as a set, must equal the list. This is how round-robin is checked without fixing its start.

`sticky-v1.json` lists hash vectors `{key, id, hash}`, where `hash` is the big-endian `u32`, plus pick vectors `{key, ids, expect}`. The vectors are generated once with Python's hashlib during apply and committed.

A case that needs a missing store (`UNAVAILABLE`) uses `"store": null`: the runner points the client at a path that does not exist.

Two runners use this data in this change: one through the Rust API, and one that calls the C API functions of the `capi` feature from Rust. Wrappers in emfor-clients-2 add their own runners.

### Fixture writer

`emfor-fixture` lives at tools/emfor-fixture/, a workspace member with a binary target and a library. It takes a case's `store` object (`emfor-fixture <case.json> <index> <out.mdb>`) and writes an LMDB file with the same flags the daemon uses (`MDB_NOSUBDIR`, default database). It uses heed from the same workspace, so the files carry the daemon's LMDB format. The runners call its library once per case.

### C library interface

`emfor.h`, generated by cargo-c from the `capi` feature of `emfor-client`:

```c
typedef enum {
    EMFOR_OK = 0,
    EMFOR_UNAVAILABLE = 1,
    EMFOR_INCOMPATIBLE = 2,
    EMFOR_NOT_FOUND = 3,
    EMFOR_ALL_DOWN = 4,
    EMFOR_INVALID_ARGUMENT = 5   /* C only: NULL name, invalid UTF-8, or buffer too small */
} emfor_status;

#define EMFOR_ADDR_MAX 64   /* enough for "[IPv6]:port" plus NUL */

emfor_status emfor_lookup(const char *name, const char *affinity_key,
                          char *out, size_t out_len);
const char *emfor_status_name(int status); /* "UNAVAILABLE", ...; "UNKNOWN" for other values */
const char *emfor_last_error(void); /* thread-local detail of the last failure on this thread */
```

- `affinity_key` may be NULL.
- On `EMFOR_OK`, `out` holds the NUL-terminated `ip:port`.
- Each exported function runs its body inside `std::panic::catch_unwind`. A panic becomes `EMFOR_UNAVAILABLE` with the message `internal error: <panic message>`, and never unwinds into C.
- `emfor_status_name` takes an `int`, not `emfor_status`: C may store any integer in an enum variable, and an out-of-range value received as a Rust enum is undefined behavior. Passing an `emfor_status` to it needs no cast.
- The strings from `emfor_status_name` are static. The string from `emfor_last_error` stays valid until the next call on the same thread.
- C programs build with `pkg-config --cflags --libs emfor` after `cargo cinstall --release --features capi --prefix <prefix>` in clients/rust/.

### Rust client interface

The crate `emfor-client` lives at clients/rust/:

```rust
pub fn lookup(name: &str, affinity_key: Option<&str>) -> Result<String, Error>;

pub struct Error { pub code: ErrorCode, pub message: String }
pub enum ErrorCode { Unavailable, Incompatible, NotFound, AllDown }
// ErrorCode::as_str() -> "UNAVAILABLE" | "INCOMPATIBLE" | "NOT_FOUND" | "ALL_DOWN"
```

- It reads the store through `lmdb-master-sys`, the LMDB binding inside heed, rather than heed itself. heed keeps a process-wide list of open files and removes a file only when its environment is closed, but a forked child must never close the environment it inherited, so with heed the child could never open the store again. Cargo.lock holds one `lmdb-master-sys`, shared with the daemon through heed; the test clients/rust/tests/same_lmdb.rs fails if a second version appears.
- It parses records with `emfor_format::ServiceRecord`.
- It uses `md-5` for the sticky hash.
- The test entry point is behind the cargo feature `testing`; the C API is behind `capi`.

## Implementation Contract

**Behavior**
- With the daemon running and service `api.svc.emfor` healthy on `10.0.0.11:8080` and `10.0.0.12:8080`, both the Rust API and a C program linked with `libemfor` return the two `id`s in alternation. Called with the same `affinity_key` on a sticky service, they return the same `id` every time.
- After the daemon stops, a lookup for a service with `on_all_down = "open"` still returns a backend; for one with `on_all_down = "closed"`, it fails with `ALL_DOWN`. This holds within `max_staleness` (3s) of the stop, at once if the stop zeroed the heartbeat.
- A process that calls the lookup, forks, and calls it again in the child gets a valid answer in both.
- A store written by a different LMDB format makes the lookup fail with `UNAVAILABLE` and the version message from "Lookup order and error codes".

**Interfaces**
- C: `emfor_lookup`, `emfor_status_name`, `emfor_last_error`, `EMFOR_ADDR_MAX`, and the `emfor_status` values above, from `emfor.h` and `pkg-config` package `emfor`.
- Rust: `emfor_client::lookup`, `Error`, `ErrorCode`; features `capi` and `testing`.
- Environment: `EMFOR_STORE`, `EMFOR_MAX_STALENESS`.
- Data: tests/conformance/v1/cases.json and sticky-v1.json, in the shapes above. tests/spike/RESULTS.md records both spikes.

**Failure modes**
- Every failure is one of the four codes (plus `EMFOR_INVALID_ARGUMENT` in C), with a message naming the cause.
- No lookup panics into the caller or aborts on malformed store content.
- A missing store is retried on the next lookup.

**Acceptance criteria**
- tests/spike/RESULTS.md shows every spike 2 check passing on macOS and Debian. Otherwise the change stops as described in "LMDB exit rule".
- `cargo test --workspace --all-features` passes, including both conformance runners over every case and vector.
- `make -C clients/c test` passes on macOS and in the Debian container against a `cargo cinstall` prefix, including a fork test, a threads test and the exported-symbol check.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` and `cargo fmt --check` pass. The C test programs compile with `-Wall -Wextra -Werror`.

**Scope boundaries**

In scope: both spikes, the `client-lookup` spec, the conformance data, `emfor-fixture`, the Rust client with its C API, the C test programs, and their docs. Out of scope: everything under Non-Goals, and any edit to crates/emfor-daemon/ or crates/emfor-format/.

## Risks / Trade-offs

- **`libemfor` and the daemon must come from the same release.** A heed upgrade that changes the LMDB lock or data format makes every older `libemfor` fail with `UNAVAILABLE`. → The version rule in "One Rust core, exported to C with cargo-c"; the error message tells the operator what to do; a heed upgrade is reviewed as a format change.
- **Two copies of the Rust runtime.** A process that loads `libemfor` and another Rust `cdylib` holds two Rust standard libraries. → Harmless apart from size; documented.
- **The static library exports every symbol**, including LMDB's. → Documented: wrappers use the shared library, and only C programs with no other LMDB link `libemfor.a`.
- **Clock skew between reader and daemon.** Both run on the same host, so they share one clock. A clock jump backwards makes the heartbeat look like it is in the future, and the store stays fresh for up to that jump. → Accepted. A jump of more than `max_staleness` is rare, and the daemon updates the heartbeat every second.
- **Reader slots.** The 126-slot table is shared by every process on the host. With `MDB_NOTLS`, a slot is held only during a lookup, so this many lookups would have to run at the same instant. → Accepted. Raising the table size would need a daemon setting, which is out of scope.
- **Round-robin is per process.** Many short-lived processes each start at a random point, so the load is only spread statistically. → This is the documented behavior in LANGUAGE.md.
- **Deleting and recreating the store file while clients run** leaves them reading the old, unlinked file, which looks stale. That falls back to `on_all_down`. → Documented: restart applications after deleting the store. The daemon itself never deletes the file.

## Migration Plan

Additive: new libraries and test data. No existing behavior changes.

## Open Questions

None.
