## Why

emfor-clients delivered the lookup only to Rust and C (`libemfor`). Applications on the hosts are also written in Go, Python, Perl and PHP, and today they can reach emfor only through DNS, which returns an IP without the port and ignores `affinity_key`. This change gives each of these languages the same lookup by wrapping `libemfor`, the library that already carries the daemon's own LMDB.

## What Changes

- **Go** (clients/go/): package `emfor` built with cgo and `pkg-config emfor`; `emfor.Lookup(name, affinityKey string) (string, error)`, with failures as `*emfor.Error` holding the shared code.
- **Python** (clients/python/): package `emfor` that loads `libemfor` with `ctypes`; `emfor.lookup(name, affinity_key=None) -> str`, raising one exception class per shared code under `emfor.Error`.
- **Perl** (clients/perl/): module `Emfor` built with XS and `Makefile.PL`; `Emfor::lookup($name, $affinity_key)`, dying with an `Emfor::Error` object.
- **PHP** (clients/php/): C extension `emfor` built with `phpize`; `Emfor\lookup(string $name, ?string $affinity_key = null): string`, throwing `Emfor\LookupException`. The route is a C extension rather than FFI because PHP's default `ffi.enable=preload` blocks FFI under php-fpm and Apache (decided 2026-11-12).
- **Shared conformance driver**: a new `emfor-conformance` binary in tools/emfor-fixture runs every case in tests/conformance/v1/ against a small lookup program in each language, one process per lookup, with `EMFOR_STORE` and `EMFOR_MAX_STALENESS` set per case. The C library gets the same program, which proves the driver against a known-good client.
- **Build and test**: one Makefile fragment installs `libemfor` into a temporary prefix for all wrappers; each wrapper has `make test` covering the conformance driver, error mapping, invalid arguments, and fork or threads where the language has them. A `clients` job in the GitHub Actions workflow (manual dispatch) runs them on Linux.
- **Docs**: a README per wrapper, and the project context and vocabulary updated with the chosen PHP and Perl routes.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `client-lookup`: adds a requirement for the Go, Python, Perl and PHP wrappers (call shape, error form, invalid arguments, loading `libemfor`), and changes "Shared conformance data defines the contract" so wrappers run the cases and pick vectors through the shared driver, while the hash vectors stay with the Rust runner.

## Impact

- New code: clients/go/, clients/python/, clients/perl/, clients/php/, clients/libemfor.mk, clients/c/tests/conformance_lookup.c, and the `emfor-conformance` binary in tools/emfor-fixture/.
- Changed: clients/c/Makefile (uses clients/libemfor.mk and runs the driver), tests/conformance/v1/README.md, .github/workflows/ci.yml (new `clients` job), docs/spectra/config.yaml and docs/spectra/LANGUAGE.md.
- Build dependencies for running the tests: Go with cgo, Python 3.9 or later, Perl with a C compiler, PHP development files (`phpize`), plus cargo-c and pkg-config as for clients/c.
- No change to the daemon, the store format, `libemfor`'s C API or the Rust crate.
