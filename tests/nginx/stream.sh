#!/bin/sh
# Runs inside the test container, after inside.sh: what the nginx section of
# docs/manual/07-dns.md says about the `stream` module (TCP). Two backends
# (nginx on 9001 and 9002, each answering its port over HTTP, which a TCP
# proxy passes on; /slow is a 1200 byte file sent at 200 bytes a second),
# emfor with the DNS server on 5300, and the nginx under test in front with
# the upstream of a `stream` block taken from emfor's SRV answers.
set -u

fail() {
    echo "FAIL: $*" >&2
    echo "--- emfor:" >&2; tail -5 /tmp/emfor.log >&2
    echo "--- nginx:" >&2; tail -5 /tmp/ngx.err >&2
    exit 1
}

echo "nginx: $(nginx -v 2>&1)"
mkdir -p /etc/emfor /var/lib/emfor
head -c 1200 /dev/zero | tr '\0' A > /tmp/slow-9001.txt
head -c 1200 /dev/zero | tr '\0' B > /tmp/slow-9002.txt

backend() { # <port>
    cat > "/tmp/b$1.conf" <<CFG
daemon off; pid /tmp/b$1.pid; error_log /tmp/b$1.err; events {}
http { access_log off; server { listen 127.0.0.1:$1;
  location / { return 200 "$1\n"; }
  location = /slow { alias /tmp/slow-$1.txt; limit_rate 200; } } }
CFG
    nginx -c "/tmp/b$1.conf" &
}
# stop a backend and wait until its master has gone: it removes its pid file
# when it exits, which would remove the file of a new one started at once
stop_backend() {
    p=$(cat "/tmp/b$1.pid")
    kill "$p"
    while kill -0 "$p" 2>/dev/null; do sleep 0.1; done
}
backend 9001
backend 9002

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

# the recipe of the manual: a stream block
cat > /etc/nginx/nginx.conf <<'CFG'
worker_processes 1;
error_log /tmp/ngx.err info;
pid /tmp/ngx.pid;
events {}
stream {
    resolver 127.0.0.1:5300 valid=1s ipv6=off;
    resolver_timeout 1s;
    upstream api {
        zone api 64k;
        server api.svc.emfor service=http resolve;
    }
    server {
        listen 8080;
        proxy_pass api;
    }
}
CFG
if ! nginx -t >/tmp/nginx-t.log 2>&1; then
    grep -q 'invalid parameter "service=http"' /tmp/nginx-t.log || { cat /tmp/nginx-t.log; fail "nginx -t failed for a reason other than the version"; }
    echo "note: this nginx has no service= / resolve (it needs 1.27.3); the stream recipe is not checked"
    exit 0
fi
nginx

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
field() { v=${2#*"$1"=}; echo "${v%% *}"; } # <name> <counts line>
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
# connections made for 2.5 seconds, 10 a second: how many got no answer
failures_in_window() {
    i=0; n=0
    while [ "$i" -lt 25 ]; do
        i=$((i + 1))
        r=$(curl -s -m 3 -w '|rc=%{exitcode}' http://127.0.0.1:8080/ 2>&1)
        case "$r" in 9001* | 9002*) ;; *) n=$((n + 1)); echo "note: connection $i of the window: '$r'" >&2 ;; esac
        sleep 0.1
    done
    echo "$n"
}

t=0
until [ "$(count 2)" != "9001=0 9002=0 other=2" ]; do
    t=$((t + 1)); [ "$t" -lt 50 ] || fail "the stream proxy never answered"; sleep 0.2
done
wait_for "stream: both backends get connections" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ] && [ "$other" = 0 ]'

# health: a stopped backend leaves the list; nginx retries a connection that
# a stopped backend refused (proxy_next_upstream is on by default in stream)
stop_backend 9002
n=$(failures_in_window)
[ "$n" -le 1 ] || fail "$n connections failed while a backend was down"
wait_for "stream: a backend that is down gets nothing" '[ "$a" = 10 ]'
echo "ok: stream: $n of 25 connections failed when a backend went down (nginx tries the next server)"
backend 9002
wait_for "stream: the recovered backend gets connections again" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'

