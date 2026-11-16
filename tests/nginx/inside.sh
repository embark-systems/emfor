#!/bin/sh
# Runs inside the test container: two backends (nginx on 9001 and 9002, each
# answering its port), emfor with the DNS server on 5300, and the nginx under
# test in front, configured as docs/manual/07-dns.md says.
set -u

fail() { echo "FAIL: $*" >&2; echo "--- emfor:" >&2; tail -5 /tmp/emfor.log >&2; echo "--- nginx:" >&2; tail -5 /tmp/ngx.err >&2; exit 1; }

echo "nginx: $(nginx -v 2>&1)"
mkdir -p /etc/emfor /var/lib/emfor

# A backend answers with its port; kill it by its pid file.
backend() {
    cat > "/tmp/b$1.conf" <<CFG
daemon off; pid /tmp/b$1.pid; error_log /tmp/b$1.err; events {}
http { access_log off; server { listen 127.0.0.1:$1; location / { return 200 "$1\n"; } } }
CFG
    nginx -c "/tmp/b$1.conf" &
}
backend 9001
backend 9002

# emfor: the services `api` (two ports) is rewritten between the steps.
config() { # <balance> <backend, ...>
    balance="$1"; shift
    list=$(printf '%s, ' "$@" | sed 's/, $//')
    cat > /etc/emfor/emfor.toml <<CFG
[dns]
listen = ["127.0.0.1:5300"]

[services."api.svc.emfor"]
check = "tcp"
backends = [$list]
balance = "$balance"
interval = "200ms"
timeout = "100ms"
rise = 1
fall = 1
jitter = "0%"
CFG
}
B1='"127.0.0.1:9001"'
B2='"127.0.0.1:9002"'
config round-robin "$B1" "$B2"
emfor server --config /etc/emfor/emfor.toml 2>/tmp/emfor.log &
EMFOR=$!
reload() { kill -HUP "$EMFOR"; }

