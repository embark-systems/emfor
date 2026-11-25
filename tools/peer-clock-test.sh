#!/bin/sh
# What a peer group shows when the clock of one member is off (chapter 11 and
# chapter 12, "After a clock jump"): three containers A, B and C of a docker
# network run emfor 's Linux build, and libfaketime moves the clock of one
# daemon (the containers share one kernel clock, so `date -s` cannot).
#
#   tools/peer-clock-test.sh
#
# Needs docker. The daemon is built in the test image (tools/docker-test.sh).
#
#   1  The clocks agree:       A shows no clock offset, the metric is small
#   2  C is 3 s behind:        A shows C, the metric is about -3, A stays
#                              healthy; with C's clock back, the signal ends
#   3  C is 6 s behind:        C's messages are dropped as stale, and A still
#                              shows C's offset and its metric
#   4  A is 4 s ahead:         A shows one line about this host, not one for
#                              each member
#
# Nothing here is run by the CI or by tools/docker-test.sh.
set -eu
cd "$(dirname "$0")/.."

net=emfor-peer-net
image=emfor-peer-clock-test
subnet=172.31.77
backend=$subnet.21
svc=app.svc.emfor
failed=0

ok() { echo "ok: $*"; }
fail() { echo "FAIL: $*"; failed=1; }
step() { echo; echo "== $*"; }

cleanup() {
    for n in peer-a peer-b peer-c peer-backend; do docker rm -f "$n" >/dev/null 2>&1 || :; done
    docker network rm "$net" >/dev/null 2>&1 || :
}
trap cleanup EXIT
trap 'exit 130' INT TERM
cleanup

# The daemon, in the volume of the test image.
tools/docker-test.sh 'cargo build -p emfor-daemon >/tmp/build.log 2>&1 || { tail -20 /tmp/build.log; exit 1; }' >/dev/null
docker image inspect "$image" >/dev/null 2>&1 || docker build -q -t "$image" - >/dev/null <<'EOF'
FROM debian:trixie-slim
RUN apt-get update -qq && apt-get install -y -qq iproute2 netcat-openbsd procps faketime >/dev/null
EOF

docker network create --subnet "$subnet.0/24" "$net" >/dev/null
docker run -d --name peer-backend --network "$net" --ip "$backend" "$image" nc -lk 8080 >/dev/null
for m in a:11 b:12 c:13; do
    docker run -d --name "peer-${m%:*}" --network "$net" --ip "$subnet.${m#*:}" \
        --cap-add NET_ADMIN -v emfor-clients-target:/t:ro "$image" sleep infinity >/dev/null
done

ip_of() { case $1 in a) echo "$subnet.11";; b) echo "$subnet.12";; c) echo "$subnet.13";; esac; }
run() { docker exec "peer-$1" sh -c "$2"; }

