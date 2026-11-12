## Context

emfor-clients (archived 2026-11-12) built the one client implementation: the Rust crate `emfor-client`, exported with cargo-c as the C library `libemfor` (`emfor.h`, `pkg-config emfor`). It carries the daemon's own LMDB, exports only `emfor_*` symbols, and handles threads and fork. Its C API is:

```c
emfor_status emfor_lookup(const char *name, const char *affinity_key, char *out, size_t out_len);
const char *emfor_status_name(int status);
const char *emfor_last_error(void);   /* thread-local */
#define EMFOR_ADDR_MAX 64
/* EMFOR_OK 0, UNAVAILABLE 1, INCOMPATIBLE 2, NOT_FOUND 3, ALL_DOWN 4, INVALID_ARGUMENT 5 */
```

The conformance data (tests/conformance/v1/) and `emfor-fixture` exist. The Rust and C runners call the client in-process and point it at each case's store through the `testing` feature, which the wrappers cannot use: they go through the shared library, whose configuration comes only from the environment, read once per process.

Tool versions on hand (2026-11-12): macOS has Go 1.27.3, Python 3.9.6 and Perl 5.34.1 (system), no PHP. The Debian 13 container (`rust:1-trixie`) gets Go, Python 3.13, Perl 5.40 and PHP 8.4 from apt.

## Goals / Non-Goals

**Goals:**

- Go, Python, Perl and PHP programs can call the lookup with one function, getting the same results as Rust and C.
- Each wrapper passes every conformance case and pick vector.
- One `make test` per wrapper, runnable on macOS and in the Debian container, and a CI job that runs them all.

**Non-Goals:**

- Publishing packages (Go module tag, PyPI, CPAN, PECL) or distribution packaging.
- Any lookup logic in a wrapper, or a change to `libemfor`'s C API or the Rust crate.
- PHP through FFI, Perl through FFI::Platypus, or pure-language reimplementations that read LMDB themselves (they would bring their own LMDB, which spike 1 showed cannot read the store).
- PHP thread-safe (ZTS) builds and Windows.
- Running the PHP tests on macOS (no PHP installed there); PHP is verified in the Debian container.

## Decisions

### PHP uses a C extension, not FFI

PHP's default `ffi.enable=preload` allows FFI only in CLI and preloaded scripts, so under php-fpm or Apache an FFI wrapper would need a php.ini change on every host. A small C extension built with `phpize` and `pkg-config emfor` works in every SAPI with no ini change. The cost is one build per PHP minor version. The user chose this on 2026-11-12.

Interface: `Emfor\lookup(string $name, ?string $affinity_key = null): string`, and `Emfor\LookupException extends RuntimeException` with a public readonly `string $errorCode` (the code name) and `getCode()` returning the C status value. A name or key with a NUL byte throws `ValueError`. Alternatives: FFI (rejected above); returning `false` on failure (rejected: PHP code commonly ignores it, and the code name would be lost).

### Perl uses XS, not FFI::Platypus

XS needs only a C compiler at install time, which installing `libemfor` already requires, and adds no runtime CPAN dependency. FFI::Platypus would need the module and libffi on every host, and macOS's system Perl does not ship it. The user chose this on 2026-11-12.

Interface: `Emfor::lookup($name, $affinity_key)` returns the address; `undef` or `""` is no key. A failure dies with an `Emfor::Error` object (`code`, `message`, stringifies as `"CODE: message"` via `use overload`). A name or key containing a NUL byte dies with the string `Emfor::lookup: invalid argument: ...`. `Makefile.PL` reads `pkg-config --cflags --libs emfor` with backticks, so no configure-time module is needed.

### Go uses cgo with pkg-config

