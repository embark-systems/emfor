# emfor-client (Rust) and libemfor

Look up a healthy backend for a service from the local emfor state store.
The emfor daemon on the same host checks the backends and keeps the store up
to date; a lookup only reads it, so no request goes over the network.

This crate is the one client implementation. Rust programs use it directly.
Built with the `capi` feature, it is also `libemfor`, the C library that the
C, Go, Python, Perl and PHP clients wrap (see [../c/README.md](../c/README.md)).

## Rust API

```rust
match emfor_client::lookup("api.svc.emfor", Some("user-42")) {
    Ok(addr) => println!("connect to {addr}"), // "10.0.0.11:8080" or "[2001:db8::11]:8080"
    Err(e) => eprintln!("{e}"),                // e.g. "ALL_DOWN: ..."
}
```

```rust
pub fn lookup(name: &str, affinity_key: Option<&str>) -> Result<String, Error>;

pub struct Error { pub code: ErrorCode, pub message: String }
pub enum ErrorCode { Unavailable, Incompatible, NotFound, AllDown }
```

- `name` is the service name, such as `api.svc.emfor`. Case and one
  trailing dot are ignored.
- The result is the backend's `ip:port`, or `[ip]:port` for IPv6.
- The service's `balance` decides the backend: `round-robin` (in turn),
  `sticky`, `failover` (first healthy in config order), `weighted` (in
  proportion to backend weights), `locality` (backends in the host's site
  first) or `fastest` (in turn over the backends the daemon lists as fast). A `balance` this library does not know, written by a newer daemon,
  is treated as `round-robin`.
- `affinity_key` is used only by services with `balance = "sticky"`: the same
  key gets the same backend while that backend is healthy. Other services
  ignore it.
- `ErrorCode::as_str()` gives the name shared by every client language:

| Code | Meaning |
| --- | --- |
| `UNAVAILABLE` | The store cannot be used: it does not exist or cannot be opened, its LMDB format differs from this library's (install `libemfor` from the same emfor release as the daemon), it is not initialized, or `EMFOR_MAX_STALENESS` is invalid. |
| `INCOMPATIBLE` | The store's format version is neither 1 nor 2, or a record cannot be parsed. |
| `NOT_FOUND` | The store has no such service. |
| `ALL_DOWN` | The service has no healthy backend and `on_all_down = "closed"`. |

When the daemon's heartbeat is older than `EMFOR_MAX_STALENESS` (or zero,
after the daemon stopped), every backend counts as unhealthy, and the
service's `on_all_down` decides: `open` still returns a backend, `closed`
returns `ALL_DOWN`.

## Configuration

Only through the environment, read on the first lookup of a process and
again in a child after `fork`:

| Variable | Default | Value |
| --- | --- | --- |
| `EMFOR_STORE` | `/var/lib/emfor/state.mdb` | Path of the store. |
| `EMFOR_MAX_STALENESS` | `3s` | How old the heartbeat may be: a positive whole number followed by `ms` or `s`, such as `1500ms`. An invalid value makes every lookup fail with `UNAVAILABLE`. |

## Threads and fork

`lookup` is safe from many threads at once. Round-robin turns are counted
per process, starting at a random backend, so separate processes do not
coordinate.

`lookup` also works in a child after `fork`, including when the parent
looked up before forking: the child opens the store again.

Each lookup holds an LMDB reader slot only for the read itself
(`MDB_NOTLS`), so many threads and processes share the daemon's reader table
of 1024 slots. A process killed in the middle of a lookup leaves its slot
taken; the daemon frees such slots about once a minute.

Applications follow a store file that is replaced while they run: the
daemon moved a damaged store aside at start, or the file was deleted and the
daemon created a new one. When a lookup finds the open store stale, or fails
with an error other than `ALL_DOWN` from a fresh store, the library checks
whether the store path now holds another file and, if so, switches to it and
looks up again. No restart is needed. It checks at most once every 100 ms in
a process (one `stat`), so a replacement is followed within about 100 ms and
lookups in a stopped daemon's store stay fast. If the new file cannot be
opened yet, lookups keep using the old one.

Do not delete the lock file (`<store>-lock`) alone, and start the daemon
before applications that run as root or as the daemon's user. If the lock
file is missing, the first process to open the store creates it, owned by
that process's user, and a daemon running as another user may then fail to
start.

## The version rule

The store is read with the daemon's own LMDB build, compiled into this crate
and into `libemfor`. Other LMDB builds, such as the system's liblmdb or PHP's
`dba`, use a different lock format and cannot read the store (see
tests/spike/RESULTS.md). So `emfor-client` and `libemfor` must come from the
same emfor release as the daemon, and are upgraded together with it. A
mismatch shows up as `UNAVAILABLE` with a message that says so.

## Building and installing libemfor

Needs [cargo-c](https://github.com/lu-zero/cargo-c) (`cargo install cargo-c`).
In this directory:

```bash
cargo cinstall --release --features capi --prefix /usr/local
```

This installs:

- `lib/libemfor.so` (`libemfor.dylib` on macOS), the shared library;
- `lib/libemfor.a`, the static library;
- `include/emfor/emfor.h`;
- `lib/pkgconfig/emfor.pc`, so C code builds with `pkg-config --cflags --libs emfor`.

Use `--libdir` to choose the library directory, and `--destdir` when building
a package. `cargo cbuild --release --features capi` builds the same files
without installing them.

Prefer the shared library. It exports only the `emfor_*` functions, so its
bundled LMDB cannot clash with another LMDB in the same process (for example
PHP with `dba` loaded). The static library carries the bundled `mdb_*`
symbols as they are; link it only into programs that use no other LMDB.

## Tests

```bash
cargo test -p emfor-client --features capi,testing
```

The `testing` feature adds a test entry point with an explicit store path and
staleness; it is not public API. The tests run every case in
tests/conformance/v1/ through the Rust API and through the C API. The C
library itself is tested from C by `make -C clients/c test`.
