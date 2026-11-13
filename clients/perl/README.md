# emfor for Perl

Look up a healthy backend for a service in the local emfor state store.
An XS module: building it needs Perl's headers and a C compiler; running it
needs no CPAN modules.

```perl
use Emfor;

my $addr = eval { Emfor::lookup('api.svc.emfor', 'user-42') };
if (my $err = $@) {
    die $err unless ref $err;                 # invalid argument
    warn "no healthy backend\n" if $err->code eq 'ALL_DOWN';
    warn "$err\n";                             # "NOT_FOUND: ..."
}
```

- `Emfor::lookup($name, $affinity_key)` returns `"ip:port"`, or
  `"[ip]:port"` for IPv6. The affinity key matters only for sticky
  services; `undef` or `""` is no key.
- Arguments are character strings, passed to the library as UTF-8. Decode
  bytes first (for example `Encode::decode('UTF-8', $bytes)`); undecoded
  non-ASCII bytes would be read as Latin-1 and pick a different backend.
- A failure dies with an `Emfor::Error` object: `code` (the shared name),
  `message`, and `"CODE: message"` when used as a string.
- A name or key containing a NUL byte dies with a plain string starting
  `Emfor::lookup: invalid argument`.
- Works in a child after `fork`.

## Building

```bash
cd clients/perl
perl Makefile.PL && make -f Makefile.xs && make -f Makefile.xs install
```

`Makefile.PL` finds `libemfor` with `pkg-config emfor` and records its
directory as the module's run path. MakeMaker's makefile is named
Makefile.xs; Makefile in this directory runs the tests.

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

Applications follow a store file that is replaced while they run (see the
C client's README); do not delete its `-lock` file alone.

## Tests

`make -C clients/perl test` installs `libemfor`, builds the module, runs
t/*.t against a store written by `emfor-fixture`, and runs the shared
conformance driver against t/conformance_lookup.pl.