Module path `github.com/vanillahsu/emfor/clients/go` (the repository's GitHub path), package `emfor`, `go 1.22` in go.mod. The file has `#cgo pkg-config: emfor` and calls `emfor_lookup` with a Go-allocated `EMFOR_ADDR_MAX` buffer.

Interface: `func Lookup(name, affinityKey string) (string, error)`. A failure returns `*emfor.Error{Code Code, Message string}`, whose `Error()` is `"CODE: message"`. `Code` is an int type with constants `Unavailable`, `Incompatible`, `NotFound`, `AllDown` and a `String()` giving the shared name. A NUL byte in either argument returns a plain error from the wrapper without calling C.

`emfor_last_error` is thread-local and goroutines move between OS threads, so the wrapper calls `runtime.LockOSThread` around the `emfor_lookup` and `emfor_last_error` pair. Alternative: a C helper that copies the message out in the same C call; rejected because it adds C code to maintain, while LockOSThread is two lines.

### Python uses ctypes

Package `emfor` (clients/python/emfor/__init__.py, `pyproject.toml` with no compiled parts), Python 3.9 or later. At import time it loads the library:

1. the path in `EMFOR_LIBRARY`, when set and non-empty;
2. otherwise `ctypes.util.find_library("emfor")`;
3. otherwise the names `libemfor.so.0`, `libemfor.so` and `libemfor.dylib`, through the system loader.

If none loads, it raises `ImportError` naming `EMFOR_LIBRARY` and the names tried. `EMFOR_LIBRARY` is a loading setting, not a lookup setting, so the rule that lookups are configured only by `EMFOR_STORE` and `EMFOR_MAX_STALENESS` still holds. Without it, a library installed under a non-standard prefix (such as the tests' temporary prefix) cannot be found on Linux, where `find_library` consults only `ldconfig`.

Interface: `lookup(name, affinity_key=None) -> str`. Arguments are encoded as UTF-8; `str` containing `"\0"` raises `ValueError`, and a non-`str` raises `TypeError`. A failure raises the subclass of `emfor.Error` for its code; each has `.code` and `.message`. ctypes releases the GIL during the call, and `emfor_last_error` is read on the same thread right after it.

### One conformance driver for every wrapper

`emfor-conformance`, a second binary in tools/emfor-fixture, replaces a runner per language. For each case it writes the store with the fixture library into a temporary directory, then for each lookup runs:

```
<program...> <name> <affinity-key or ""> <count>
```

with `EMFOR_STORE` and `EMFOR_MAX_STALENESS` (the case's raw `max_staleness`, or `<max_staleness_ms>ms`) in the environment. `count` is 1, or the size of `expect_all_of`, so round-robin is observed within one process as the cases expect. The program prints one line per lookup, `OK <address>` or `ERR <CODE> <message>`, and the driver compares them with the expectation (the same rules as clients/rust/tests/support `check`). For the pick vectors it writes one store with a sticky service `pick-<i>.svc.emfor` per vector and looks each up with its key. It prints a summary and exits non-zero on any failure. Hash vectors have no wrapper API; picks exercise the same hash.

One process per lookup is what the wrappers need anyway, because each reads the environment once per process. 30 cases have 45 lookups, plus 6 picks; at an interpreter start of about 50 ms (estimate, to be measured by timing `make test`) a run takes a few seconds.

Each wrapper ships its lookup program next to its tests: clients/go/cmd/emfor-conformance-lookup, clients/python/tests/conformance_lookup.py, clients/perl/t/conformance_lookup.pl, clients/php/tests/conformance_lookup.php, and clients/c/tests/conformance_lookup.c. The C one runs first, so a driver bug shows up against the library already proven by the in-process runners. Alternative: a runner per language reimplementing the case logic; rejected as five copies of the expectation rules.

### Shared libemfor build fragment

clients/libemfor.mk installs `libemfor` with `cargo cinstall --release --features capi` into clients/build/prefix (with `--libdir` set so Debian does not add a triplet directory), and defines `PKG_CONFIG_PATH`, the rpath flag, `EMFOR_LIBRARY` for Python, and the `emfor-conformance` command. clients/c/Makefile, clients/go/Makefile, clients/python/Makefile, clients/perl/Makefile and clients/php/Makefile include it, so each `make -C clients/<lang> test` builds and tests one wrapper. clients/c switches from its own build/prefix to the shared one.

Go, Perl and PHP binaries link the shared library with an rpath to the prefix, so tests need no `LD_LIBRARY_PATH`. The Perl XS module is built in clients/perl with MakeMaker's makefile named Makefile.xs and loaded from blib/ (`perl -I blib/lib -I blib/arch`). The PHP extension is built from a copy of its sources in clients/build/php/src, because phpize writes its own Makefile, and loaded with `php -d extension=clients/build/php/src/modules/emfor.so`.

### CI job

A `clients` job in .github/workflows/ci.yml on `ubuntu-latest` installs `golang`, `python3`, `perl` with `libperl-dev` headers, `php-dev` and `php-cli`, plus cargo-c and pkg-config, then runs `make -C clients test`, a top-level target that runs the five wrappers' tests. The workflow stays manual (`workflow_dispatch`).

## Implementation Contract

**Behavior**

- With the daemon running and `api.svc.emfor` healthy on two backends, each of Go, Python, Perl and PHP returns one of the two `ip:port` strings, and successive calls in one process alternate.
- A missing service fails with code `NOT_FOUND` in each language's failure form (spec example "wrapper call and failure form"); a NUL byte in the name fails with the language's argument error.
- A Python, Perl or PHP CLI process that looked up before `fork` gets a valid answer in the child.
- The PHP extension, loaded together with `dba` and its `lmdb` handler in one process on Debian, returns an address while the `dba` store stays readable.

**Interfaces**

- Go: `emfor.Lookup`, `emfor.Error`, `emfor.Code` and its four constants; module `github.com/vanillahsu/emfor/clients/go`.
- Python: `emfor.lookup`, `emfor.Error` and its four subclasses; environment `EMFOR_LIBRARY`.
- Perl: `Emfor::lookup`, `Emfor::Error` (`code`, `message`).
- PHP: `Emfor\lookup`, `Emfor\LookupException` (`errorCode`); extension name `emfor`.
- Driver: `emfor-conformance [--cases <cases.json>] [--vectors <sticky-v1.json>] [--timeout-ms <ms>] -- <program...>`, defaulting to tests/conformance/v1/ and a 10 s limit per program run; the lookup program protocol in the spec example "lookup program protocol".
- Make: `make -C clients/<lang> test` for c, go, python, perl, php; `make -C clients test` for all.

**Failure modes**

- `libemfor` missing at build time: `pkg-config` fails and the build stops with its message. Missing at run time: the loader error from the OS (Go, Perl, PHP) or `ImportError` naming `EMFOR_LIBRARY` (Python).
- A lookup program that crashes, exits non-zero, or runs past the time limit (then it is killed) makes every lookup of that run a failure, reported with its exit status and stderr or the timeout.
- The wrappers never retry or cache results; each call is one `emfor_lookup`.

**Acceptance criteria**

- `make -C clients/<lang> test` passes for go, python and perl on macOS and in the Debian container, and for php in the Debian container; `make -C clients/c test` still passes on both.
- `emfor-conformance` fails (non-zero exit, failures listed) against a program that always prints `OK 10.9.9.9:1`.
- A manual run against the daemon shows alternation and `NOT_FOUND` from each language; results recorded in the final task.

**Scope**

- In: clients/go, clients/python, clients/perl, clients/php, clients/libemfor.mk, clients/Makefile, the driver, the C lookup program, the CI job, READMEs, config.yaml and LANGUAGE.md updates.
- Out: anything listed under Non-Goals; changes to clients/rust or the daemon.

## Risks / Trade-offs

- [macOS system Perl may not build XS without Xcode's SDK headers] → `Makefile.PL` uses the Perl's own `Config`, which points at the SDK when the Command Line Tools are installed; if it still fails, the task records it and uses Homebrew Perl on macOS.
- [Go programs need cgo, so `CGO_ENABLED=0` builds cannot use the wrapper] → Documented in clients/go/README.md; such programs use DNS.
- [The PHP extension must be rebuilt for each PHP minor version] → Documented; the extension has no PHP-version-specific code beyond the standard API.
- [Process-per-lookup conformance is slower than in-process] → A few seconds per language (estimate above); acceptable for a test that runs on demand.
- [Python finds a stale `libemfor` from another prefix through the system search] → `EMFOR_LIBRARY` takes priority, and the version rule in the READMEs applies; a format mismatch still fails with `UNAVAILABLE` and the release message.

## Migration Plan

Additive: new directories and a new CI job. clients/c/Makefile moves its install prefix to clients/build/prefix, so an old clients/c/build directory can be deleted by hand.

## Open Questions

(none)
