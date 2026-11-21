#!/bin/sh
# Runs inside the test container: two backends (nginx on 9001 and 9002, each
# answering its port; /slow is a 1200 byte file sent at 200 bytes a second),
# emfor with its DNS server on 5300 and the service in a conf.d file, and the
# nginx under test in front (the recipe of docs/manual/14-integrations.md). A client
# sends a POST every 50 ms all the time. First a backend is stopped without
# drain while a slow request is in flight; then both backends are deployed in
# turn with examples/deploy/emfor-drain.sh. This test config uses a 200 ms
# interval and fall = 1, which makes a stopped backend show unhealthy within
# a fraction of a second; the last case therefore runs the defaults (2 s,
# fall = 3), where a backend that has just stopped still shows healthy for
# about 6 seconds, and checks that wait-ready does not take that for ready.
set -u

FILE=/etc/emfor/conf.d/20-api.toml
DRAIN=/usr/local/bin/emfor-drain.sh
export EMFOR_RELOAD='kill -HUP $(cat /tmp/emfor.pid)'
# The container has no emfor user and runs the daemon as root, so check-config
# runs as root here too (the scripts default to the daemon user "emfor").
export EMFOR_USER=root

fail() {
    echo "FAIL: $*" >&2
    echo "--- emfor:" >&2; tail -8 /tmp/emfor.log >&2
    echo "--- nginx:" >&2; tail -5 /tmp/ngx.err >&2
    echo "--- connections:" >&2; ss -tn state established 2>&1 | head -12 >&2
    exit 1
}

echo "nginx: $(nginx -v 2>&1)"
mkdir -p /etc/emfor/conf.d /var/lib/emfor
head -c 1200 /dev/zero | tr '\0' x > /tmp/slow.txt

backend() { # <port>
    cat > "/tmp/b$1.conf" <<CFG
daemon off; pid /tmp/b$1.pid; error_log /tmp/b$1.err; events {}
http {
    access_log off;
    server {
        listen 127.0.0.1:$1;
        add_header X-Port $1 always;
        location / { return 200 "$1\n"; }
        location = /slow { alias /tmp/slow.txt; limit_rate 200; }
    }
}
CFG
    nginx -c "/tmp/b$1.conf" &
}
stop_backend() { kill "$(cat "/tmp/b$1.pid")"; }
backend 9001
backend 9002

cat > /etc/emfor/emfor.toml <<'CFG'
include_dir = "conf.d"

[dns]
listen = ["127.0.0.1:5300"]
CFG
cat > "$FILE" <<'CFG'
[services."api.svc.emfor"]
check = "tcp"
balance = "round-robin"
interval = "200ms"
timeout = "100ms"
rise = 1
fall = 1
jitter = "0%"
backends = [
  { addr = "127.0.0.1:9001" },
  { addr = "127.0.0.1:9002" },
]
CFG
chmod 644 "$FILE"
emfor check-config >/dev/null 2>&1 || fail "check-config refuses the test config"
emfor server --config /etc/emfor/emfor.toml 2>/tmp/emfor.log &
echo $! > /tmp/emfor.pid

