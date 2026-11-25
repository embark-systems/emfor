#!/bin/sh
# What the daemon costs with many services and backends, and how big its store
# gets (the numbers behind the map_size rule of chapter 12).
#
#   tools/scale-test.sh [SERVICESxBACKENDS ...]      default: 10x5 40x10 80x25 80x50
#
# Needs docker. The release daemon is built in the test image
# (tools/docker-test.sh) and runs in a debian:trixie-slim container with one
# tcp listener per half of the backends (a loopback address each, 127.0.S.B).
# Per size it prints: the store's used bytes (emfor_store_used_bytes) after the
# first checks and after 60 s, resident memory, the CPU of the daemon in
# percent of one core over 30 s, the largest check latency, and how long it
# takes until every backend of the stopped listener is unhealthy (expected:
# fall x interval + timeout = 3 x 1 s + 0.5 s, plus the scrape).
#
# The numbers are those of one container on one host, not a guarantee.
# Nothing here is run by the CI or by tools/docker-test.sh.
set -eu
cd "$(dirname "$0")/.."

image=emfor-scale-test
name=emfor-scale-test
sizes=${*:-"10x5 40x10 80x25 80x50"}

cleanup() { docker rm -f "$name" >/dev/null 2>&1 || :; }
trap cleanup EXIT
trap 'exit 130' INT TERM
cleanup

tools/docker-test.sh 'cargo build --release -p emfor-daemon >/tmp/build.log 2>&1 || { tail -20 /tmp/build.log; exit 1; }' >/dev/null
docker image inspect "$image" >/dev/null 2>&1 || docker build -q -t "$image" - >/dev/null <<'EOF'
FROM debian:trixie-slim
RUN apt-get update -qq && apt-get install -y -qq netcat-openbsd procps socat >/dev/null
EOF

docker run -d --init --name "$name" -v emfor-clients-target:/t:ro "$image" sleep infinity >/dev/null
run() { docker exec "$name" sh -c "$1"; }

# gen SERVICES BACKENDS: the config; odd backends on port 8081, even on 8082.
gen() {
    awk -v s="$1" -v b="$2" 'BEGIN {
        print "[store]\npath = \"/var/lib/emfor/state.mdb\"\nmap_size = \"1GiB\""
        print "[metrics]\nlisten = [\"127.0.0.1:5301\"]"
        print "[defaults]\ninterval = \"1s\"\ntimeout = \"500ms\"\nrise = 2\nfall = 3"
        for (i = 1; i <= s; i++) {
            printf "[services.\"s%d.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [", i
            for (j = 1; j <= b; j++) printf "%s\"127.0.%d.%d:%d\"", (j > 1 ? ", " : ""), i, j, (j % 2 ? 8081 : 8082)
            print "]"
        }
    }'
}

scrape() { run "printf 'GET /metrics HTTP/1.0\\r\\n\\r\\n' | nc -w15 127.0.0.1 5301"; }
metric() { scrape | awk -v m="$1" '$1 == m { print $2 }'; }
count() { scrape | grep -c "$1" || :; }
ticks() { run "awk '{print \$14 + \$15}' /proc/\$(pgrep -x emfor)/stat"; }

printf '%-8s %9s %9s %9s %7s %8s %9s %9s\n' size backends 'store@up' 'store@60s' 'RSS MiB' 'CPU %' 'switch s' 'store@hist'
for size in $sizes; do
    s=${size%x*} b=${size#*x}
    total=$((s * b))
    down=$((s * (b / 2)))
    run 'pkill emfor || :; pkill socat || :; while pgrep -x emfor >/dev/null; do sleep 0.2; done; rm -rf /var/lib/emfor; mkdir -p /etc/emfor /var/lib/emfor'
    gen "$s" "$b" | docker exec -i "$name" sh -c 'cat > /etc/emfor/emfor.toml; chmod 644 /etc/emfor/emfor.toml'
    run '(nohup socat TCP-LISTEN:8081,fork,reuseaddr,backlog=4096 - >/dev/null 2>&1 </dev/null &); (nohup socat TCP-LISTEN:8082,fork,reuseaddr,backlog=4096 - >/dev/null 2>&1 </dev/null &); (nohup /t/release/emfor --config /etc/emfor/emfor.toml server >/var/log/emfor.log 2>&1 &)'
    n=0
    while [ "$(count 'state="healthy"} 1')" -lt "$total" ] && [ "$n" -lt 30 ]; do n=$((n + 1)); sleep 2; done
    healthy=$(count 'state="healthy"} 1')
    store10=$(metric emfor_store_used_bytes)
    sleep 20
    t0=$(ticks); sleep 30; t1=$(ticks)
    cpu=$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.1f", (b - a) / 30 }')
    sleep 20
    store60=$(metric emfor_store_used_bytes)
    rss=$(run "awk '/VmRSS/ {printf \"%.0f\", \$2 / 1024}' /proc/\$(pgrep -x emfor)/status")
    # Switch: the listener of the even backends stops.
    start=$(date +%s.%N)
    run "pkill -f '^socat TCP-LISTEN:8082' || :"
    n=0
    while [ "$(count 'state="unhealthy"} 1')" -lt "$down" ] && [ "$n" -lt 300 ]; do n=$((n + 1)); sleep 0.2; done
    end=$(date +%s.%N)
    sw=$(awk -v a="$start" -v b="$end" 'BEGIN { printf "%.1f", b - a }')
    [ "$n" -lt 300 ] || sw=">${sw}s"
    # Fill the history: the listener comes and goes until every even backend has
    # changed state more often than the history keeps (20 entries).
    i=0
    while [ "$i" -lt 12 ]; do
        i=$((i + 1))
        run '(nohup socat TCP-LISTEN:8082,fork,reuseaddr,backlog=4096 - >/dev/null 2>&1 </dev/null &)'
        sleep 6
        run "pkill -f '^socat TCP-LISTEN:8082' || :"
        sleep 6
    done
    storeh=$(metric emfor_store_used_bytes)
    [ "$healthy" = "$total" ] || echo "   (only $healthy of $total backends healthy after the wait)"
    printf '%-8s %9s %9s %9s %7s %8s %9s %9s\n' "$size" "$total" "$store10" "$store60" "$rss" "$cpu" "$sw" "$storeh"
done
