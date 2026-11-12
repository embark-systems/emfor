# Lookup performance

A lookup reads the local state store: one LMDB read transaction, one JSON
record parsed, one backend picked. It sends nothing over the network. This
page gives measured figures and the limits behind them.

## Measured figures

Measured on 2026-11-13 with `clients/rust/examples/bench.rs` (see
[Measuring it yourself](#measuring-it-yourself)) against a running daemon:
the quick-start config of the [README](../README.md) with two healthy HTTP
backends, release build, Apple M4 Max (14 cores). Linux is Debian 13
(aarch64) in a container on the same machine. Figures vary by a few percent
from run to run.

| | macOS 27 | Linux 7.2 |
| --- | --- | --- |
| First lookup in a process (opens the store) | 443 µs | 151 µs |
| Lookup, one thread: median | 1.5 µs | 1.1 µs |
| Lookup, one thread: 99th percentile | 3.3 µs | 2.8 µs |
| DNS A query to the daemon on 127.0.0.1: median | 24.8 µs | 4.6 µs |

The Rust client is the library every other client wraps, so a C program
pays the same; Go, Python, Perl and PHP add their own call overhead on top.

Lookups from several threads of one process, in total:

| Threads | macOS 27 | Linux 7.2 |
| --- | --- | --- |
| 1 | 710,000 /s | 940,000 /s |
| 2 | 870,000 /s | 1,650,000 /s |
| 4 | 1,050,000 /s | 2,810,000 /s |
| 8 | 330,000 /s | 1,930,000 /s |
| 14 | 130,000 /s | 2,110,000 /s |

## Why throughput stops growing with threads

The client opens the store with `MDB_NOTLS`, and every lookup begins a new
read transaction. Beginning one takes the reader-table lock in the store's
lock file (`<store>-lock`), which every reader on the host shares. Beyond
about four threads that lock, not the lookup itself, sets the pace. On
Linux the total levels off at about two million lookups a second; macOS
locks degrade under contention, so there the total falls instead. macOS is
a development platform only.

A faster design was measured and not adopted: each thread keeps its read
transaction and renews it (`mdb_txn_reset` / `mdb_txn_renew`), and the
client's own locks become lock-free reads. With 8 threads that reached
5.2 million lookups a second on Linux and 3.6 million on macOS. The cost is
that every thread that has looked up keeps one LMDB reader slot until it
exits, where today a slot is held only during a lookup. The store has
LMDB's default of 126 slots for all processes on the host, so a host with
many worker processes or large thread pools could run out and see lookups
fail. Adopting it would also need the daemon to create the store with more
slots.

## Measuring it yourself

With the daemon running, from the repository:

```sh
cargo build --release -p emfor-client --example bench
EMFOR_STORE=/var/lib/emfor/state.mdb \
  target/release/examples/bench web.svc.emfor --threads 8 --dns 127.0.0.1:5300
```

`--count N` sets the lookups per measurement (default 100000), `--threads T`
the threads of the throughput run (default 8), and `--dns ADDR` adds the DNS
comparison. The service must have a healthy backend, or the run stops at
the first error.
