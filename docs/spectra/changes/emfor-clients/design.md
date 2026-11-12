## Context

The daemon writes health state to a single-file LMDB store; its format is defined in docs/spectra/specs/state-store/spec.md and crates/emfor-format/src/lib.rs.
- **Keys:** `meta/schema_version` = `1`; `meta/heartbeat` = Unix milliseconds, written at least every second and set to `0` on clean shutdown; `meta/started_at`; and one JSON `ServiceRecord` per `svc/<name>`.
- **Record fields:** `name`, `on_all_down`, `balance`, `all_down`, and `backends` in config order. Each backend has `id`, `ip`, `port`, `state`, `checked_at`, `reason`.
- **File lifetime:** the daemon keeps the same file across restarts and never truncates it (`Store::open` in crates/emfor-daemon/src/store.rs). On start it rewrites every `svc/*` key, so a reader that keeps the environment open sees the new run.

**How the daemon builds LMDB** (heed 0.22 bundles lmdb-master-sys 0.2.6, measured on 2026-11-12):
- liblmdb 0.9.70 with `MDB_DEVEL 0`, lock format version 2, data version 1;
- no `posix-sem` feature, so each platform uses LMDB's default lock: robust POSIX mutex on Linux and on FreeBSD 11 or later, SysV semaphore on macOS;
- the lock type is part of the lock file's format signature. A reader built with another lock type gets `MDB_INCOMPATIBLE`; it does not corrupt anything.

The daemon's DNS path already picks backends (crates/emfor-daemon/src/select.rs) for round-robin and failover. It never computes the sticky hash, because DNS has no affinity key. Terms follow docs/spectra/LANGUAGE.md: 查詢 (`emfor_lookup`), 錯誤碼, 黏著鍵, 分配方式, 心跳.

## Goals / Non-Goals

**Goals:**

- Settle, before writing any client, whether LMDB works for C (through the system liblmdb) and for PHP (through `dba`).
- Define one lookup contract that every language implements identically, checked by shared test data.
- Ship C and Rust clients that pass all of that data.

**Non-Goals:**

- Go, Python, Perl and PHP clients (next change, `emfor-clients-2`).
- Store format or daemon changes, packaging, a configuration API in code, a degraded-result signal to callers, and balance methods beyond the first three.
- The FreeBSD run of the spike, which waits until the manually started CI is run.

## Decisions

### Compatibility spike before any client

The first task is a spike. Each check opens the store in a running daemon's directory, read-only:

| Check | How | Passes when |
| --- | --- | --- |
| C with the system liblmdb | A small C program linked through `pkg-config lmdb` opens the file with `MDB_RDONLY | MDB_NOSUBDIR | MDB_NOTLS` and reads `meta/heartbeat` three times, 1s apart | open succeeds and the heartbeat value increases |
| PHP `dba` | `dba_open($path, "r", "lmdb")` from the distribution's PHP packages | `dba_handlers()` lists `lmdb`, the open succeeds, and `dba_fetch("meta/schema_version")` returns `1` |

The spike runs on macOS (Homebrew `lmdb`, `php`) and in a Debian container (`liblmdb-dev`, `php-cli`, the PHP dba package). Scripts live in tests/spike/. Results go in tests/spike/RESULTS.md, with OS, package versions, the LMDB version reported by `mdb_version`, and pass or fail. FreeBSD is added to the CI `freebsd` job but is not run in this change.

### LMDB exit rule

Leaving LMDB is decided only from the spike results.
- **Trigger 1 (PHP):** the distribution's `dba` has no `lmdb` handler, or it cannot open the daemon's `MDB_NOSUBDIR` file read-only. A PHP FFI binding does not count as a fix unless the user accepts enabling FFI, which production PHP often disables.
- **Trigger 2 (locks):** the system liblmdb returns `MDB_INCOMPATIBLE` on a supported platform, and clients cannot be pointed at a liblmdb built with matching options.
- **Not reasons:** performance, or one awkward binding that a thin C wrapper can fix.

