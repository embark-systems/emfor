# 9. Client libraries

[Contents](README.md) · Previous: [8. How a backend is chosen](08-choosing-a-backend.md) · Next: [10. DNS](10-dns.md)

A client library answers one question: "which `ip:port` should I connect to
for this service, right now?" It reads the local store, picks a backend as
[chapter 8](08-choosing-a-backend.md) describes, and returns. It sends
nothing over the network and does not retry, connect or pool connections:
that stays in your application.

Every client has one function, `lookup(service, affinity_key)`, and the same
four errors. Every example below was built and run against a running emfor
0.7.0 daemon in a Debian container; install the libraries as in
[chapter 4](04-installing.md#client-libraries).

## What every client has in common

### The answer

`ip:port` for IPv4, `[ip]:port` for IPv6, ready to pass to a connect
function. The affinity key matters only for `balance = "sticky"`; no key and
an empty key are the same.

### The errors

| Code | Meaning | Example message |
| --- | --- | --- |
| `UNAVAILABLE` | The store cannot be used: missing, unreadable, another LMDB build (a client from another release), not initialized, or an invalid `EMFOR_MAX_STALENESS`. | `store /var/lib/emfor/state.mdb does not exist; is the emfor daemon running?` |
| `INCOMPATIBLE` | The store's format version is neither 1 nor 2, or a record cannot be parsed. | |
| `NOT_FOUND` | No such service in the store. | `no service named "nope.svc.emfor"` |
| `ALL_DOWN` | No backend may be chosen and `on_all_down = "closed"` (or every backend is drained). | `service db.svc.emfor: no backend is healthy, and on_all_down is closed` |

Printed, an error reads `CODE: message`. What to do with each:

- `ALL_DOWN`: the service is down as far as emfor knows. Fail the request,
  or use a fallback.
- `NOT_FOUND`: usually a typo, or a service that is not in this host's
  config.
- `UNAVAILABLE`: the daemon is not running, the store is not readable by
  your user, or the client comes from another release. This is a problem of
  the host, not of the service.

A stale store is not an error: `on_all_down` decides
([chapter 8](08-choosing-a-backend.md#tiers)).

### The environment

Read at the first lookup of a process, and again in a child after `fork`:

| Variable | Default | Meaning |
| --- | --- | --- |
| `EMFOR_STORE` | `/var/lib/emfor/state.mdb` | The store. Set it when `[store] path` is not the default (the FreeBSD port uses `/var/db/emfor/state.mdb`). |
| `EMFOR_MAX_STALENESS` | `3s` | How old the heartbeat may be: a positive whole number with `ms` or `s` (`1500ms`, `5s`). An invalid value makes every lookup fail: `UNAVAILABLE: EMFOR_MAX_STALENESS="3" is invalid: use a positive whole number followed by ms or s, such as 3s or 1500ms`. |
| `EMFOR_LIBRARY` | | Python only: the path of `libemfor`, when it is not in a standard directory. |

There is no other configuration. The client never reads the daemon's config.

### Permissions

The process needs read access to the store and **write access to its lock
file** (`<store>-lock`), where every reader registers. With the default
modes (0640 and 0660, owned by the daemon's user and group), put the
application's user in the daemon's group:

```sh
# usermod -aG emfor www-data
```

A user outside the group gets
`UNAVAILABLE: cannot open store /var/lib/emfor/state.mdb: Permission denied`.
See [chapter 12](12-operating.md#who-can-read-the-store).

### Threads, fork and a replaced store

- `lookup` is safe from many threads at once. Round-robin counters are per
  process and start at a random backend.
- It works in a child after `fork()`, even if the parent looked up before or
  other threads were looking up at that moment: the child opens the store
  again.
- A lookup holds an LMDB reader slot only during the read. The store has
  1024 slots for the whole host. A process killed in the middle of a lookup
  leaves its slot taken until the daemon frees it, about once a minute.
- When the daemon replaces the store file (it moved a damaged one aside at
  start, or the file was deleted), running applications switch to the new
  file within about 100 ms. No restart is needed.
- A store file that exists but cannot be opened is tried again at most once
  a second; a missing one at every lookup, so an application started before
  the daemon finds the store as soon as it exists.

Do not delete `<store>-lock` alone, and start the daemon before applications
that run as root or as the daemon's user: if the lock file is missing, the
first process to open the store creates it, owned by that process's user, and
a daemon running as another user may then fail to start.

### Same release as the daemon

`libemfor` contains the daemon's own LMDB build, and the lock file format
depends on it: a system `liblmdb`, PHP's `dba` or a client of another emfor
release cannot read the store reliably. Install clients from the same
release as the daemon and upgrade them together. A mismatch shows up as
`UNAVAILABLE` with a message that says so.

### Cost

From [docs/performance.md](../performance.md) (Apple M4 Max, Rust client;
not measured for this manual):

| | macOS | Linux (container) |
| --- | --- | --- |
| First lookup in a process (opens the store) | 443 µs | 151 µs |
| Lookup, one thread, median | 1.5 µs | 1.1 µs |
| DNS A query to the daemon, median | 24.8 µs | 4.6 µs |

Go, Python, Perl and PHP add their own call overhead. Throughput grows with
threads up to about four; after that the shared reader lock of the store
sets the pace. A `sticky` lookup costs one MD5 per unit of weight of every
candidate. So: look up per request, not once at start-up. Caching the
answer defeats the health checks.

## C

```c
#include <emfor.h>
#include <stdio.h>

int main(int argc, char **argv)
{
    const char *service = argc > 1 ? argv[1] : "web.svc.emfor";
    char addr[EMFOR_ADDR_MAX];

    emfor_status st = emfor_lookup(service, NULL, addr, sizeof addr);
    if (st != EMFOR_OK) {
        fprintf(stderr, "%s: %s\n", emfor_status_name(st), emfor_last_error());
        return 1;
    }
    printf("%s\n", addr);
    return 0;
}
```

```console
$ cc app.c $(pkg-config --cflags --libs emfor) -o app
$ ./app
127.0.0.1:8080
$ ./app nope.svc.emfor
NOT_FOUND: no service named "nope.svc.emfor"
```

| Function | |
| --- | --- |
| `emfor_status emfor_lookup(const char *name, const char *affinity_key, char *out, uintptr_t out_len)` | Writes the address and a NUL to `out`. `affinity_key` may be `NULL`. On failure `out` is unchanged. |
| `const char *emfor_status_name(int status)` | `"OK"`, `"ALL_DOWN"` and so on; static. |
| `const char *emfor_last_error(void)` | The message of this thread's last failed call, valid until its next emfor call. |

`emfor_status` is `EMFOR_OK` (0), `EMFOR_UNAVAILABLE` (1),
`EMFOR_INCOMPATIBLE` (2), `EMFOR_NOT_FOUND` (3), `EMFOR_ALL_DOWN` (4) or
`EMFOR_INVALID_ARGUMENT` (5: a `NULL` name or buffer, a string that is not
UTF-8, or a buffer that is too small). `EMFOR_ADDR_MAX` (64) always fits.
The header also defines `EMFOR_MAJOR`, `EMFOR_MINOR` and `EMFOR_PATCH`.

If `libemfor` is not in a directory the dynamic linker searches, add
`-Wl,-rpath,<libdir>` or run `ldconfig`.

## Go

```go
package main

import (
	"errors"
	"fmt"
	"os"

	emfor "github.com/embark-systems/emfor/clients/go"
)

func main() {
	addr, err := emfor.Lookup("web.svc.emfor", "") // "" is no affinity key
	var e *emfor.Error
	switch {
	case err == nil:
		fmt.Println(addr)
	case errors.As(err, &e) && e.Code == emfor.AllDown:
		fmt.Fprintln(os.Stderr, "no healthy backend:", e.Message)
		os.Exit(1)
	default:
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
```

```sh
$ go get github.com/embark-systems/emfor/clients/go@v0.8.0
$ go run .
```

- `emfor.Lookup(name, affinityKey string) (string, error)`.
- A failure is an `*emfor.Error` with `Code` (`emfor.Unavailable`,
  `emfor.Incompatible`, `emfor.NotFound`, `emfor.AllDown`) and `Message`. A
  name or key with a NUL byte, or not valid UTF-8, gives a plain error.
- The package uses cgo and `pkg-config emfor`: it needs a C compiler and
  cannot be built with `CGO_ENABLED=0`.
- The module's tag is `clients/go/v0.8.0`. In the test, `go.mod` pointed at a
  local checkout with a `replace` line.

## Python

```python
import sys
import emfor

try:
    addr = emfor.lookup("web.svc.emfor")
except emfor.AllDownError as e:
    sys.exit(f"no healthy backend: {e.message}")
except emfor.Error as e:
    sys.exit(f"{e.code}: {e.message}")
print(addr)
```

- `emfor.lookup(name, affinity_key=None)`.
- Errors are `emfor.Error` subclasses: `UnavailableError`,
  `IncompatibleError`, `NotFoundError`, `AllDownError`, each with `.code`
  and `.message`.
- Python 3.9 or later. The package uses `ctypes`; nothing is compiled.
- Safe from many threads (the GIL is released during the call) and after
  `os.fork()`.
- It loads `libemfor` from `EMFOR_LIBRARY`, or else by its name from the
  standard directories.

## Perl

```perl
use strict;
use warnings;
use Emfor;

my $addr = eval { Emfor::lookup('web.svc.emfor') };
if (my $err = $@) {
    die $err unless ref $err;            # an invalid argument
    die "no healthy backend\n" if $err->code eq 'ALL_DOWN';
    die "$err\n";                        # "NOT_FOUND: ..."
}
print "$addr\n";
```

- `Emfor::lookup($name, $affinity_key)`; `undef` or `""` is no key.
- A failure dies with an `Emfor::Error`: `code`, `message`, and
  `"CODE: message"` as a string. A NUL byte in an argument dies with a plain
  string starting `Emfor::lookup: invalid argument`.
- Arguments are character strings, passed as UTF-8. Decode bytes first
  (`Encode::decode('UTF-8', $bytes)`), or a non-ASCII affinity key picks a
  different backend than other languages do.
- Build: `perl Makefile.PL && make -f Makefile.xs && make -f Makefile.xs install`
  in `clients/perl`.

## PHP

```php
<?php
try {
    $addr = Emfor\lookup('web.svc.emfor');
} catch (Emfor\LookupException $e) {
    fwrite(STDERR, $e->getMessage() . "\n");   // "ALL_DOWN: ..."
    exit(1);
}
echo $addr, "\n";
```

Build the extension for each PHP minor version you run (PHP 8.1 or later),
from a copy of the sources, since `phpize` writes its own Makefile:

```sh
$ mkdir -p /tmp/emfor-php
$ cp clients/php/config.m4 clients/php/emfor.c clients/php/emfor_arginfo.h /tmp/emfor-php/
$ cd /tmp/emfor-php && phpize && ./configure --enable-emfor && make
# make install
```

Then add `extension=emfor` to an ini file PHP loads (`php --ini` lists them).

- `Emfor\lookup(string $name, ?string $affinity_key = null): string`.
- A failure throws `Emfor\LookupException` (a `RuntimeException`):
  `$e->errorCode` is the code name, `getCode()` the C status value,
  `getMessage()` `"CODE: message"`. A NUL byte or invalid UTF-8 throws
  `ValueError`.
- Works after `pcntl_fork()` and next to the `dba` extension's `lmdb`
  handler.
- With PHP-FPM, every worker process is a separate client.

## Rust

```text
# Cargo.toml of your application
[dependencies]
emfor-client = { path = "clients/rust" }   # from the same emfor release
```

```rust
use std::process::ExitCode;

fn main() -> ExitCode {
    match emfor_client::lookup("web.svc.emfor", None) {
        Ok(addr) => {
            println!("{addr}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}"); // "ALL_DOWN: ..." and the like
            ExitCode::FAILURE
        }
    }
}
```

- `emfor_client::lookup(name: &str, affinity_key: Option<&str>) -> Result<String, Error>`.
- `Error` has `code` (`ErrorCode::Unavailable`, `Incompatible`, `NotFound`,
  `AllDown`) and a message; `Display` gives `CODE: message`.
- The crate contains the bundled LMDB itself, so no `libemfor` is needed.

## Without a client library

- **DNS**: any program can resolve `web.svc.emfor` through the daemon's DNS
  server ([chapter 10](10-dns.md)). A and AAAA answers carry no port.
- **The command line**: `emfor lookup web.svc.emfor` prints one address and
  exits 0, or prints the error and exits 1 (`ALL_DOWN`), 2 (no such service)
  or 3 (store unusable). Fine for scripts; too slow to run per request.
