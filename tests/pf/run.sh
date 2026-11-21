#!/bin/sh
# Usage (FreeBSD, as root, pf not in use):  tests/pf/run.sh
#
# Checks the recipe of docs/manual/14-integrations.md, section "pf (FreeBSD)":
# examples/pf/emfor-pf-table.sh keeps a pf table equal to the backends emfor
# offers, and an rdr rule on that table spreads connections over them. It
# needs a FreeBSD with the pf kernel module, nginx (pkg install nginx), curl,
# drill and an `emfor` binary (EMFOR=/path, default the one in PATH). It runs
# on the loopback only: backends on 127.0.0.2 and 127.0.0.3 port 8000, and the
# address that is redirected, 127.0.0.10 port 80, are aliases of lo0 that it
# adds and removes. It loads its own pf rules and enables pf, so it refuses
# to run when pf is already enabled (it would replace your rules); at the end
# it disables pf again and flushes the rules, tables and states.
set -u

root="$(cd "$(dirname "$0")/../.." && pwd)"
EMFOR=${EMFOR:-emfor}
UPDATER="$root/examples/pf/emfor-pf-table.sh"
W=/tmp/emfor-pf-test
VIP=127.0.0.10
B1=127.0.0.2
B2=127.0.0.3

fail() {
    echo "FAIL: $*" >&2
    echo "--- emfor:" >&2; tail -4 "$W/emfor.log" >&2 2>/dev/null
    echo "--- updater:" >&2; tail -4 "$W/updater.log" >&2 2>/dev/null
    echo "--- table:" >&2; pfctl -t api -T show >&2 2>/dev/null
    exit 1
}

[ "$(uname -s)" = FreeBSD ] || { echo "tests/pf/run.sh is for FreeBSD" >&2; exit 2; }
[ "$(id -u)" = 0 ] || { echo "run it as root" >&2; exit 2; }
command -v "$EMFOR" >/dev/null 2>&1 || { echo "no emfor binary (EMFOR=/path)" >&2; exit 2; }
for c in nginx curl drill pfctl; do
    command -v "$c" >/dev/null 2>&1 || [ -x "/usr/local/sbin/$c" ] || { echo "$c is not installed" >&2; exit 2; }
done
NGINX=$(command -v nginx || echo /usr/local/sbin/nginx)
kldstat -q -m pf 2>/dev/null || kldload pf || { echo "the pf kernel module cannot be loaded" >&2; exit 2; }
if pfctl -si 2>/dev/null | grep -q '^Status: Enabled'; then
    echo "pf is already enabled: this test would replace your rules; stop it first (pfctl -d)" >&2
    exit 2
fi

cleanup() {
    [ -s "$W/updater.pid" ] && kill "$(cat "$W/updater.pid")" 2>/dev/null
    [ -s "$W/emfor.pid" ] && kill "$(cat "$W/emfor.pid")" 2>/dev/null
    for p in "$W"/b*.pid; do [ -s "$p" ] && kill "$(cat "$p")" 2>/dev/null; done
    sleep 1
    pfctl -d >/dev/null 2>&1
    pfctl -F all >/dev/null 2>&1
    for a in $B1 $B2 $VIP; do ifconfig lo0 -alias "$a" >/dev/null 2>&1; done
}
trap cleanup EXIT

rm -rf "$W"; mkdir -p "$W"
for a in $B1 $B2 $VIP; do ifconfig lo0 alias "$a/32" || fail "cannot add $a to lo0"; done
head -c 1200 /dev/zero | tr '\0' A > "$W/slow-$B1.txt"
head -c 1200 /dev/zero | tr '\0' B > "$W/slow-$B2.txt"

# a backend answers its own address, and /slow is 1200 bytes at 200 a second
backend() { # <address>
    cat > "$W/b$1.conf" <<CFG
daemon off; pid $W/b$1.pid; error_log $W/b$1.err; events {}
http { access_log off; server { listen $1:8000;
  location / { return 200 "$1\n"; }
  location = /slow { alias $W/slow-$1.txt; limit_rate 200; } } }
CFG
    "$NGINX" -c "$W/b$1.conf" >/dev/null 2>&1 &
}
stop_backend() {
    p=$(cat "$W/b$1.pid"); kill "$p"
    while kill -0 "$p" 2>/dev/null; do sleep 0.1; done
}
backend $B1
backend $B2

