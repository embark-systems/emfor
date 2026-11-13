#!/bin/sh
# Usage: tests/spike/run.sh [1|2]
# Starts emfor with a throwaway store, then runs a compatibility spike
# against it and prints a PASS or FAIL line per check; exits non-zero if any
# check fails. See tests/spike/RESULTS.md.
#   1: C with the system liblmdb (pkg-config lmdb) and PHP dba "lmdb".
#      Needs: cc, pkg-config, liblmdb headers, php with dba.
#   2 (default): the cargo-c probe library (tests/spike/probe) from C,
#      Python ctypes and PHP FFI, plus an exported-symbol check.
#      Needs: cargo-c, cc, pkg-config, python3, php with FFI (and dba with
#      the lmdb handler for the same-process check).
set -u
spike="${1:-2}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
work="$(mktemp -d)"
pid=""
trap '[ -n "$pid" ] && kill "$pid" 2>/dev/null; rm -rf "$work"' EXIT

uname -a
cargo build --quiet --bin emfor || exit 1
case "$spike" in
1)
    cc -Wall -Wextra -Werror -o "$work/lmdb_read" tests/spike/lmdb_read.c \
        $(pkg-config --cflags --libs lmdb) || exit 1
    ;;
2)
    (cd tests/spike/probe && cargo cinstall --release --features capi \
        --prefix "$work/prefix" --libdir "$work/prefix/lib" >/dev/null 2>&1) || exit 1
    export PKG_CONFIG_PATH="$work/prefix/lib/pkgconfig"
    libdir=$(pkg-config --variable=libdir emforprobe)
    if [ "$(uname)" = Darwin ]; then so="$libdir/libemforprobe.dylib"; else so="$libdir/libemforprobe.so"; fi
    cc -Wall -Wextra -Werror -o "$work/probe_read" tests/spike/probe_read.c \
        $(pkg-config --cflags --libs emforprobe) -Wl,-rpath,"$libdir" || exit 1
    ;;
*) echo "unknown spike $spike"; exit 2 ;;
esac

cat > "$work/emfor.toml" <<CFG
[store]
path = "$work/state.mdb"

[services."api.svc.emfor"]
check = "tcp"
backends = ["127.0.0.1:9"]
interval = "200ms"
timeout = "100ms"
CFG
target/debug/emfor server --config "$work/emfor.toml" >"$work/emfor.log" 2>&1 &
pid=$!
i=0
until [ -s "$work/state.mdb" ]; do
    i=$((i + 1))
    [ "$i" -lt 50 ] || { cat "$work/emfor.log"; echo "FAIL emfor did not create the store"; exit 1; }
    sleep 0.2
done
sleep 1

status=0
if [ "$spike" = 1 ]; then
    echo "== C (system liblmdb $(pkg-config --modversion lmdb))"
    "$work/lmdb_read" "$work/state.mdb" || status=1
    echo "== PHP dba"
    if command -v php >/dev/null; then
        php tests/spike/dba_read.php "$work/state.mdb" || status=1
    else
        echo "FAIL php not installed"; status=1
    fi
else
    echo "== C (probe library via pkg-config)"
    "$work/probe_read" "$work/state.mdb" || status=1
    echo "== Python ctypes"
    python3 tests/spike/probe_read.py "$so" "$work/state.mdb" || status=1
    echo "== PHP FFI"
    if command -v php >/dev/null; then
        php tests/spike/probe_read.php "$so" "$work/state.mdb" "$work" || status=1
    else
        echo "FAIL php not installed"; status=1
    fi
    echo "== Exported symbols"
    tests/spike/check_symbols.sh "$so" probe_ || status=1
fi
exit $status