cat > /etc/nginx/nginx.conf <<'CFG'
worker_processes 1;
error_log /tmp/ngx.err info;
pid /tmp/ngx.pid;
events {}
http {
    access_log off;
    resolver 127.0.0.1:5300 valid=1s ipv6=off;
    upstream api {
        zone api 64k;
        server api.svc.emfor service=http resolve;
    }
    server {
        listen 8080;
        location / { proxy_pass http://api; proxy_buffering off; }
    }
}
CFG
nginx -t >/tmp/nginx-t.log 2>&1 || { cat /tmp/nginx-t.log; fail "nginx -t (the recipe needs nginx 1.27.3 or later)"; }
nginx

# The load: a POST every 50 ms; a line "<port> <http code>" for each.
load_start() { # <log>
    : > "$1"
    (
        while :; do
            out=$(curl -s -m 3 -d x -w ' %{http_code}' http://127.0.0.1:8080/ | tr -d '\n')
            echo "$out" >> "$1"
            sleep 0.05
        done
    ) &
    LOAD=$!
}
load_stop() { kill "$LOAD" 2>/dev/null; wait "$LOAD" 2>/dev/null; true; }
bad() { grep -vc ' 200$' "$1"; true; } # lines that are not "<port> 200"
seen() { # <log> <port> <lines to look at from the end>
    tail -n "$3" "$1" | grep -c "^$2 200$"
}
wait_both() { # <log>: both backends answer in the last 20 requests
    t=0
    while [ "$t" -lt 100 ]; do
        [ "$(seen "$1" 9001 20)" -ge 1 ] && [ "$(seen "$1" 9002 20)" -ge 1 ] && return 0
        t=$((t + 1)); sleep 0.2
    done
    fail "$1: not both backends answer in the last 20 requests"
}

# A request in flight: it takes about 6 seconds. Sets X, the port of the
# backend that answers it, and Y, the other one.
slow_start() {
    rm -f /tmp/slow.h /tmp/slow.out
    curl -s -m 30 -D /tmp/slow.h -o /tmp/slow.out http://127.0.0.1:8080/slow &
    SLOW=$!
    t=0
    until grep -qi '^x-port:' /tmp/slow.h 2>/dev/null; do
        t=$((t + 1)); [ "$t" -lt 50 ] || fail "the slow request got no answer"; sleep 0.1
    done
    X=$(tr -d '\r' < /tmp/slow.h | awk 'tolower($1) == "x-port:" {print $2}')
    case "$X" in 9001) Y=9002 ;; 9002) Y=9001 ;; *) fail "the slow request came from port '$X'" ;; esac
}

# --- control: a backend stopped without drain cuts the request in flight
load_start /tmp/load1.log
wait_both /tmp/load1.log
slow_start
stop_backend "$X"
wait "$SLOW" 2>/dev/null
size=$(wc -c < /tmp/slow.out | tr -d " ")
[ "$size" -lt 1200 ] || fail "stopping a backend without drain did not cut the request in flight ($size bytes): the manual's claim is wrong"
sleep 3
load_stop
n=$(bad /tmp/load1.log)
echo "ok: without drain, stopping the backend cut the request in flight ($size of 1200 bytes)"
echo "note: of the $(wc -l < /tmp/load1.log | tr -d " ") new POSTs sent meanwhile, $n failed (nginx retries a request that was refused before it was sent)"
backend "$X"
$DRAIN wait-ready "127.0.0.1:$X" 30 2 || fail "$X did not turn healthy"

# --- the deploy: drain, wait, stop, start, wait, undrain; once per backend
load_start /tmp/load2.log
wait_both /tmp/load2.log

slow_start

for round in 1 2; do
    if [ "$round" = 1 ]; then P=$X; else P=$Y; fi
    ADDR=127.0.0.1:$P
    $DRAIN on "$FILE" "$ADDR" || fail "drain on $ADDR"
    grep -q "addr = \"$ADDR\", drain = true" "$FILE" || fail "the file does not show drain for $ADDR"
    if [ "$round" = 1 ]; then
        kill -0 "$SLOW" 2>/dev/null || fail "the slow request ended before the drain was in place (timing)"
        echo "ok: the request in flight to $ADDR is still running after the drain"
    fi
    $DRAIN wait-idle "$P" 60 || fail "wait-idle on $P"
    open=$(ss -Htn state established "( sport = :$P )" | wc -l)
    echo "note: after wait-idle $open connections to $P are still open (idle, kept by nginx)"
    if [ "$round" = 1 ]; then
        kill -0 "$SLOW" 2>/dev/null && fail "wait-idle returned while the request was still running"
        wait "$SLOW"
        size=$(wc -c < /tmp/slow.out | tr -d " ")
        [ "$size" = 1200 ] || fail "the request in flight got $size bytes, not 1200"
        grep -q '^HTTP/1.1 200' /tmp/slow.h || fail "the request in flight did not get 200"
        echo "ok: the request in flight finished with all 1200 bytes; wait-idle returned after it"
    fi
    mark=$(wc -l < /tmp/load2.log | tr -d " ")
    sleep 2
    [ "$(tail -n +"$((mark + 1))" /tmp/load2.log | grep -c "^$P ")" = 0 ] \
        || fail "$ADDR still got requests after wait-idle"
    echo "ok: $ADDR gets no new requests after wait-idle"
    # The deploy: the backend is down and up again; the others serve.
    stop_backend "$P"
    sleep 3
    backend "$P"
    $DRAIN wait-ready "$ADDR" 30 2 || fail "$ADDR did not turn healthy"
    $DRAIN off "$FILE" "$ADDR" || fail "drain off $ADDR"
    ! grep -q drain "$FILE" || fail "the file still has drain after off"
    wait_both /tmp/load2.log
    echo "ok: $ADDR deployed and back in rotation"
