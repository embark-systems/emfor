# emfor for Go

Look up a healthy backend for a service in the local emfor state store.

```go
import "github.com/embark-systems/emfor/clients/go"

addr, err := emfor.Lookup("api.svc.emfor", "user-42") // "" is no key
var e *emfor.Error
switch {
case err == nil:
	// connect to addr: "10.0.0.11:8080" or "[2001:db8::11]:8080"
case errors.As(err, &e) && e.Code == emfor.AllDown:
	// no healthy backend
default:
	log.Print(err) // "NOT_FOUND: ...", or an invalid argument
}
```

- `Lookup(name, affinityKey string) (string, error)`. The affinity key
  matters only for sticky services.
- A failure is an `*emfor.Error` with `Code` (`emfor.Unavailable`,
  `emfor.Incompatible`, `emfor.NotFound`, `emfor.AllDown`; `Code.String()`
  gives the shared name) and `Message`.
- A name or key containing a NUL byte, or not valid UTF-8, returns a plain
  error, not an `*emfor.Error`.
- Safe from many goroutines. Round-robin turns are counted per process.

The package uses cgo and finds the library with `pkg-config emfor`, so it
needs a C compiler at build time and cannot be built with `CGO_ENABLED=0`;
such programs can use emfor's DNS instead.

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
| `INCOMPATIBLE` | The store's format version is not 1, or a record cannot be parsed. |
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

Restart applications after deleting the store file; do not delete its
`-lock` file alone.

## Tests

`make -C clients/go test` installs `libemfor` into clients/build/prefix, runs
`go vet` and `go test` against a store written by `emfor-fixture`, and runs
the shared conformance driver against cmd/emfor-conformance-lookup.