# Requests to nginx, counted by answer: "9001=12 9002=8".
count() { # <n>
    i=0; a=0; b=0; other=0
    while [ "$i" -lt "$1" ]; do
        i=$((i + 1))
        case "$(curl -s -m 2 http://127.0.0.1:8080/)" in
            9001) a=$((a + 1)) ;;
            9002) b=$((b + 1)) ;;
            *) other=$((other + 1)) ;;
        esac
    done
    echo "9001=$a 9002=$b other=$other"
}
# Wait until `count 10` satisfies a shell test on a, b and other (10 s).
wait_for() { # <description> <test on $a $b $other>
    t=0
    while [ "$t" -lt 50 ]; do
        out=$(count 10); a=${out#9001=}; a=${a%% *}
        b=${out#*9002=}; b=${b%% *}; other=${out#*other=}
        if eval "$2"; then echo "ok: $1 ($out)"; return 0; fi
        t=$((t + 1)); sleep 0.2
    done
    fail "$1: $out"
}

# --- the recipe: the SRV upstream (nginx 1.27.3 or later)
cat > /etc/nginx/nginx.conf <<'CFG'
worker_processes 1;
error_log /tmp/ngx.err info;
pid /tmp/ngx.pid;
events {}
http {
    access_log off;
    resolver 127.0.0.1:5300 valid=1s ipv6=off;
    resolver_timeout 1s;
    upstream api {
        zone api 64k;
        server api.svc.emfor service=http resolve;
    }
    server {
        listen 8080;
        location / { proxy_pass http://api; }
    }
}
CFG
SRV_OK=1
if ! nginx -t >/tmp/nginx-t.log 2>&1; then
    SRV_OK=0
    grep -q 'invalid parameter "service=http"' /tmp/nginx-t.log || { cat /tmp/nginx-t.log; fail "nginx -t failed for a reason other than the version"; }
    echo "note: this nginx has no service= / resolve; checking the A record recipe only"
fi

if [ "$SRV_OK" = 1 ]; then
    nginx
    # emfor answers the SRV query of `service=http`.
    t=0
    until [ "$(dig +short -p 5300 @127.0.0.1 _http._tcp.api.svc.emfor SRV | wc -l)" -ge 2 ]; do
        t=$((t + 1)); [ "$t" -lt 50 ] || fail "emfor did not answer the SRV query"; sleep 0.2
    done
    wait_for "both backends get requests" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ] && [ "$other" = 0 ]'

    # A backend that fails its checks leaves the list; one that recovers returns.
    kill "$(cat /tmp/b9002.pid)"
    wait_for "a backend that is down gets nothing" '[ "$a" = 10 ]'
    backend 9002
    wait_for "the recovered backend gets requests again" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'

    # drain: emfor reloads, nginx is not touched.
    config round-robin "$B1" '{ addr = "127.0.0.1:9002", drain = true }'; reload
    wait_for "a drained backend gets nothing, nginx not reloaded" '[ "$a" = 10 ]'
    config round-robin "$B1" "$B2"; reload
    wait_for "an undrained backend gets requests again" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'

    # failover: the first backend, the second is a backup (priority 1).
    config failover "$B1" "$B2"; reload
    wait_for "failover: only the first backend while it is healthy" '[ "$a" = 10 ]'
    kill "$(cat /tmp/b9001.pid)"
    wait_for "failover: the second backend when the first is down" '[ "$b" = 10 ]'
    backend 9001
    wait_for "failover: the first backend again" '[ "$a" = 10 ]'

    # weights: nginx follows the SRV weights (3 to 1).
    config weighted '{ addr = "127.0.0.1:9001", weight = 3 }' '{ addr = "127.0.0.1:9002", weight = 1 }'; reload
    sleep 2
    out=$(count 80); a=${out#9001=}; a=${a%% *}
    [ "$a" -ge 50 ] && [ "$a" -le 70 ] || fail "weighted 3:1 gave $out"
    echo "ok: weights are followed ($out)"

    # emfor cannot answer (everything drained: SERVFAIL; or stopped): nginx keeps
    # the last list and logs it.
    config round-robin '{ addr = "127.0.0.1:9001", drain = true }' '{ addr = "127.0.0.1:9002", drain = true }'; reload
    sleep 3
    out=$(count 10)
    case "$out" in *"other=0") ;; *) fail "nginx stopped answering when emfor had nothing to say: $out" ;; esac
    grep -q 'could not be resolved' /tmp/ngx.err || fail "nginx did not log the failed resolution"
    echo "ok: nginx keeps its last list when emfor answers SERVFAIL ($out)"
    # emfor answers again (one backend): with resolver_timeout 1s nginx follows
    # within a few seconds ...
    config round-robin "$B1" '{ addr = "127.0.0.1:9002", drain = true }'; reload
    t=0
    while [ "$t" -lt 15 ]; do
        out=$(count 6); b=${out#*9002=}; b=${b%% *}
        [ "$b" = 0 ] && break
        t=$((t + 1)); sleep 1
    done
    [ "$t" -lt 15 ] || fail "nginx did not follow emfor in 15 seconds after a SERVFAIL with resolver_timeout 1s ($out)"
    echo "ok: with resolver_timeout 1s nginx follows emfor again after about $t seconds ($out)"
    # ... but with the default resolver_timeout (30 seconds) it waits that long
    # after the failed lookup
    grep -v 'resolver_timeout 1s;' /etc/nginx/nginx.conf > /tmp/nginx.conf.new && cat /tmp/nginx.conf.new > /etc/nginx/nginx.conf
    nginx -s stop; sleep 1; nginx; sleep 2
    config round-robin "$B1" "$B2"; reload; sleep 3
    config round-robin '{ addr = "127.0.0.1:9001", drain = true }' '{ addr = "127.0.0.1:9002", drain = true }'; reload
    sleep 4
    out=$(count 10)
    case "$out" in *"other=0") ;; *) fail "nginx stopped answering when emfor answered SERVFAIL (default resolver_timeout): $out" ;; esac
    config round-robin "$B1" '{ addr = "127.0.0.1:9002", drain = true }'; reload
    sleep 5
    out=$(count 20); b=${out#*9002=}; b=${b%% *}
    [ "$b" -ge 1 ] || fail "nginx followed emfor 5 seconds after a SERVFAIL with the default resolver_timeout ($out): the manual says it waits 30 seconds"
    echo "ok: with the default resolver_timeout nginx still has its old list 5 seconds after emfor answers again ($out)"
    t=0
    while [ "$t" -lt 45 ]; do
        out=$(count 6); b=${out#*9002=}; b=${b%% *}
        [ "$b" = 0 ] && break
        t=$((t + 1)); sleep 1
    done
    [ "$t" -lt 45 ] || fail "nginx did not follow emfor in 45 seconds after a SERVFAIL ($out)"
    echo "ok: with the default resolver_timeout nginx follows emfor again after about $((t + 5)) seconds ($out)"
    nginx -s stop; sleep 1
fi

# --- the fallback for an older nginx: one A record, the port in proxy_pass.
# Both backends listen on port 9001 of two addresses.
cat > /tmp/b9101.conf <<'CFG'
daemon off; pid /tmp/b9101.pid; error_log /tmp/b9101.err; events {}
http { access_log off; server { listen 127.0.0.2:9001; location / { return 200 "second\n"; } } }
CFG
cat > /tmp/b9100.conf <<'CFG'
daemon off; pid /tmp/b9100.pid; error_log /tmp/b9100.err; events {}
http { access_log off; server { listen 127.0.0.1:9001; location / { return 200 "first\n"; } } }
CFG
# the earlier backends hold 127.0.0.1:9001: stop everything and start over
for f in /tmp/b9001.pid /tmp/b9002.pid; do [ -s "$f" ] && kill "$(cat "$f")" 2>/dev/null; done
sleep 1
nginx -c /tmp/b9100.conf & nginx -c /tmp/b9101.conf &
config round-robin '"127.0.0.1:9001"' '"127.0.0.2:9001"'; reload
cat > /etc/nginx/nginx.conf <<'CFG'
worker_processes 1;
error_log /tmp/ngx.err info;
pid /tmp/ngx.pid;
events {}
http {
    access_log off;
    resolver 127.0.0.1:5300 valid=1s ipv6=off;
    server {
        listen 8080;
        location / {
            set $backend api.svc.emfor;
            proxy_pass http://$backend:9001;
        }
    }
}
CFG
nginx -t >/dev/null 2>&1 || { nginx -t; fail "the A record recipe does not load"; }
nginx
t=0; first=0; second=0
while [ "$t" -lt 30 ]; do
    case "$(curl -s -m 2 http://127.0.0.1:8080/)" in
        first) first=$((first + 1)) ;;
        second) second=$((second + 1)) ;;
    esac
    [ "$first" -ge 1 ] && [ "$second" -ge 1 ] && break
    t=$((t + 1)); sleep 0.3
done
[ "$first" -ge 1 ] && [ "$second" -ge 1 ] || fail "the A record recipe did not reach both backends (first=$first second=$second)"
echo "ok: the A record recipe reaches both backends (first=$first second=$second)"
echo "ok: tests/nginx"
