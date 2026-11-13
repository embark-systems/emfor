#!/bin/sh
# Usage: tests/resolver/resolved.sh   (Linux with systemd-resolved, needs sudo)
# Checks the systemd-resolved recipe from docs/deploy/dns-resolver.md on the
# host itself. Used by the CI linux job on the GitHub Actions Ubuntu runner.
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
fail() { echo "FAIL: $*" >&2; exit 1; }

echo "systemd version: $(systemctl --version | head -1)"
cargo build --bin emfor

tmp="$(mktemp -d)"
nc -lk 127.0.0.1 8080 >/dev/null 2>&1 &
backend=$!
cat > "$tmp/emfor.toml" <<CFG
[store]
path = "$tmp/state.mdb"

[dns]
listen = ["127.0.0.1:5300"]

[services."api.svc.emfor"]
check = "tcp"
backends = ["127.0.0.1:8080"]
interval = "200ms"
timeout = "100ms"
rise = 1
CFG
target/debug/emfor server --config "$tmp/emfor.toml" 2>"$tmp/emfor.log" &
emfor=$!
cleanup() {
    kill "$emfor" "$backend" 2>/dev/null || true
    sudo rm -f /etc/systemd/resolved.conf.d/emfor.conf
    sudo systemctl restart systemd-resolved || true
}
trap cleanup EXIT

i=0
until dig +short @127.0.0.1 -p 5300 api.svc.emfor A | grep -qx 127.0.0.1; do
    i=$((i + 1)); [ "$i" -lt 50 ] || { cat "$tmp/emfor.log"; fail "emfor did not answer"; }
    sleep 0.2
done

sudo install -D -m 0644 docs/deploy/resolver/resolved.conf /etc/systemd/resolved.conf.d/emfor.conf
sudo systemctl restart systemd-resolved

i=0
until out=$(getent hosts api.svc.emfor); do
    i=$((i + 1)); [ "$i" -lt 50 ] || { resolvectl status || true; fail "getent hosts api.svc.emfor failed"; }
    sleep 0.2
done
echo "$out" | grep -q '^127\.0\.0\.1 ' || fail "unexpected answer: $out"
resolvectl query api.svc.emfor
# SRV through systemd-resolved, as docs/deploy/dns-resolver.md shows it.
srv=$(resolvectl service _http._tcp api.svc.emfor) || fail "resolvectl service failed"
echo "$srv"
echo "$srv" | grep -q '127-0-0-1.api.svc.emfor' || fail "unexpected SRV answer: $srv"
echo "ok: systemd-resolved forwards svc.emfor to emfor ($out)"
