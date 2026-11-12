#!/bin/sh
# Usage: tests/spike/run.sh
# Starts emfor with a throwaway store, then checks that C (system liblmdb,
# via pkg-config) and PHP (dba "lmdb" handler) can read it read-only.
# Needs: cargo, cc, pkg-config, liblmdb headers, php with dba.
# Prints a PASS or FAIL line per check; exits non-zero if either fails.
set -u
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
work="$(mktemp -d)"
trap 'kill "$pid" 2>/dev/null; rm -rf "$work"' EXIT

uname -a
cargo build --quiet --bin emfor || exit 1
cc -Wall -Wextra -Werror -o "$work/lmdb_read" tests/spike/lmdb_read.c \
    $(pkg-config --cflags --libs lmdb) || exit 1

cat > "$work/emfor.toml" <<CFG
[store]
path = "$work/state.mdb"

[services."api.svc.emfor"]
check = "tcp"
backends = ["127.0.0.1:9"]
interval = "200ms"
timeout = "100ms"
CFG
target/debug/emfor --config "$work/emfor.toml" >"$work/emfor.log" 2>&1 &
pid=$!
i=0
until [ -s "$work/state.mdb" ]; do
    i=$((i + 1))
    [ "$i" -lt 50 ] || { cat "$work/emfor.log"; echo "FAIL emfor did not create the store"; exit 1; }
    sleep 0.2
done
sleep 1

status=0
echo "== C (system liblmdb $(pkg-config --modversion lmdb))"
"$work/lmdb_read" "$work/state.mdb" || status=1
echo "== PHP dba"
if command -v php >/dev/null; then
    php tests/spike/dba_read.php "$work/state.mdb" || status=1
else
    echo "FAIL php not installed"; status=1
fi
exit $status