If a trigger fires, work on this change stops after the spike and the results are reported. A separate change, `emfor-sqlite-store`, then makes these changes:
- the store moves to SQLite in WAL mode, with table `kv(key TEXT PRIMARY KEY, value TEXT)`, the same keys and JSON, one transaction per writer batch, and read-only clients;
- `schema_version` becomes 2;
- the daemon gets a SQLite sink in place of crates/emfor-daemon/src/store.rs.

That change must verify that read-only WAL readers need existing `-wal` and `-shm` files with readable permissions. Its fallback is CDB, rewritten as a whole file and swapped in with an atomic rename. No client exists yet, so there is nothing to migrate. Afterwards this change resumes with the store read through SQLite.

### Lookup order and error codes

`emfor_lookup(name, affinity_key?)` runs these steps in order. The first one that ends the lookup decides the result.

1. **Open the store** (the first time, or after a fork; see "Opening, fork and threads").
   - The file is missing, permission is denied, `MDB_INCOMPATIBLE` is returned, or any other open error → `UNAVAILABLE`.
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
6. **Pick one candidate by `balance`** and return its `id`.

| Code | Meaning |
| --- | --- |
| `UNAVAILABLE` | The store cannot be used: it cannot be opened, is not initialized, or the client's environment setting is invalid. |
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

There is no configuration function. A test entry point that takes the path and staleness directly exists in each client for its own tests and the conformance runner; it is not part of the public API.

### Opening, fork and threads

**Opening:**
- The environment is opened with `MDB_RDONLY | MDB_NOSUBDIR | MDB_NOTLS`, once per process, and reused.
- `MDB_NOTLS` lets read transactions be created on any thread.
- A failed open is not remembered: the next lookup tries again, so a client started before the daemon recovers by itself.

**Fork:** each lookup compares `getpid()` with the pid that opened the environment. If they differ:
- the inherited handle is dropped without closing it, because LMDB forbids using it after a fork;
- the store is opened again;
- the environment variables are read again;
- the round-robin counters start afresh.

**Threads:**
- `emfor_lookup` may be called from many threads at once.
- Opening is guarded by a mutex.
- Each lookup uses its own read transaction.
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
- **`schema_version`** may be omitted, so the key is absent.
- **`raw`** holds extra keys written verbatim, for malformed records.
- **Each lookup** has `name`, an optional `affinity_key`, and exactly one of:
  - `expect`: one `ip:port`;
  - `expect_error`: one of the four codes;
  - `expect_all_of`: a list of `ip:port`. The runner calls the lookup that many times in a row and the results, as a set, must equal the list. This is how round-robin is checked without fixing its start.

`sticky-v1.json` lists hash vectors `{key, id, hash}`, where `hash` is the big-endian `u32`, plus pick vectors `{key, ids, expect}`. The vectors are generated once with Python's hashlib during apply and committed.

A case that needs a missing store (`UNAVAILABLE`) uses `"store": null`: the runner points the client at a path that does not exist.

### Fixture writer

`emfor-fixture` lives at tools/emfor-fixture/, a workspace member with a binary target. It takes a case's `store` object (`emfor-fixture <case.json> <index> <out.mdb>`) and writes an LMDB file with the same flags the daemon uses (`MDB_NOSUBDIR`, default database). It uses heed, so the files carry the daemon's lock type. The C and Rust conformance runners call it once per case.

### C client interface

clients/c/emfor.h:

```c
typedef enum {
    EMFOR_OK = 0,
    EMFOR_UNAVAILABLE = 1,
    EMFOR_INCOMPATIBLE = 2,
    EMFOR_NOT_FOUND = 3,
    EMFOR_ALL_DOWN = 4,
    EMFOR_INVALID_ARGUMENT = 5   /* C only: NULL name or buffer too small */
} emfor_status;

#define EMFOR_ADDR_MAX 64   /* enough for "[IPv6]:port" plus NUL */

emfor_status emfor_lookup(const char *name, const char *affinity_key,
                          char *out, size_t out_len);
const char *emfor_status_name(emfor_status status); /* "UNAVAILABLE", ... */
const char *emfor_last_error(void); /* thread-local detail of the last failure */
```

