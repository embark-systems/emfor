#!/bin/sh
# Runs inside the test container: two backends (nginx on 9001 and 9002, each
# answering its port), emfor with the DNS server on 5300, and the Caddy under
# test in front (the recipe of docs/manual/14-integrations.md, "Caddy").
# The container's own resolver does not know svc.emfor, so what Caddy needs
# from emfor it must be told with `resolvers`.
set -u

fail() {
    echo "FAIL: $*" >&2
    echo "--- emfor:" >&2; tail -5 /tmp/emfor.log >&2
    echo "--- caddy:" >&2; tail -4 /tmp/caddy.log | cut -c1-300 >&2
    exit 1
}

echo "caddy: $(caddy version)"
mkdir -p /etc/emfor /var/lib/emfor

backend() { # <port>
    cat > "/tmp/b$1.conf" <<CFG
daemon off; pid /tmp/b$1.pid; error_log /tmp/b$1.err; events {}
http { access_log off; server { listen 127.0.0.1:$1; location / { return 200 "$1\n"; } } }
CFG
    nginx -c "/tmp/b$1.conf" &
}
backend 9001
backend 9002

config() { # <balance> <backend 1> <backend 2>
    cat > /etc/emfor/emfor.toml <<CFG
[dns]
listen = ["127.0.0.1:5300"]

[services."api.svc.emfor"]
check = "tcp"
balance = "$1"
backends = [$2, $3]
interval = "200ms"
timeout = "100ms"
rise = 1
fall = 1
jitter = "0%"
CFG
}
B1='{ addr = "127.0.0.1:9001" }'
B2='{ addr = "127.0.0.1:9002" }'
config round-robin "$B1" "$B2"
emfor server --config /etc/emfor/emfor.toml 2>/tmp/emfor.log &
EMFOR=$!
reload() { kill -HUP "$EMFOR"; }

# caddy_start <reverse_proxy lines> <dynamic srv lines> <transport lines>:
# stop the running Caddy and start one with this Caddyfile.
CADDY=
caddy_start() {
    if [ -n "$CADDY" ]; then kill "$CADDY"; wait "$CADDY" 2>/dev/null; fi
    cat > /tmp/Caddyfile <<CFG
{
    admin off
}
:8080 {
    reverse_proxy {
        $1
        dynamic srv {
            name _http._tcp.api.svc.emfor
            refresh 1s
            resolvers 127.0.0.1:5300
            $2
        }
        transport http {
            $3
        }
    }
}
CFG
    caddy run --config /tmp/Caddyfile --adapter caddyfile > /tmp/caddy.log 2>&1 &
    CADDY=$!
    t=0
    until curl -s -m 1 -o /dev/null http://127.0.0.1:8080/ 2>/dev/null || [ "$t" -ge 25 ]; do
        t=$((t + 1)); sleep 0.2
    done
    sleep 1
}

# Requests to Caddy, counted by answer: "9001=12 9002=8 other=0".
count() { # <n>
    i=0; a=0; b=0; other=0
    while [ "$i" -lt "$1" ]; do
        i=$((i + 1))
        case "$(curl -s -m 3 http://127.0.0.1:8080/)" in
            9001) a=$((a + 1)) ;;
            9002) b=$((b + 1)) ;;
            *) other=$((other + 1)) ;;
        esac
    done
    echo "9001=$a 9002=$b other=$other"
}
field() { # <name> <counts line>
    v=${2#*"$1"=}; echo "${v%% *}"
}
# Wait until `count 10` satisfies a shell test on a, b and other (10 s).
wait_for() { # <description> <test on $a $b $other>
    t=0
    while [ "$t" -lt 50 ]; do
        out=$(count 10); a=$(field 9001 "$out"); b=$(field 9002 "$out"); other=$(field other "$out")
        if eval "$2"; then echo "ok: $1 ($out)"; return 0; fi
        t=$((t + 1)); sleep 0.2
    done
    fail "$1: $out"
}
# Requests sent for 2.5 seconds, 10 a second: how many were not answered.
failures_in_window() {
    i=0; n=0
    while [ "$i" -lt 25 ]; do
        i=$((i + 1))
        case "$(curl -s -m 3 http://127.0.0.1:8080/)" in 9001 | 9002) ;; *) n=$((n + 1)) ;; esac
        sleep 0.1
    done
    echo "$n"
}

RESOLVERS='resolvers 127.0.0.1:5300'

