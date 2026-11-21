#!/bin/sh
# Runs inside the test container: two backends (HAProxy answering on 9001 and
# 9002 with its port), emfor with the DNS server on 5300, and the HAProxy under
# test in front, configured as docs/manual/14-integrations.md says.
set -u

fail() { echo "FAIL: $*" >&2; echo "--- emfor:" >&2; tail -5 /tmp/emfor.log >&2; exit 1; }

echo "haproxy: $(haproxy -v 2>&1 | head -1)"
mkdir -p /etc/emfor /var/lib/emfor

backend() { # <port> [address]
    cat > "/tmp/b$1.cfg" <<CFG
defaults
  mode http
  timeout client 5s
  timeout server 5s
  timeout connect 1s
frontend f
  bind ${2:-127.0.0.1}:$1
  http-request return status 200 content-type text/plain string "$1"
CFG
    haproxy -f "/tmp/b$1.cfg" -D -p "/tmp/b$1.pid" >/dev/null 2>&1
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
start_emfor() { emfor server --config /etc/emfor/emfor.toml 2>>/tmp/emfor.log & EMFOR=$!; }
start_emfor
reload() { kill -HUP "$EMFOR"; }

count() { # <n>: a=9001 b=9002 other=the rest
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
wait_for() { # <description> <test on $a $b $other>
    t=0
    while [ "$t" -lt 60 ]; do
        out=$(count 10); a=${out#9001=}; a=${a%% *}
        b=${out#*9002=}; b=${b%% *}; other=${out#*other=}
        if eval "$2"; then echo "ok: $1 ($out)"; return 0; fi
        t=$((t + 1)); sleep 0.2
    done
    fail "$1: $out"
}

# --- the recipe
cat > /tmp/hap.cfg <<'CFG'
defaults
    mode http
    timeout client 5s
    timeout server 5s
    timeout connect 1s

resolvers emfor
    nameserver emfor 127.0.0.1:5300
    accepted_payload_size 4096
    hold valid 1s
    resolve_retries 3
    timeout resolve 1s
    timeout retry 1s

frontend fe
    bind 127.0.0.1:8080
    default_backend api

backend api
    balance roundrobin
    server-template s 1-4 _http._tcp.api.svc.emfor resolvers emfor resolve-opts allow-dup-ip resolve-prefer ipv4 init-addr none
CFG
haproxy -c -f /tmp/hap.cfg >/dev/null 2>&1 || { haproxy -c -f /tmp/hap.cfg; fail "the HAProxy config is not valid"; }
haproxy -f /tmp/hap.cfg -D -p /tmp/hap.pid >/dev/null 2>&1

wait_for "both backends get requests" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ] && [ "$other" = 0 ]'

# A backend that fails emfor's checks leaves the list; one that recovers returns.
kill "$(cat /tmp/b9002.pid)"
wait_for "a backend that is down gets nothing" '[ "$a" = 10 ]'
backend 9002
wait_for "the recovered backend gets requests again" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'

# drain: emfor reloads, HAProxy is not touched.
config round-robin "$B1" '{ addr = "127.0.0.1:9002", drain = true }'; reload
wait_for "a drained backend gets nothing, HAProxy not reloaded" '[ "$a" = 10 ]'
config round-robin "$B1" "$B2"; reload
wait_for "an undrained backend gets requests again" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'

# What HAProxy does not take from the answer: priorities and weights.
config failover "$B1" "$B2"; reload
sleep 3
out=$(count 40); a=${out#9001=}; a=${a%% *}; b=${out#*9002=}; b=${b%% *}
[ "$a" -ge 10 ] && [ "$b" -ge 10 ] || fail "HAProxy honours SRV priorities now ($out): update the manual"
echo "ok: SRV priorities are not followed, failover spreads over both ($out)"
config weighted '{ addr = "127.0.0.1:9001", weight = 3 }, { addr = "127.0.0.1:9002", weight = 1 }'; reload
sleep 3
out=$(count 80); a=${out#9001=}; a=${a%% *}; b=${out#*9002=}; b=${b%% *}
[ "$a" -ge 30 ] && [ "$a" -le 50 ] || fail "HAProxy follows SRV weights now ($out): update the manual"
echo "ok: SRV weights are not followed ($out)"

# More backends than slots: only as many as the template has.
backend 9003
config round-robin "$B1" "$B2" '"127.0.0.1:9003"'; reload
sleep 3
seen=$(for i in $(seq 1 60); do curl -s -m 2 http://127.0.0.1:8080/; echo; done | sort -u | grep -c .)
[ "$seen" = 3 ] || fail "three backends and four slots: $seen distinct answers"
echo "ok: three backends fill three of the four slots"

# emfor stops: the answers time out or are refused, and HAProxy keeps its
# servers (hold valid applies to good answers only).
config round-robin "$B1" "$B2"; reload
sleep 3
kill "$EMFOR"
sleep 4
out=$(count 10); case "$out" in *"other=0") ;; *) fail "HAProxy lost its servers when emfor stopped: $out" ;; esac
echo "ok: HAProxy keeps its servers while emfor does not answer ($out)"
echo "ok: tests/haproxy"