- `affinity_key` may be NULL.
- On `EMFOR_OK`, `out` holds the NUL-terminated `ip:port`.
- The library embeds cJSON (MIT) and a public-domain MD5. It links the system liblmdb and pthreads.
- `make` in clients/c/ builds `libemfor.a` and a shared library (`libemfor.so`, or `libemfor.dylib` on macOS). `make test` runs the unit tests and the conformance runner.
- Supported compilers: the system `cc` on Linux, FreeBSD and macOS, using C11 atomics and `_Thread_local`.

### Rust client interface

The crate `emfor-client` lives at clients/rust/:

```rust
pub fn lookup(name: &str, affinity_key: Option<&str>) -> Result<String, Error>;

pub struct Error { pub code: ErrorCode, pub message: String }
pub enum ErrorCode { Unavailable, Incompatible, NotFound, AllDown }
// ErrorCode::as_str() -> "UNAVAILABLE" | "INCOMPATIBLE" | "NOT_FOUND" | "ALL_DOWN"
```

- It reads the store with heed, using the same liblmdb as the daemon, and parses records with `emfor_format::ServiceRecord`.
- It uses `md-5` for the sticky hash.
- The test entry point is behind the cargo feature `testing`.

## Implementation Contract

**Behavior**
- With the daemon running and service `api.svc.emfor` healthy on `10.0.0.11:8080` and `10.0.0.12:8080`, both clients return the two `id`s in alternation. Called with the same `affinity_key` on a sticky service, they return the same `id` every time.
- After the daemon stops, a lookup for a service with `on_all_down = "open"` still returns a backend; for one with `on_all_down = "closed"`, it fails with `ALL_DOWN`. This holds within `max_staleness` (3s) of the stop, at once if the stop zeroed the heartbeat.
- A process that calls the lookup, forks, and calls it again in the child gets a valid answer in both.

**Interfaces**
- C: `emfor_lookup`, `emfor_status_name`, `emfor_last_error`, `EMFOR_ADDR_MAX`, and the `emfor_status` values above.
- Rust: `emfor_client::lookup`, `Error`, `ErrorCode`.
- Environment: `EMFOR_STORE`, `EMFOR_MAX_STALENESS`.
- Data: tests/conformance/v1/cases.json and sticky-v1.json, in the shapes above. tests/spike/RESULTS.md records the spike results.

**Failure modes**
- Every failure is one of the four codes (plus `EMFOR_INVALID_ARGUMENT` in C), with a message naming the cause.
- No lookup panics or aborts on malformed store content.
- A missing store is retried on the next lookup.

**Acceptance criteria**
- tests/spike/RESULTS.md shows the C and PHP checks passing on macOS and Debian. Otherwise the change stops as described in "LMDB exit rule".
- `cargo test --workspace` passes, including the Rust client's conformance runner over every case and vector.
- `make -C clients/c test` passes on macOS and in the Debian container, including the C conformance runner and a fork test.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass. The C code compiles with `-Wall -Wextra -Werror`.

**Scope boundaries**

In scope: the spike, the `client-lookup` spec, the conformance data, `emfor-fixture`, and the C and Rust clients with their docs. Out of scope: everything under Non-Goals, and any edit to crates/emfor-daemon/ or crates/emfor-format/.

## Risks / Trade-offs

- **Clock skew between reader and daemon.** Both run on the same host, so they share one clock. A clock jump backwards makes the heartbeat look like it is in the future, and the store stays fresh for up to that jump. → Accepted. A jump of more than `max_staleness` is rare, and the daemon updates the heartbeat every second.
- **An `MDB_NOTLS` reader that never ends its transaction pins old pages.** → Each lookup opens and ends its own read transaction; no transaction outlives a lookup.
- **Round-robin is per process.** Many short-lived processes each start at a random point, so the load is only spread statistically. → This is the documented behavior in LANGUAGE.md.
- **Deleting and recreating the store file while clients run** leaves them reading the old, unlinked file, which looks stale. That falls back to `on_all_down`. → Documented: restart applications after deleting the store. The daemon itself never deletes the file.
- **The spike results only hold for the package versions tested.** → RESULTS.md records the versions; the same checks run again for each language in emfor-clients-2.

## Migration Plan

Additive: new libraries and test data. No existing behavior changes.

## Open Questions

None.