# failover puts every new connection on the first backend, so the slow
# request below is on a known one
config failover "$B1" "$B2"; reload
wait_for "stream failover: only the first backend while it is healthy" '[ "$a" = 10 ]'

# slow <out file>: a request that takes about 6 seconds, through the proxy,
# on 9001; a second for it to be under way
slow() {
    rm -f "$1"
    curl -s -m 30 -o "$1" http://127.0.0.1:8080/slow &
    SLOW=$!
    sleep 1
    kill -0 "$SLOW" 2>/dev/null || fail "the slow request ended at once"
}

# a connection in flight when its backend is stopped is cut
slow /tmp/s1.out
stop_backend 9001
wait "$SLOW" 2>/dev/null
size=$(wc -c < /tmp/s1.out 2>/dev/null | tr -d " "); size=${size:-0} # no file: no byte arrived before the cut
[ "$size" -lt 1200 ] || fail "stopping the backend that had the connection did not cut it ($size bytes)"
echo "ok: stream: stopping a backend cuts the connection in flight ($size of 1200 bytes)"
wait_for "stream failover: the second backend when the first is down" '[ "$b" = 10 ]'
backend 9001
wait_for "stream failover: the first backend again" '[ "$a" = 10 ]'

# drain: a connection already open to the drained backend is not cut, and
# new connections go to the other one
slow /tmp/s2.out
config failover '{ addr = "127.0.0.1:9001", drain = true }' "$B2"; reload
wait_for "stream: a drained backend gets no new connection, nginx not reloaded" '[ "$b" = 10 ]'
kill -0 "$SLOW" 2>/dev/null || fail "the slow request ended too early (timing)"
wait "$SLOW" 2>/dev/null
size=$(wc -c < /tmp/s2.out | tr -d " ")
[ "$size" = 1200 ] || fail "the connection in flight got $size bytes after a drain, not 1200"
echo "ok: stream: a connection already open when its backend is drained finishes (1200 bytes)"
config failover "$B1" "$B2"; reload
wait_for "stream: an undrained backend gets connections again" '[ "$a" = 10 ]'

# weights 3 to 1
config weighted '{ addr = "127.0.0.1:9001", weight = 3 }' '{ addr = "127.0.0.1:9002", weight = 1 }'; reload
sleep 2
out=$(count 80); a=${out#9001=}; a=${a%% *}
[ "$a" -ge 50 ] && [ "$a" -le 70 ] || fail "stream weighted 3:1 gave $out"
echo "ok: stream: weights are followed ($out)"

# emfor has nothing to answer (every backend drained: SERVFAIL): nginx keeps
# its last list ...
config round-robin '{ addr = "127.0.0.1:9001", drain = true }' '{ addr = "127.0.0.1:9002", drain = true }'; reload
sleep 4
out=$(count 10)
[ "${out#*other=}" = 0 ] || fail "nginx stopped answering when emfor answered SERVFAIL: $out"
echo "ok: stream: nginx keeps its last list when emfor answers SERVFAIL ($out)"
# ... and when emfor answers again (one backend), nginx follows within a few
# seconds because of resolver_timeout 1s (the default, 30 seconds, is how long
# nginx waits after a failed lookup: tests/nginx/inside.sh)
config round-robin "$B1" '{ addr = "127.0.0.1:9002", drain = true }'; reload
t=0
while [ "$t" -lt 15 ]; do
    out=$(count 6)
    [ "$(field 9002 "$out")" = 0 ] && break
    t=$((t + 1)); sleep 1
done
[ "$t" -lt 15 ] || fail "nginx did not follow emfor in 15 seconds after a SERVFAIL with resolver_timeout 1s ($out)"
echo "ok: stream: with resolver_timeout 1s nginx follows emfor again after about $t seconds ($out)"

echo "ok: tests/nginx (stream)"