# --- without the transport's resolvers Caddy finds the targets but cannot
# dial their names (the resolver of this container does not know the zone)
caddy_start "" "" ""
out=$(count 10)
[ "$(field other "$out")" = 10 ] || fail "Caddy without transport resolvers answered: $out"
grep -q 'no such host' /tmp/caddy.log || fail "no 'no such host' in the Caddy log"
echo "ok: without resolvers in transport http, the names of the targets are not dialed ($out)"

# --- the recipe, with a grace period and retries
caddy_start "lb_try_duration 2s
        lb_try_interval 100ms" "grace_period 30s" "$RESOLVERS"
wait_for "both backends get requests" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ] && [ "$other" = 0 ]'

# health: a stopped backend loses nothing, because of the retries
kill "$(cat /tmp/b9002.pid)"
n=$(failures_in_window)
[ "$n" = 0 ] || fail "$n requests failed while a backend was down, with lb_try_duration"
wait_for "a backend that is down gets nothing" '[ "$a" = 10 ]'
echo "ok: no request failed when a backend went down (lb_try_duration 2s)"
backend 9002
wait_for "the recovered backend gets requests again" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'

# drain: emfor reloads, Caddy is not touched
config round-robin "$B1" '{ addr = "127.0.0.1:9002", drain = true }'; reload
sleep 2
n=$(failures_in_window)
[ "$n" = 0 ] || fail "$n requests failed during a drain"
wait_for "a drained backend gets nothing, Caddy not reloaded" '[ "$a" = 10 ]'
config round-robin "$B1" "$B2"; reload
wait_for "an undrained backend gets requests again" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'

# weights: Caddy does not follow the SRV weights (3 to 1)
config weighted '{ addr = "127.0.0.1:9001", weight = 3 }' '{ addr = "127.0.0.1:9002", weight = 1 }'; reload
sleep 3
out=$(count 80); a=$(field 9001 "$out")
[ "$a" -ge 25 ] && [ "$a" -le 55 ] || fail "weighted 3:1 gave $out: Caddy follows the weights now, the manual must change"
echo "ok: SRV weights are not followed ($out)"

# priorities: not followed by the default policy; lb_policy first takes the
# order of the answer, which is sorted by priority
config failover "$B1" "$B2"; reload
sleep 3
wait_for "failover, default policy: the backup gets requests too (priorities are not followed)" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'
caddy_start "lb_policy first
        lb_try_duration 2s
        lb_try_interval 100ms" "grace_period 30s" "$RESOLVERS"
wait_for "lb_policy first: only the first backend while it is healthy" '[ "$a" = 10 ]'
kill "$(cat /tmp/b9001.pid)"
wait_for "lb_policy first: the second backend when the first is down" '[ "$b" = 10 ]'
backend 9001
wait_for "lb_policy first: the first backend again" '[ "$a" = 10 ]'

# emfor has nothing to answer (everything drained: SERVFAIL): with a grace
# period Caddy keeps its last list; without one it has no upstream
config round-robin "$B1" "$B2"; reload
caddy_start "" "grace_period 30s" "$RESOLVERS"
wait_for "both backends get requests" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'
config round-robin '{ addr = "127.0.0.1:9001", drain = true }' '{ addr = "127.0.0.1:9002", drain = true }'; reload
sleep 4
out=$(count 10)
[ "$(field other "$out")" = 0 ] || fail "with grace_period Caddy stopped answering when emfor answered SERVFAIL: $out"
echo "ok: with grace_period Caddy keeps its last list when emfor answers SERVFAIL ($out)"
config round-robin "$B1" "$B2"; reload
caddy_start "" "" "$RESOLVERS"
wait_for "both backends get requests" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'
config round-robin '{ addr = "127.0.0.1:9001", drain = true }' '{ addr = "127.0.0.1:9002", drain = true }'; reload
sleep 4
out=$(count 10)
[ "$(field other "$out")" = 10 ] || fail "without grace_period Caddy still answered when emfor answered SERVFAIL: $out"
echo "ok: without grace_period Caddy has no upstream when emfor answers SERVFAIL ($out)"

# no retries: a request sent to a backend that has just stopped is lost
config round-robin "$B1" "$B2"; reload
caddy_start "" "grace_period 30s" "$RESOLVERS"
wait_for "both backends get requests" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'
kill "$(cat /tmp/b9002.pid)"
n=$(failures_in_window)
[ "$n" -ge 1 ] || fail "no request failed when a backend went down without lb_try_duration: the manual's claim is wrong"
echo "ok: without lb_try_duration, requests are lost while a stopped backend is still in the list ($n of 25)"

echo "ok: tests/caddy"
