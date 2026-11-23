#!/bin/sh
# What a peer group does when its members do not list the same services:
# three containers A, B and C of a docker network run emfor 's Linux build, and
# four scenarios check the claims of chapter 11, "When members differ".
#
#   tools/peer-mismatch-test.sh
#
# Needs docker. The daemon is built in the test image (tools/docker-test.sh) and
# shared with the containers through its cargo volume; the first run builds
# that image and pulls debian:trixie-slim. B and C are cut off from the
# backend (a blackhole route), A is not: A's own check passes, so A shows the
# backend down only when two of its three assigned checkers vote down.
#
#   1  All three list the backend:               A takes the votes of B and C  -> down
#   2  C does not list that backend:             only B votes down             -> up
#   3  C has another check definition (timeout): C's votes do not count       -> up
#   4  A service only A and B list: C is among its assigned checkers, a dead
#      slot, until A and B set peers = false; then nobody is assigned.
#
# A also reports the members that send no results (silent_checkers in
# status --json, a status line and emfor_peer_silent_checkers): in scenario 2
# and 4 it names C, in scenarios 1 and 3 nobody is silent.
#
# Nothing here is run by the CI or by tools/docker-test.sh.
set -eu
cd "$(dirname "$0")/.."

net=emfor-peer-net
image=emfor-peer-test
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
tools/docker-test.sh 'cargo build -p emfor-daemon 2>&1 | tail -1' >/dev/null
docker image inspect "$image" >/dev/null 2>&1 || docker build -q -t "$image" - >/dev/null <<'EOF'
FROM debian:trixie-slim
RUN apt-get update -qq && apt-get install -y -qq iproute2 netcat-openbsd procps >/dev/null
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

start() { run "$1" "pkill emfor || :; sleep 0.5; rm -f /var/lib/emfor/state.mdb*; (nohup /t/debug/emfor --config /etc/emfor/emfor.toml server >/var/log/emfor.log 2>&1 &)" ; }
stop_all() { for m in a b c; do run "$m" 'pkill emfor || :; ip route del blackhole '"$backend"'/32 2>/dev/null || :' >/dev/null; done; }
cut_off() { run "$1" "ip route replace blackhole $backend/32"; }

json() { run a "/t/debug/emfor --config /etc/emfor/emfor.toml status --json" 2>/dev/null | tr -d ' \n'; }
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
live() { # wait until A sees both others
    n=0
    while [ "$n" -lt 20 ]; do
        run a "/t/debug/emfor --config /etc/emfor/emfor.toml status" 2>/dev/null | grep -q 'peers: 2/2 live' && return 0
        n=$((n + 1)); sleep 1
    done
    echo "   (A did not report 2 live members; the votes may not be there)"
}

step "1 all three list the backend: B and C cut off, A sees it up"
stop_all
for m in a b c; do config "$m" "" 500ms true "$backend"; start "$m"; done
sleep 4; live
[ "$(state_of)" = healthy ] && ok "A: healthy at the start" || fail "A: healthy at the start ($(state_of))"
cut_off b; cut_off c
if wait_for unhealthy 15; then ok "A: down by votes of B and C"; else fail "A: down by votes of B and C ($(state_of))"; fi
if no_silent 25; then ok "A: nobody is silent while all three send results"; else fail "A: nobody is silent ($(silent_json))"; fi
[ "$(silent_metric)" = "emfor_peer_silent_checkers 0" ] && ok "metric is 0" || fail "metric is 0 ($(silent_metric))"
show

step "2 C does not list that backend: only B votes down"
stop_all
config a "" 500ms true "$backend"; config b "" 500ms true "$backend"; config c "" 500ms true "$subnet.22"
for m in a b c; do start "$m"; done
sleep 4; live
cut_off b; cut_off c
if wait_silent 15 "\"member\":\"$subnet.13:5302\",\"service\":\"$svc\""; then
    ok "A reports C as silent within 15 seconds of the cut"
else
    fail "A reports C as silent within 15 seconds of the cut ($(silent_json))"
fi
if stays healthy 10; then ok "A stays healthy: one down vote is not enough"; else fail "A stays healthy ($(state_of))"; fi
silent_text | grep -q "no results from $subnet.13:5302 for $svc (1 backends)" && ok "status text names C" || fail "status text names C ($(silent_text))"
[ "$(silent_metric)" = "emfor_peer_silent_checkers 1" ] && ok "metric is 1" || fail "metric is 1 ($(silent_metric))"
show

step "3 C has another check definition (timeout 700ms): its votes do not count"
stop_all
config a "" 500ms true "$backend"; config b "" 500ms true "$backend"; config c "" 700ms true "$backend"
for m in a b c; do start "$m"; done
sleep 4; live
cut_off b; cut_off c
if stays healthy 15; then ok "A stays healthy: C's votes are not counted"; else fail "A stays healthy ($(state_of))"; fi
if [ -z "$(silent_json)" ]; then ok "C is a digest mismatch, not a silent checker"; else fail "C is not silent ($(silent_json))"; fi
show
if json | grep -q '"digest_mismatch":\[[^]]*[a-z0-9"]'; then ok "A shows a digest mismatch"; else fail "A shows a digest mismatch"; fi

step "4 a service only A and B list"
stop_all
only="[services.\"only.svc.emfor\"]
check = \"tcp\"
backends = [\"$backend:8080\"]"
onlyoff="[services.\"only.svc.emfor\"]
check = \"tcp\"
peers = false
backends = [\"$backend:8080\"]"
config a "$only" 500ms true "$backend"; config b "$only" 500ms true "$backend"; config c "" 500ms true "$backend"
for m in a b c; do start "$m"; done
sleep 6; live
if wait_silent 15 "\"member\":\"$subnet.13:5302\",\"service\":\"only.svc.emfor\""; then
    ok "A reports C as silent for the service C does not have"
else
    fail "A reports C as silent for the service C does not have ($(silent_json))"
fi
if only_checkers | grep -q "$subnet.13"; then
    ok "A: C is among the assigned checkers of a service C does not have"
else
    echo "   A: $(only_checkers)"
    fail "A: C is among the assigned checkers of a service C does not have"
fi
config a "$onlyoff" 500ms true "$backend"; config b "$onlyoff" 500ms true "$backend"
start a; start b
sleep 6
if only_checkers | grep -q "$subnet.13"; then
    fail "A: with peers = false, C is no longer an assigned checker"
else
    ok "A: with peers = false, C is no longer an assigned checker"
fi
if silent_json | grep -q 'only.svc.emfor'; then
    fail "A: with peers = false, the silent report for the service is gone"
else
    ok "A: with peers = false, the silent report for the service is gone"
fi
echo
if [ "$failed" = 0 ]; then echo "all checks passed"; else echo "some checks FAILED"; exit 1; fi
