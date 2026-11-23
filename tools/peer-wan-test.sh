#!/bin/sh
# What the votes of a peer group do when the network between the members is
# slow or loses packets, the case of members in two data centres (chapter 11,
# "Several data centres"): three containers A, B and C run emfor 's Linux
# build, and tc netem puts a delay or a loss on the peer messages (UDP, port 5302) of every
# member; the checks of the backend are not touched.
#
#   tools/peer-wan-test.sh
#
# Needs docker and a kernel with sch_netem (Docker Desktop has it). The daemon
# is built in the test image (tools/docker-test.sh). Every setting runs 60
# seconds with the backend up for all three members; the table says, for A,
#   not live   seconds in which A saw fewer than 2 live members,
#   not up     seconds in which A did not show the backend healthy,
#   silent     seconds in which A named a silent checker,
# and then B and C are cut off from the backend (A is not): `down in` is the
# time until A shows it unhealthy, which needs the down votes of B and C.
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


netem() { # netem MEMBER ARGS...: delay or loss on the peer port only (UDP 5302); no ARGS clears it
    m=$1; shift
    run "$m" 'tc qdisc del dev eth0 root 2>/dev/null || :'
    [ $# -gt 0 ] || return 0
    run "$m" "tc qdisc add dev eth0 root handle 1: prio
tc qdisc add dev eth0 parent 1:3 handle 30: netem $*
tc filter add dev eth0 protocol ip parent 1:0 prio 3 u32 match ip protocol 17 0xff match ip dport 5302 0xffff flowid 1:3"
}
live_count() { run a "/t/debug/emfor --config /etc/emfor/emfor.toml status" 2>/dev/null | grep -o 'peers: [0-9]*/' | head -1 | tr -dc 0-9; }

printf '%-28s %9s %7s %7s %10s\n' setting 'not live' 'not up' silent 'down in'
for setting in "none:" "delay 100ms 20ms:delay 100ms 20ms" "loss 10%:loss 10%" "loss 30%:loss 30%" "loss 50%:loss 50%" "delay 1500ms 500ms:delay 1500ms 500ms"; do
    label=${setting%%:*} args=${setting#*:}
    stop_all
    for m in a b c; do netem "$m"; done
    for m in a b c; do config "$m" "" 500ms true "$backend"; start "$m"; done
    sleep 4; live
    for m in a b c; do [ -z "$args" ] || netem "$m" $args; done
    notlive=0 notup=0 sil=0 n=0
    while [ "$n" -lt 60 ]; do
        [ "$(live_count)" = 2 ] || notlive=$((notlive + 1))
        [ "$(state_of)" = healthy ] || notup=$((notup + 1))
        [ -z "$(silent_json)" ] || sil=$((sil + 1))
        n=$((n + 1)); sleep 1
    done
    cut_off b; cut_off c
    n=0
    while [ "$n" -lt 40 ] && [ "$(state_of)" != unhealthy ]; do n=$((n + 1)); sleep 1; done
    if [ "$(state_of)" = unhealthy ]; then down="${n}s"; else down=">40s"; fi
    printf '%-28s %9s %7s %7s %10s\n' "$label" "$notlive" "$notup" "$sil" "$down"
done
