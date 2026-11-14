# emfor for Python

Look up a healthy backend for a service in the local emfor state store.
Python 3.9 or later; no compiled parts (it uses `ctypes`).

```python
import emfor

try:
    addr = emfor.lookup("api.svc.emfor", affinity_key="user-42")
except emfor.AllDownError:
    ...  # no healthy backend
except emfor.Error as e:
    log.warning("%s: %s", e.code, e.message)
```

- `lookup(name, affinity_key=None) -> str` returns `"ip:port"`, or
  `"[ip]:port"` for IPv6. The affinity key matters only for sticky
  services; `None` or `""` is no key.
- Failures raise `UnavailableError`, `IncompatibleError`, `NotFoundError`
  or `AllDownError`, all subclasses of `emfor.Error`, with `.code` (the
  shared name) and `.message`. `str(e)` is `"CODE: message"`.
- A non-`str` argument raises `TypeError`; one containing `"\0"` (or a
  lone surrogate) raises `ValueError`.
- Safe from many threads (the GIL is released during the call) and after
  `os.fork()`.

## Finding libemfor

At import, the package loads the library from the path in `EMFOR_LIBRARY`
if it is set, and otherwise tries, in order: the names of this release's
library, `libemfor.so.0.4` and `libemfor.0.4.dylib` for release 0.4.x (they
follow `emfor.__version__`); what `ctypes.util.find_library("emfor")` finds,
which may be another release's; and `libemfor.so` and `libemfor.dylib`. If
none loads, the import raises `ImportError`. On Linux,
`find_library` only knows libraries in the `ldconfig` cache, so set
`EMFOR_LIBRARY` for a library outside the standard directories.

Install the package with `pip install clients/python`, or put clients/python
on `PYTHONPATH`.

## Configuration

Only through the environment, read on the first lookup of a process and
again in a child after `fork`:

| Variable | Default | Value |
| --- | --- | --- |
| `EMFOR_STORE` | `/var/lib/emfor/state.mdb` | Path of the store. |
| `EMFOR_MAX_STALENESS` | `3s` | How old the heartbeat may be: a positive whole number followed by `ms` or `s`. An invalid value makes every lookup fail with `UNAVAILABLE`. |

When the heartbeat is older than that (or zero, after the daemon stopped),
every backend counts as unhealthy and the service's `on_all_down` decides:
`open` still returns a backend, `closed` fails with `ALL_DOWN`.

## Error codes

| Code | Meaning |
| --- | --- |
| `UNAVAILABLE` | The store cannot be used: missing, unreadable, another LMDB format (install `libemfor` from the same emfor release as the daemon), not initialized, or an invalid `EMFOR_MAX_STALENESS`. |
| `INCOMPATIBLE` | The store's format version is neither 1 nor 2, or a record cannot be parsed. |
| `NOT_FOUND` | No such service. |
| `ALL_DOWN` | No healthy backend, and `on_all_down = "closed"`. |

## Installing libemfor

This wrapper calls `libemfor`, the C library built from
[clients/rust](../rust/README.md), which contains the daemon's own LMDB.
Install it from the same emfor release as the daemon and upgrade them
together:

```bash
cd clients/rust
cargo cinstall --release --features capi --prefix /usr/local
```

Applications follow a store file that is replaced while they run (see the
C client's README); do not delete its `-lock` file alone.

## Tests

`make -C clients/python test` installs `libemfor`, runs the unit tests in
tests/ against a store written by `emfor-fixture`, and runs the shared
conformance driver against tests/conformance_lookup.py.
