#!/bin/sh
# The recipe of chapter 12, "Metrics over TLS": emfor serves /metrics on
# loopback and nginx in front of it terminates TLS and asks the scraper for a
# client certificate. One container runs both, with a throwaway CA.
#
#   tools/metrics-proxy-test.sh
#
# Needs docker. The daemon is built in the test image (tools/docker-test.sh).
#
#   1  emfor listens on loopback only, nginx on all addresses
#   2  a scraper with a certificate of the CA gets the metrics over TLS
#   3  no client certificate, or one of another CA, gets no metrics
#   4  plain HTTP to the TLS port gets no metrics, other paths give 404
#   5  a new server certificate is picked up by `nginx -s reload`; emfor keeps
#      running and keeps answering
#
# Nothing here is run by the CI or by tools/docker-test.sh.
set -eu
cd "$(dirname "$0")/.."

image=emfor-metrics-proxy-test

# The daemon, in the volume of the test image.
tools/docker-test.sh 'cargo build -p emfor-daemon 2>&1 | tail -1' >/dev/null
docker image inspect "$image" >/dev/null 2>&1 || docker build -q -t "$image" - >/dev/null <<'EOF'
FROM debian:trixie-slim
RUN apt-get update -qq && apt-get install -y -qq nginx-light openssl curl iproute2 procps >/dev/null
EOF

exec docker run -i --rm --init -v emfor-clients-target:/t:ro "$image" sh -s <<'INSIDE'
set -u
failed=0
ok() { echo "ok: $*"; }
fail() { echo "FAIL: $*"; failed=1; }
step() { echo; echo "== $*"; }

mkdir -p /etc/emfor /var/lib/emfor /etc/proxy
cat > /etc/emfor/emfor.toml <<'CFG'
[store]
path = "/var/lib/emfor/state.mdb"
[metrics]
listen = ["127.0.0.1:5301"]
# The proxy is the only source emfor sees, so it is the only one to allow.
allow = ["127.0.0.1/32"]
[services."app.svc.emfor"]
check = "tcp"
backends = ["127.0.0.1:8080"]
CFG
chmod 644 /etc/emfor/emfor.toml
/t/debug/emfor server --config /etc/emfor/emfor.toml >/tmp/emfor.log 2>&1 &
emfor_pid=$!