# config MEMBER EXTRA TIMEOUT PEERS BACKEND
# Writes the config of one member. EXTRA is added to the end (more services).
config() {
    m=$1 extra=$2 timeout=$3 peers=$4 be=$5
    run "$m" "mkdir -p /etc/emfor /var/lib/emfor
head -c 48 /dev/zero | tr '\\0' 'k' > /etc/emfor/peers.key; chmod 600 /etc/emfor/peers.key
cat > /etc/emfor/emfor.toml <<EOF
site = \"dc1\"
[store]
path = \"/var/lib/emfor/state.mdb\"
[peers]
listen = \"$(ip_of "$m"):5302\"
members = [\"$subnet.11:5302\", \"$subnet.12:5302\", \"$subnet.13:5302\"]
secret_file = \"/etc/emfor/peers.key\"
[metrics]
listen = [\"127.0.0.1:5301\"]
[defaults]
interval = \"1s\"
timeout = \"$timeout\"
rise = 1
fall = 2
[services.\"$svc\"]
check = \"tcp\"
peers = $peers
backends = [\"$be:8080\"]
$extra
EOF
chmod 644 /etc/emfor/emfor.toml"
}

# start MEMBER [OFFSET]: with OFFSET (for example -3s) the daemon's clock is that
# far from the container's, through libfaketime.
start() { run "$1" "pkill emfor || :; sleep 0.5; rm -f /var/lib/emfor/state.mdb*; (nohup ${2:+faketime -f \"$2\"} /t/debug/emfor --config /etc/emfor/emfor.toml server >/var/log/emfor.log 2>&1 &)" ; }
stop_all() { for m in a b c; do run "$m" 'pkill emfor || :; ip route del blackhole '"$backend"'/32 2>/dev/null || :' >/dev/null; done; }
cut_off() { run "$1" "ip route replace blackhole $backend/32"; }

# AFAKE: the clock of A's daemon, so that the status command reads the store
# with the same clock (it counts a store written "in the future" as stale).
AFAKE=
json() { run a "${AFAKE:+faketime -f \"$AFAKE\"} /t/debug/emfor --config /etc/emfor/emfor.toml status --json" 2>/dev/null | tr -d ' \n'; }
state_of() { json | grep -o '"state":"[a-z]*"' | head -1 | cut -d'"' -f4; }

# wait_for STATE SECONDS: A's state of the backend reaches STATE.
wait_for() {
    n=0
    while [ "$n" -lt "$2" ]; do
        [ "$(state_of)" = "$1" ] && return 0
        n=$((n + 1)); sleep 1
    done
    return 1
}
# stays STATE SECONDS: A's state is STATE for every second of the period.
stays() {
    n=0
    while [ "$n" -lt "$2" ]; do
        [ "$(state_of)" = "$1" ] || return 1
        n=$((n + 1)); sleep 1
    done
}
show() {
    echo "   A: $(state_of); $(json | grep -o '"assigned_checkers":[^]]*][^]]*]' | head -c 300)"
    echo "   A: $(json | grep -o '"digest_mismatch":[^]]*]' | head -c 200)"
    run a "/t/debug/emfor --config /etc/emfor/emfor.toml status" 2>/dev/null | grep -i 'peers' | sed 's/^/   A text: /'
}
silent_json() { json | grep -o '"silent_checkers":\[[^]]*\]' || :; }
silent_text() { run a "/t/debug/emfor --config /etc/emfor/emfor.toml status" 2>/dev/null | grep 'no results from' || :; }
silent_metric() { run a "printf 'GET /metrics HTTP/1.0\\r\\n\\r\\n' | nc -w2 127.0.0.1 5301" 2>/dev/null | grep '^emfor_peer_silent_checkers ' || :; }
# wait_silent SECONDS PATTERN: A's silent_checkers contains PATTERN.
wait_silent() {
    n=0
    while [ "$n" -lt "$1" ]; do
        silent_json | grep -q "$2" && return 0
        n=$((n + 1)); sleep 1
    done
    return 1
}
# no_silent SECONDS: A reports no silent checker at any second of the period.
no_silent() {
    n=0
    while [ "$n" -lt "$1" ]; do
        [ -z "$(silent_json)" ] || return 1
        n=$((n + 1)); sleep 1
    done
}
only_checkers() { json | grep -o '{"backend":"[^}]*"service":"only.svc.emfor"}'; }


offsets_json() { json | grep -o '"clock_offsets":\[[^]]*\]' || :; }
status_text() { run a "${AFAKE:+faketime -f \"$AFAKE\"} /t/debug/emfor --config /etc/emfor/emfor.toml status" 2>/dev/null; }
metrics() { run a "printf 'GET /metrics HTTP/1.0\\r\\n\\r\\n' | nc -w2 127.0.0.1 5301" 2>/dev/null; }
# offset_of MEMBER: the metric of that member in seconds, or empty.
offset_of() { metrics | awk -v p="peer=\"$1\"" '$1 ~ "^emfor_peer_clock_offset_seconds" && index($1, p) { print $2 }'; }
# near VALUE LOW HIGH: LOW <= VALUE <= HIGH
near() { awk -v v="$1" -v lo="$2" -v hi="$3" 'BEGIN { exit !(v != "" && v >= lo && v <= hi) }'; }
wait_for_json() { # wait_for_json SECONDS PATTERN
    n=0
    while [ "$n" -lt "$1" ]; do
        offsets_json | grep -q "$2" && return 0
        n=$((n + 1)); sleep 1
    done
    return 1
}
live() { n=0; while [ "$n" -lt 20 ]; do status_text | grep -q 'peers: 2/2 live' && return 0; n=$((n + 1)); sleep 1; done; }
C=$subnet.13:5302

step "1 the clocks agree"
stop_all
for m in a b c; do config "$m" "" 500ms true "$backend"; start "$m"; done
sleep 4; live
n=0; while [ "$n" -lt 20 ] && [ -z "$(offsets_json)" ]; do n=$((n + 1)); sleep 1; done
if [ -z "$(json)" ]; then fail "A: status --json answers (no clock offset to check)"; elif [ -z "$(offsets_json)" ]; then ok "A: no clock offset for 20 seconds"; else fail "A: no clock offset ($(offsets_json))"; fi
if near "$(offset_of "$C")" -0.5 0.5; then ok "A: the metric for C is small ($(offset_of "$C"))"; else fail "A: the metric for C is small ($(offset_of "$C"))"; fi
if status_text | grep -q 'clock of'; then fail "A: no clock line in status"; else ok "A: no clock line in status"; fi

step "2 C is 3 s behind"
stop_all
for m in a b; do config "$m" "" 500ms true "$backend"; start "$m"; done
config c "" 500ms true "$backend"; start c "-3s"
sleep 4
if wait_for_json 15 "\"member\":\"$C\""; then ok "A: C is in clock_offsets within 15 seconds"; else fail "A: C is in clock_offsets ($(offsets_json))"; fi
status_text | grep -E "peers: clock of $C is (2\.9|3\.0|3\.1) s behind this host" >/dev/null && ok "A: the status line names C, 3 s behind" || fail "A: the status line names C ($(status_text | grep 'clock of'))"
if near "$(offset_of "$C")" -3.2 -2.8; then ok "A: the metric for C is about -3 ($(offset_of "$C"))"; else fail "A: the metric for C is about -3 ($(offset_of "$C"))"; fi
if [ "$(state_of)" = healthy ]; then ok "A: the backend stays healthy"; else fail "A: the backend stays healthy ($(state_of))"; fi
start c
n=0; while [ "$n" -lt 20 ] && [ -n "$(offsets_json)" ]; do n=$((n + 1)); sleep 1; done
if [ -z "$(json)" ]; then fail "A: status --json answers (the offset is gone)"; elif [ -z "$(offsets_json)" ]; then ok "A: the offset is gone within 20 seconds of C's clock being right"; else fail "A: the offset is gone ($(offsets_json))"; fi

step "3 C is 6 s behind"
start c "-6s"
if wait_for_json 15 "\"member\":\"$C\""; then ok "A: C is in clock_offsets"; else fail "A: C is in clock_offsets ($(offsets_json))"; fi
if near "$(offset_of "$C")" -6.2 -5.8; then ok "A: the metric for C is about -6 ($(offset_of "$C"))"; else fail "A: the metric for C is about -6 ($(offset_of "$C"))"; fi
stale=$(metrics | awk '$1 == "emfor_peer_messages_rejected_total{reason=\"stale\"}" { print $2 }')
if [ "${stale:-0}" -gt 0 ]; then ok "A: stale messages are counted ($stale)"; else fail "A: stale messages are counted (${stale:-none})"; fi
if status_text | grep -q "peers: 1/2 live"; then ok "A: C is not live"; else fail "A: C is not live ($(status_text | grep 'peers:'))"; fi

step "4 A is 4 s ahead"
stop_all
AFAKE=+4s
config a "" 500ms true "$backend"; start a "+4s"
for m in b c; do config "$m" "" 500ms true "$backend"; start "$m"; done
sleep 4
n=0; while [ "$n" -lt 20 ] && ! status_text | grep -q 'clock of this host'; do n=$((n + 1)); sleep 1; done
if status_text | grep -q "peers: clock of this host differs from every member by up to"; then ok "A: one line about this host"; else fail "A: one line about this host ($(status_text | grep 'clock'))"; fi
if [ "$(status_text | grep -c 'clock of')" = 1 ]; then ok "A: no line for each member"; else fail "A: no line for each member"; fi
echo
if [ "$failed" = 0 ]; then echo "all checks passed"; else echo "some checks FAILED"; exit 1; fi
