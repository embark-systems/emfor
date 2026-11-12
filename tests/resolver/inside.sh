#!/bin/sh
# Runs inside the test container: start a backend and emfor, configure the
# resolver exactly as docs/deploy/dns-resolver.md says, and resolve through it.
set -eu
resolver="$1"

fail() { echo "FAIL: $*" >&2; exit 1; }

# A TCP backend for emfor to health-check.
nc -lk 127.0.0.1 8080 >/dev/null 2>&1 &

mkdir -p /etc/emfor /var/lib/emfor
cat > /etc/emfor/emfor.toml <<'CFG'
[dns]
listen = ["127.0.0.1:5300"]

[services."api.svc.emfor"]
check = "tcp"
backends = ["127.0.0.1:8080"]
interval = "200ms"
timeout = "100ms"
rise = 1
CFG
emfor --config /etc/emfor/emfor.toml 2>/tmp/emfor.log &

case "$resolver" in
dnsmasq)
    cp /resolver/dnsmasq.conf /etc/dnsmasq.d/emfor.conf
    # No upstream inside the test container: only the emfor zone matters.
    dnsmasq --no-resolv --listen-address=127.0.0.1 --bind-interfaces \
        --conf-dir=/etc/dnsmasq.d --pid-file=/tmp/dnsmasq.pid
    ;;
unbound)
    cp /resolver/unbound.conf /etc/unbound/unbound.conf.d/emfor.conf
    # Debian's systemd unit creates the root trust anchor; do the same so
    # DNSSEC validation stays on and domain-insecure is really exercised.
    install -o unbound -m 0644 /usr/share/dns/root.key /var/lib/unbound/root.key
    unbound-checkconf
    unbound -d -c /etc/unbound/unbound.conf >/tmp/unbound.log 2>&1 &
    ;;
*) fail "unknown resolver $resolver" ;;
esac

echo "nameserver 127.0.0.1" > /etc/resolv.conf

# emfor directly: wait until the backend is healthy.
i=0
until dig +short @127.0.0.1 -p 5300 api.svc.emfor A | grep -qx 127.0.0.1; do
    i=$((i + 1)); [ "$i" -lt 50 ] || { cat /tmp/emfor.log; fail "emfor did not answer"; }
    sleep 0.2
done
dig @127.0.0.1 -p 5300 example.com A | grep -q 'status: REFUSED' \
    || fail "emfor answered a name outside its zone"

# Through the system resolver.
i=0
until out=$(getent hosts api.svc.emfor); do
    i=$((i + 1)); [ "$i" -lt 50 ] || { cat /tmp/*.log 2>/dev/null; fail "getent hosts api.svc.emfor failed"; }
    sleep 0.2
done
echo "$out" | grep -q '^127\.0\.0\.1 ' || fail "unexpected answer: $out"
if getent hosts nope.svc.emfor >/dev/null; then fail "nope.svc.emfor resolved"; fi

# SRV through the system resolver, and the target name it points to.
srv=$(dig +short _http._tcp.api.svc.emfor SRV)
[ "$srv" = "0 1 8080 127-0-0-1.api.svc.emfor." ] || fail "unexpected SRV answer: $srv"
target=$(dig +short 127-0-0-1.api.svc.emfor A)
[ "$target" = "127.0.0.1" ] || fail "unexpected target answer: $target"

echo "ok: $resolver forwards svc.emfor to emfor ($out; SRV $srv)"