cd /etc/proxy
# mkca NAME; mkcert NAME CA CN [EXTRA]: a CA, and a certificate signed by it.
mkca() {
    openssl req -x509 -newkey rsa:2048 -nodes -keyout "$1.key" -out "$1.pem" \
        -subj "/CN=$1" -days 2 2>/dev/null
}
mkcert() {
    openssl req -newkey rsa:2048 -nodes -keyout "$1.key" -out "$1.csr" \
        -subj "/CN=$3" 2>/dev/null
    printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\n' > "$1.ext"
    openssl x509 -req -in "$1.csr" -CA "$2.pem" -CAkey "$2.key" \
        -set_serial "${4:-$(date +%s%N | cut -c1-15)}" -out "$1.pem" -days 2 \
        -extfile "$1.ext" 2>/dev/null
}
mkca ca
mkca otherca
mkcert server ca localhost 1001
mkcert scraper ca prometheus
mkcert stranger otherca prometheus
chmod 600 ./*.key

cat > /etc/proxy/nginx.conf <<'CFG'
pid /tmp/nginx.pid;
error_log /tmp/nginx-error.log;
events {}
http {
    access_log off;
    server {
        listen 8443 ssl;
        ssl_certificate /etc/proxy/server.pem;
        ssl_certificate_key /etc/proxy/server.key;
        ssl_protocols TLSv1.2 TLSv1.3;
        # Only scrapers with a certificate of this CA.
        ssl_client_certificate /etc/proxy/ca.pem;
        ssl_verify_client on;
        location = /metrics {
            proxy_pass http://127.0.0.1:5301/metrics;
        }
        location / { return 404; }
    }
}
CFG
nginx -c /etc/proxy/nginx.conf

# Wait for the first metrics from emfor itself.
t=0
until [ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:5301/metrics)" = 200 ]; do
    t=$((t + 1)); [ "$t" -lt 100 ] || { echo "emfor gave no metrics"; cat /tmp/emfor.log; exit 1; }
    sleep 0.2
done

scrape() { # <extra curl args>: the status code of a scrape over TLS
    curl -s -o /tmp/body -w '%{http_code}' --connect-timeout 5 "$@" 2>/dev/null || :
}
good="--cacert /etc/proxy/ca.pem --cert /etc/proxy/scraper.pem --key /etc/proxy/scraper.key"

step "1  who listens where"
ss -ltnH | awk '{print $4}' > /tmp/listen
grep -q '^127.0.0.1:5301$' /tmp/listen && ok "emfor listens on 127.0.0.1:5301" \
    || fail "emfor is not on 127.0.0.1:5301: $(cat /tmp/listen)"
grep -qE '^(0.0.0.0|\*):8443$' /tmp/listen && ok "nginx listens on port 8443 on every address" \
    || fail "nginx is not on 8443: $(cat /tmp/listen)"
grep -qE '^(0.0.0.0|\*|\[::\]):5301$' /tmp/listen \
    && fail "emfor listens beyond loopback" || ok "nothing else exposes 5301"

step "2  a scraper with a certificate of the CA"
c=$(scrape $good https://localhost:8443/metrics)
[ "$c" = 200 ] && grep -q '^emfor_' /tmp/body && ok "200 with emfor metrics over TLS" \
    || fail "status $c: $(head -c 200 /tmp/body)"

step "3  no certificate, or a certificate of another CA"
c=$(scrape --cacert /etc/proxy/ca.pem https://localhost:8443/metrics)
[ "$c" != 200 ] && ! grep -q '^emfor_' /tmp/body && ok "no client certificate: status $c, no metrics" \
    || fail "no client certificate got status $c"
c=$(scrape --cacert /etc/proxy/ca.pem --cert /etc/proxy/stranger.pem --key /etc/proxy/stranger.key \
    https://localhost:8443/metrics)
[ "$c" != 200 ] && ! grep -q '^emfor_' /tmp/body && ok "certificate of another CA: status $c, no metrics" \
    || fail "certificate of another CA got status $c"
c=$(scrape --cert /etc/proxy/scraper.pem --key /etc/proxy/scraper.key https://localhost:8443/metrics)
[ "$c" != 200 ] && ok "a scraper that does not trust the server's CA does not connect" \
    || fail "an unverified server was accepted: status $c"

step "4  plain HTTP and other paths"
c=$(scrape http://127.0.0.1:8443/metrics)
[ "$c" != 200 ] && ! grep -q '^emfor_' /tmp/body && ok "plain HTTP to the TLS port: status $c, no metrics" \
    || fail "plain HTTP got status $c"
c=$(scrape $good https://localhost:8443/other)
[ "$c" = 404 ] && ok "another path gives 404" || fail "another path gave $c"

step "5  a new server certificate"
serial() {
    openssl s_client -connect 127.0.0.1:8443 -cert /etc/proxy/scraper.pem -key /etc/proxy/scraper.key \
        -CAfile /etc/proxy/ca.pem </dev/null 2>/dev/null | openssl x509 -noout -serial
}
before=$(serial)
mkcert server2 ca localhost 2002
cp server2.pem server.pem; cp server2.key server.key
nginx -c /etc/proxy/nginx.conf -s reload
sleep 1
after=$(serial)
[ -n "$after" ] && [ "$before" != "$after" ] && ok "served certificate changed ($before -> $after)" \
    || fail "certificate not renewed: '$before' -> '$after'"
c=$(scrape $good https://localhost:8443/metrics)
[ "$c" = 200 ] && ok "the scraper still gets metrics" || fail "after the reload: status $c"
kill -0 "$emfor_pid" 2>/dev/null && ok "emfor kept running (pid $emfor_pid)" || fail "emfor stopped"
grep -q "ERROR" /tmp/emfor.log && fail "emfor logged an error: $(grep ERROR /tmp/emfor.log | head -2)" || :

echo
[ "$failed" = 0 ] && echo "all checks passed" || { echo "SOME CHECKS FAILED"; exit 1; }
INSIDE
