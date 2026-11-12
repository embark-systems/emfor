# emfor C client (libemfor)

Look up a healthy backend for a service from the local emfor state store.
The emfor daemon on the same host checks the backends and keeps the store up
to date; a lookup only reads it.

`libemfor` is built from the Rust crate in [../rust/](../rust/README.md) with
cargo-c. It contains the daemon's own LMDB build, so it must come from the
same emfor release as the daemon and be upgraded together with it. A system
liblmdb cannot read the store (its lock format differs).

## Installing

```bash
cd clients/rust
cargo cinstall --release --features capi --prefix /usr/local
```

See [../rust/README.md](../rust/README.md) for the installed files and for
shared versus static linking (prefer the shared library).

## Building a program

```bash
cc example.c $(pkg-config --cflags --libs emfor) -o example
```

```c
#include <emfor.h>
#include <stdio.h>

char addr[EMFOR_ADDR_MAX];
emfor_status st = emfor_lookup("api.svc.emfor", "user-42", addr, sizeof addr);
if (st == EMFOR_OK)
    printf("connect to %s\n", addr); /* "10.0.0.11:8080" or "[2001:db8::11]:8080" */
else
    fprintf(stderr, "%s: %s\n", emfor_status_name(st), emfor_last_error());
```

[example.c](example.c) is a complete program.

## API

```c
emfor_status emfor_lookup(const char *name, const char *affinity_key,
                          char *out, size_t out_len);
const char *emfor_status_name(int status);
const char *emfor_last_error(void);
#define EMFOR_ADDR_MAX 64
```

`emfor_lookup` looks up one backend of service `name` (such as
`api.svc.emfor`; case and one trailing dot are ignored) and writes its
`ip:port`, or `[ip]:port` for IPv6, with a terminating NUL to `out`.
A buffer of `EMFOR_ADDR_MAX` bytes always fits. `affinity_key` may be NULL;
it is used only by services with `balance = "sticky"`, where the same key
gets the same backend while that backend is healthy. The service's `balance`
decides the backend: `round-robin`, `sticky`, `failover`, `weighted` (in
proportion to backend weights), `locality` (backends in the host's site
first) or `fastest` (in turn over the backends the daemon lists as fast); a `balance` the library does not know is treated as `round-robin`.

On failure `out` is left unchanged, and the status says why:

| Status | Value | Meaning |
| --- | --- | --- |
| `EMFOR_OK` | 0 | The address is in `out`. |
| `EMFOR_UNAVAILABLE` | 1 | The store cannot be used: it does not exist or cannot be opened, its LMDB format differs from this library's (install `libemfor` from the same emfor release as the daemon), it is not initialized, or `EMFOR_MAX_STALENESS` is invalid. |
| `EMFOR_INCOMPATIBLE` | 2 | The store's format version is not 1, or a record cannot be parsed. |
| `EMFOR_NOT_FOUND` | 3 | The store has no such service. |
| `EMFOR_ALL_DOWN` | 4 | The service has no healthy backend and `on_all_down = "closed"`. |
| `EMFOR_INVALID_ARGUMENT` | 5 | C only: `name` or `out` is NULL, a string is not UTF-8, or `out_len` is too small for the address. |

The first four are the error codes shared by every client language.

When the daemon's heartbeat is older than `EMFOR_MAX_STALENESS` (or zero,
after the daemon stopped), every backend counts as unhealthy, and the
service's `on_all_down` decides: `open` still returns a backend, `closed`
returns `EMFOR_ALL_DOWN`.

`emfor_status_name` returns the status's name, such as `"NOT_FOUND"`
(`"UNKNOWN"` for other values); the string is static.
`emfor_last_error` returns a message about the last failed call on the
calling thread, valid until that thread's next emfor call.

No Rust panic reaches C: an internal error is reported as
`EMFOR_UNAVAILABLE` with a message starting `internal error`.

## Configuration

Only through the environment, read on the first lookup of a process and
again in a child after `fork`:

| Variable | Default | Value |
| --- | --- | --- |
| `EMFOR_STORE` | `/var/lib/emfor/state.mdb` | Path of the store. |
| `EMFOR_MAX_STALENESS` | `3s` | How old the heartbeat may be: a positive whole number followed by `ms` or `s`, such as `1500ms`. An invalid value makes every lookup fail with `EMFOR_UNAVAILABLE`. |

## Threads and fork

`emfor_lookup` is safe from many threads at once. Round-robin turns are
counted per process, starting at a random backend. It also works in a child
after `fork`, including when the parent looked up before forking.

If the store file is deleted and created again while applications run, they
keep reading the old, deleted file, which looks stale. Restart the
applications after deleting the store. The daemon itself never deletes it.

Do not delete the lock file (`<store>-lock`) alone, and start the daemon
before applications that run as root or as the daemon's user. If the lock
file is missing, the first process to open the store creates it, owned by
that process's user, and a daemon running as another user may then fail to
start.

## Tests

```bash
make -C clients/c test
```

Installs `libemfor` into clients/c/build/prefix, builds the programs in
tests/ with `-Wall -Wextra -Werror`, writes a store with `emfor-fixture`,
and runs: lookups and status codes, a fork test, 8 threads × 1000 lookups,
and a check that the shared library exports only `emfor_*` symbols. Needs
cargo, cargo-c, pkg-config and a C compiler.
