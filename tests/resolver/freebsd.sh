#!/bin/sh
# Usage: tests/resolver/freebsd.sh   (FreeBSD, as root)
# Installs emfor with examples/rc.d/emfor, checks start/reload/stop, and
# checks the local_unbound recipe from docs/deploy/dns-resolver.md.
# Used by the CI freebsd job.
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
fail() { echo "FAIL: $*" >&2; exit 1; }

cargo build --bin emfor
install -m 0555 target/debug/emfor /usr/local/bin/emfor
install -m 0555 examples/rc.d/emfor /usr/local/etc/rc.d/emfor
mkdir -p /usr/local/etc/emfor /var/db/emfor
cat > /usr/local/etc/emfor/emfor.toml <<'CFG'
[store]
path = "/var/db/emfor/state.mdb"

[dns]
listen = ["127.0.0.1:5300"]

[services."api.svc.emfor"]
check = "tcp"
backends = ["127.0.0.1:8080"]
interval = "200ms"
timeout = "100ms"
rise = 1
CFG

nc -l -k 127.0.0.1 8080 >/dev/null 2>&1 &

# rc.d script: start, configtest, reload, restart after a crash, stop.
sysrc emfor_enable=YES
service emfor configtest
service emfor start
i=0
until drill -p 5300 api.svc.emfor @127.0.0.1 A | grep -q '127\.0\.0\.1$'; do
    i=$((i + 1)); [ "$i" -lt 50 ] || fail "emfor did not answer"
    sleep 0.2
done
service emfor reload
sleep 1
service emfor status || fail "emfor not running after reload"
# An invalid config makes reload fail and leaves the daemon alone.
cp /usr/local/etc/emfor/emfor.toml /tmp/emfor.toml.good
echo 'nonsense =' >> /usr/local/etc/emfor/emfor.toml
if service emfor reload; then fail "reload accepted an invalid config"; fi
cp /tmp/emfor.toml.good /usr/local/etc/emfor/emfor.toml
# daemon(8) starts emfor again after it dies.
first=$(cat /var/run/emfor.child.pid)
kill -KILL "$first"
i=0
until [ -s /var/run/emfor.child.pid ] && [ "$(cat /var/run/emfor.child.pid)" != "$first" ] &&
    pgrep -F /var/run/emfor.child.pid emfor >/dev/null; do
    i=$((i + 1)); [ "$i" -lt 50 ] || fail "emfor not restarted after it was killed"
    sleep 0.2
done

# local_unbound recipe.
sysrc local_unbound_enable=YES
service local_unbound start || service local_unbound restart
mkdir -p /var/unbound/conf.d
cp docs/deploy/resolver/unbound.conf /var/unbound/conf.d/emfor.conf
service local_unbound restart
i=0
until out=$(getent hosts api.svc.emfor); do
    i=$((i + 1)); [ "$i" -lt 50 ] || { cat /var/unbound/unbound.conf; fail "getent hosts api.svc.emfor failed"; }
    sleep 0.2
done
echo "$out" | grep -q '^127\.0\.0\.1 ' || fail "unexpected answer: $out"
drill api.svc.emfor @127.0.0.1 A

service emfor stop
if service emfor status; then fail "emfor still running after stop"; fi
if pgrep -x emfor; then fail "emfor left running after stop"; fi
echo "ok: rc.d script and local_unbound recipe ($out) on $(freebsd-version)"