done
load_stop
n=$(bad /tmp/load2.log)
total=$(wc -l < /tmp/load2.log | tr -d " ")
[ "$total" -ge 100 ] || fail "only $total requests were sent"
[ "$n" = 0 ] || { grep -v ' 200$' /tmp/load2.log | sort | uniq -c >&2; fail "$n of $total requests failed during the deploy"; }
echo "ok: no request failed during the rolling deploy ($total requests)"

# --- wait-ready with the default timings (interval 2s, fall 3, rise 2)
cat > "$FILE" <<'CFG'
[services."api.svc.emfor"]
check = "tcp"
backends = [
  { addr = "127.0.0.1:9001" },
  { addr = "127.0.0.1:9002" },
]
CFG
chmod 644 "$FILE"
sh -c "$EMFOR_RELOAD"
sleep 1
$DRAIN wait-ready 127.0.0.1:9001 60 || fail "default timings: 9001 did not turn healthy"
$DRAIN on "$FILE" 127.0.0.1:9001 || fail "default timings: drain on"
stop_backend 9001
t0=$(date +%s)
# The stale state this case is about: the backend is gone, the checks still
# say healthy.
st=$(emfor status --json | jq -r '.services[0].backends[] | select(.id == "127.0.0.1:9001") | .checked_state')
[ "$st" = healthy ] || fail "default timings: expected the checks to still say healthy right after the stop, got $st"
$DRAIN wait-ready 127.0.0.1:9001 90 &
W=$!
sleep 12
kill -0 "$W" 2>/dev/null || fail "default timings: wait-ready returned while the backend was down"
backend 9001
t1=$(date +%s)
wait "$W" || fail "default timings: wait-ready did not return after the backend was up"
t2=$(date +%s)
[ "$t2" -gt "$t1" ] || fail "default timings: wait-ready returned before the backend was started again"
echo "ok: default timings: wait-ready waited $((t2 - t0)) s after the stop, and returned only after the restart"
$DRAIN off "$FILE" 127.0.0.1:9001 || fail "default timings: drain off"

# --- refusing a line it cannot change, and a failed check-config
cp "$FILE" /tmp/keep.toml
sed 's/{ addr = "127.0.0.1:9002" }/{ addr = "127.0.0.1:9002"}/' /tmp/keep.toml > "$FILE"
cp "$FILE" /tmp/odd.toml
if $DRAIN on "$FILE" 127.0.0.1:9002 2>/dev/null; then fail "the script changed a line it cannot change"; fi
cmp -s "$FILE" /tmp/odd.toml || fail "the script touched a file it refused"
echo "ok: a backend line without the space before } is refused and the file is untouched"
cp /tmp/keep.toml "$FILE"
sed 's/^check = "tcp"/check = "nonsense"/' /tmp/keep.toml > "$FILE"
cp "$FILE" /tmp/bad.toml
if $DRAIN on "$FILE" 127.0.0.1:9002 2>/dev/null; then fail "the script accepted a config that check-config refuses"; fi
cmp -s "$FILE" /tmp/bad.toml || fail "the old file was not put back after check-config refused"
echo "ok: when check-config refuses, the old file is put back"
cp /tmp/keep.toml "$FILE"

echo "ok: tests/deploy"
