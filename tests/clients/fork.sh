#!/bin/sh
# Usage: tests/clients/fork.sh   (needs cargo, cargo-c, pkg-config and cc)
# Builds libemfor and runs the C fork tests against it: a child forked while
# other threads look up must finish its lookup (spec client-lookup "Clients
# are safe across threads and fork"). `make -C clients test` runs the same
# tests with GNU make; this script is for hosts without it (the CI freebsd
# job, where FreeBSD's make is BSD make) and runs on any POSIX system.
# Everything goes under clients/build/fork, which is rebuilt each time.
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
work="$root/clients/build/fork"
mkdir -p "$work"

# libemfor, installed to a prefix of its own.
(cd clients/rust && cargo cinstall --release --features capi \
    --prefix "$work/prefix" --libdir "$work/prefix/lib" \
    --pkgconfigdir "$work/prefix/lib/pkgconfig" >/dev/null)
PKG_CONFIG_PATH="$work/prefix/lib/pkgconfig"
export PKG_CONFIG_PATH

# A fresh store: its heartbeat is the time it was written.
cargo run -q -p emfor-fixture --bin emfor-fixture -- \
    "$root/clients/c/tests/fixture.json" 0 "$work/state.mdb"
EMFOR_STORE="$work/state.mdb"
EMFOR_MAX_STALENESS=60s
export EMFOR_STORE EMFOR_MAX_STALENESS

# spectra-scenario: client-lookup / Fork while other threads look up through the C library
for t in test_fork test_fork_threads; do
    # The flags of clients/c/Makefile.
    ${CC:-cc} -O2 -std=c11 -D_POSIX_C_SOURCE=200809L -Wall -Wextra -Werror \
        $(pkg-config --cflags emfor) "clients/c/tests/$t.c" -o "$work/$t" \
        $(pkg-config --libs emfor) -Wl,-rpath,"$work/prefix/lib" -lpthread
    "$work/$t"
done
echo "PASS C fork tests"
