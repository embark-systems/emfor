# emfor for PHP

Look up a healthy backend for a service in the local emfor state store.
A C extension for PHP 8.1 or later; it works under php-fpm, Apache and the
CLI with no `ffi.enable` setting.

```php
try {
    $addr = Emfor\lookup('api.svc.emfor', 'user-42');
} catch (Emfor\LookupException $e) {
    if ($e->errorCode === 'ALL_DOWN') { /* no healthy backend */ }
    error_log($e->getMessage());              // "NOT_FOUND: ..."
}
```

- `Emfor\lookup(string $name, ?string $affinity_key = null): string` returns
  `"ip:port"`, or `"[ip]:port"` for IPv6. The affinity key matters only for
  sticky services; `null` or `""` is no key.
- A failure throws `Emfor\LookupException` (extends `RuntimeException`):
  `errorCode` is the shared name (readonly), `getCode()` the C status
  value, and `getMessage()` is `"CODE: message"`.
- A name or key containing a NUL byte, or not valid UTF-8, throws
  `ValueError`.
- Works in a child after `pcntl_fork()`, and in the same process as the
  `dba` extension's `lmdb` handler.

## Building

The extension must be built for each PHP minor version it runs on.

phpize writes its own Makefile, so build from a copy of the sources:

```bash
mkdir -p /tmp/emfor-php
cp clients/php/config.m4 clients/php/emfor.c clients/php/emfor_arginfo.h /tmp/emfor-php/
cd /tmp/emfor-php && phpize && ./configure --enable-emfor && make && make install
```

Then enable it with `extension=emfor` in an ini file that PHP loads
(`php --ini` lists them). `config.m4` finds `libemfor` with
`pkg-config emfor` and records its directory as the extension's run path.

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

`make -C clients/php test` installs `libemfor`, builds the extension, runs
tests/emfor_test.php with it loaded (the fork check needs pcntl; the `dba`
check runs when the `lmdb` handler exists, and `EMFOR_REQUIRE_DBA=1`, set in
CI, makes its absence a failure), and runs the shared
conformance driver against tests/conformance_lookup.php. The tests are a plain
PHP script rather than `.phpt` files: `run-tests.php` starts PHP with `-n`,
which would leave out the `dba` extension the same-process check needs.
