# 6. Client libraries

[Manual](README.md) · Previous: [5. Choosing a backend](05-backend-selection.md) · Next: [7. DNS](07-dns.md)

A client library has one call: look up a service, get back one backend
address. It reads the local store, so it needs the emfor daemon running on
the same host, and nothing else.

## The lookup

Every client takes a service name and an optional affinity key, and
returns a string:

- `ip:port` for IPv4, such as `10.0.0.11:8080`;
- `[ip]:port` for IPv6, such as `[2001:db8::11]:8080`.

Both forms can be passed to the usual "connect to host:port" functions of
most languages. The name is matched without regard to case and with one
trailing dot ignored. The affinity key matters only for
`balance = "sticky"`; an empty key is no key.

Call `lookup` for every new connection, not once at startup: the answer
changes as backends fail and recover, and a lookup costs about a
microsecond.

## Errors

Every client fails with one of four codes, named the same in every
language:

| Code | Meaning | What to do |
| --- | --- | --- |
| `UNAVAILABLE` | The store cannot be used: it does not exist, cannot be opened, was written by another emfor release's LMDB, is not initialized, or `EMFOR_MAX_STALENESS` is invalid. | Check that the daemon runs, `EMFOR_STORE`, file permissions, and that `libemfor` comes from the daemon's release. The message says which. |
| `INCOMPATIBLE` | The store has another format version, or a record cannot be read. | Install the client from the daemon's release. |
| `NOT_FOUND` | No service of that name. | Check the name against the config. |
| `ALL_DOWN` | No healthy backend, and the service has `on_all_down = "closed"`. | Fail the request, or retry later. |

A stale store is not an error of its own: it makes every backend count as
unhealthy, so an `open` service still returns an address and a `closed`
one fails with `ALL_DOWN`.

## Configuration

Clients have no configuration call. Two environment variables are read on
the first lookup in a process (and again in a child after `fork`):

| Variable | Default | Meaning |
| --- | --- | --- |
| `EMFOR_STORE` | `/var/lib/emfor/state.mdb` | Path of the store; must match the daemon's `[store] path`. |
| `EMFOR_MAX_STALENESS` | `3s` | How old the heartbeat may be: a positive whole number with `ms` or `s`, such as `1500ms`. An invalid value makes every lookup fail with `UNAVAILABLE`. |

## Examples

**Rust**

```rust
match emfor_client::lookup("api.svc.emfor", Some("user-42")) {
    Ok(addr) => println!("connect to {addr}"),
    Err(e) => eprintln!("{e}"), // "ALL_DOWN: ..."; e.code is an ErrorCode
}
```

**C**

```c
#include <emfor.h>

char addr[EMFOR_ADDR_MAX];
emfor_status st = emfor_lookup("api.svc.emfor", "user-42", addr, sizeof addr);
if (st == EMFOR_OK)
    printf("connect to %s\n", addr);
else
    fprintf(stderr, "%s: %s\n", emfor_status_name(st), emfor_last_error());
```

Build with `cc app.c $(pkg-config --cflags --libs emfor)`. C also has
`EMFOR_INVALID_ARGUMENT` for a NULL or non-UTF-8 argument or a buffer that
is too small; `EMFOR_ADDR_MAX` bytes always fit.

**Go**

```go
import "github.com/embark-systems/emfor/clients/go"

addr, err := emfor.Lookup("api.svc.emfor", "user-42")
var e *emfor.Error
if errors.As(err, &e) && e.Code == emfor.AllDown {
	// no healthy backend
}
```

**Python**

```python
import emfor

try:
    addr = emfor.lookup("api.svc.emfor", affinity_key="user-42")
except emfor.AllDownError:
    ...
except emfor.Error as e:
    log.warning("%s: %s", e.code, e.message)
```

**Perl**

```perl
use Emfor;

my $addr = eval { Emfor::lookup('api.svc.emfor', 'user-42') };
warn "$@\n" if $@;   # an Emfor::Error with ->code and ->message
```

**PHP**

```php
try {
    $addr = Emfor\lookup('api.svc.emfor', 'user-42');
} catch (Emfor\LookupException $e) {
    error_log($e->getMessage());   // $e->errorCode is "ALL_DOWN" etc.
}
```

Each client's README covers installing it and its exact API:
[Rust](../../clients/rust/README.md), [C](../../clients/c/README.md),
[Go](../../clients/go/README.md), [Python](../../clients/python/README.md),
[Perl](../../clients/perl/README.md), [PHP](../../clients/php/README.md).

## Threads, processes and fork

- Lookups are safe from many threads at once.
- The store is opened once per process and reused. A lookup holds an LMDB
  reader slot only while it reads. The store has 1024 slots shared by all
  processes on the host; a process killed in the middle of a lookup leaves
  its slot taken until the daemon frees it, about once a minute.
- After `fork`, the child opens the store again on its first lookup; this
  works also when the parent looked up before forking (php-fpm, preforking
  servers), and when other threads of the parent were looking up at the
  moment of the fork.
- Round-robin counters are per process.

## When the store file is replaced or grows

If the store file is replaced while applications run (the daemon moved a
damaged store aside, or someone deleted it and the daemon created a new
one), clients notice within about 100 ms and switch to the new file; no
application restart is needed. The same holds when the daemon is restarted
with a larger `[store] map_size` and the store grows past the old size.

A store file that exists but cannot be opened (another LMDB format, no
permission, not a store) is tried again at most once a second; lookups in
between fail with the same `UNAVAILABLE` error. A missing store file is
tried on every lookup, so applications started before the daemon find the
store as soon as it exists.

Do not delete the lock file (`<store>-lock`) alone. Start the daemon
before applications that run as root or as the daemon's user: if the lock
file is missing, the first process to open the store creates it, owned by
that process's user, and the daemon may then fail to start.

## Same release as the daemon

The client library contains the daemon's own LMDB build, and the store's
lock file format depends on it. Install `libemfor` (and the wrappers) from
the same emfor release as the daemon, and upgrade them together. A
mismatch shows up as `UNAVAILABLE` with a message that says so.

### Store format version

The daemon writes store format version 2, which adds the backend field
`backup`. Clients from this release read versions 1 and 2, so they work
with an older daemon too; older clients fail every lookup on a version 2
store with `INCOMPATIBLE`. When upgrading, upgrade the clients (and
`emfor status`) on a host before the daemon.

## Performance

A lookup takes about 1–1.5 µs on one thread. Throughput grows with threads
up to about four, then the shared reader lock of the store sets the pace
(about two million lookups a second in total on Linux). See
[docs/performance.md](../performance.md) for the measurements.