config() { # <balance> <backend, ...>
    balance="$1"; shift
    list=$(printf '%s, ' "$@" | sed 's/, $//')
    cat > "$W/emfor.toml" <<CFG
[store]
path = "$W/state.mdb"

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
E1="{ addr = \"$B1:8000\" }"
E2="{ addr = \"$B2:8000\" }"
start_emfor() { "$EMFOR" server --config "$W/emfor.toml" 2>>"$W/emfor.log" & echo $! > "$W/emfor.pid"; }
reload() { kill -HUP "$(cat "$W/emfor.pid")"; }
config round-robin "$E1" "$E2"
start_emfor

cat > "$W/pf.conf" <<CFG
table <api> persist
rdr pass on lo0 inet proto tcp from any to $VIP port 80 -> <api> port 8000 round-robin
CFG
pfctl -f "$W/pf.conf" 2>/dev/null || fail "pfctl could not load the rules"
pfctl -e >/dev/null 2>&1 || fail "pfctl could not enable pf"

SLEEP=1 "$UPDATER" loop api.svc.emfor api 127.0.0.1 5300 2>"$W/updater.log" &
echo $! > "$W/updater.pid"

count() { # <n>
    i=0; a=0; b=0; other=0
    while [ "$i" -lt "$1" ]; do
        i=$((i + 1))
        case "$(curl -s -m 2 "http://$VIP:80/")" in
            "$B1") a=$((a + 1)) ;;
            "$B2") b=$((b + 1)) ;;
            *) other=$((other + 1)) ;;
        esac
    done
    echo "$B1=$a $B2=$b other=$other"
}
field() { v=${2#*"$1"=}; echo "${v%% *}"; } # <name> <counts line>
wait_for() { # <description> <test on $a $b $other>
    t=0
    while [ "$t" -lt 60 ]; do
        out=$(count 10); a=$(field $B1 "$out"); b=$(field $B2 "$out"); other=$(field other "$out")
        if eval "$2"; then echo "ok: $1 ($out)"; return 0; fi
        t=$((t + 1)); sleep 0.2
    done
    fail "$1: $out"
}
table() { pfctl -t api -T show 2>/dev/null | sed 's/^[[:space:]]*//' | sort | tr '\n' ' '; }
wait_table() { # <description> <expected table, addresses separated by spaces>
    t=0
    while [ "$t" -lt 100 ]; do
        [ "$(table)" = "$2" ] || [ "$(table)" = "$2 " ] && { echo "ok: $1 (table: $(table))"; return 0; }
        t=$((t + 1)); sleep 0.2
    done
    fail "$1: the table is '$(table)', expected '$2'"
}

echo "pf: $(uname -sr)"
wait_table "the table holds both backends" "$B1 $B2"
wait_for "both backends get connections through the redirect" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ] && [ "$other" = 0 ]'

# health: a backend that fails its checks leaves the table
stop_backend $B2
wait_table "a backend that fails its checks leaves the table" "$B1"
wait_for "it gets no connection" '[ "$a" = 10 ]'
backend $B2
wait_table "the recovered backend returns to the table" "$B1 $B2"

# a connection that is open when its backend leaves the table is not cut:
# its pf state stays. failover puts everything on the first backend.
config failover "$E1" "$E2"; reload
wait_table "failover: only the first backend (priority 0)" "$B1"
curl -s -m 30 -o "$W/s.out" "http://$VIP:80/slow" &
SLOW=$!
sleep 1
config failover "{ addr = \"$B1:8000\", drain = true }" "$E2"; reload
wait_table "drain: the drained backend leaves the table" "$B2"
wait_for "drain: new connections go to the other backend" '[ "$b" = 10 ]'
kill -0 "$SLOW" 2>/dev/null || fail "the slow request ended too early (timing)"
wait "$SLOW" 2>/dev/null
size=$(wc -c < "$W/s.out" 2>/dev/null | tr -d ' '); size=${size:-0}
[ "$size" = 1200 ] || fail "the connection open during the drain got $size bytes, not 1200"
echo "ok: a connection already open when its backend is drained finishes (1200 bytes): the pf state stays"
config failover "$E1" "$E2"; reload
wait_table "undrained: the first backend is back and alone" "$B1"
stop_backend $B1
wait_table "failover: the second backend when the first is down" "$B2"
backend $B1
wait_table "failover: the first backend again" "$B1"

# weights are not used: pf round-robin is equal
config weighted "{ addr = \"$B1:8000\", weight = 3 }" "{ addr = \"$B2:8000\", weight = 1 }"; reload
wait_table "weighted: both backends" "$B1 $B2"
out=$(count 80); a=$(field $B1 "$out")
[ "$a" -ge 25 ] && [ "$a" -le 55 ] || fail "weighted 3:1 gave $out: pf follows the weights now, the manual must change"
echo "ok: SRV weights are not used, pf shares equally ($out)"

# nothing to offer (every backend drained: SERVFAIL): the table is emptied, and pf
# drops the connection of a client: it waits for its own timeout
config round-robin "{ addr = \"$B1:8000\", drain = true }" "{ addr = \"$B2:8000\", drain = true }"; reload
wait_table "every backend drained: the table is emptied" ""
start=$(date +%s)
out=$(count 3)
elapsed=$(( $(date +%s) - start ))
[ "$(field other "$out")" = 3 ] || fail "connections were served with an empty table: $out"
[ "$elapsed" -ge 5 ] || fail "3 connections with an empty table took $elapsed seconds: the manual says they wait for the client's timeout (2 seconds each here)"
echo "ok: with an empty table a connection is dropped, the client waits for its timeout ($out, $elapsed s for 3)"

# with EMPTY set, the table holds an address where nothing listens
kill "$(cat "$W/updater.pid")"; wait "$(cat "$W/updater.pid")" 2>/dev/null
EMPTY=127.0.0.1 SLEEP=1 "$UPDATER" loop api.svc.emfor api 127.0.0.1 5300 2>>"$W/updater.log" &
echo $! > "$W/updater.pid"
wait_table "EMPTY=127.0.0.1: the table holds the address where nothing listens" "127.0.0.1"
# (a client on another host is refused at once then, and a loopback client is
# not: pf lets the refusal of a redirected loopback connection time out, so
# the refusal is not checked here; the manual says what was seen from another host)

# emfor does not answer: the table is left as it is
config round-robin "$E1" "$E2"; reload
wait_table "back to both backends" "$B1 $B2"
kill "$(cat "$W/emfor.pid")"; wait "$(cat "$W/emfor.pid")" 2>/dev/null
sleep 5
[ "$(table)" = "$B1 $B2 " ] || fail "the table changed while emfor was away: '$(table)'"
echo "ok: when emfor does not answer, the table is left as it is ($(table))"
config round-robin "$E1" "{ addr = \"$B2:8000\", drain = true }"
start_emfor
wait_table "emfor back: the table follows it again" "$B1"

echo "ok: tests/pf"
