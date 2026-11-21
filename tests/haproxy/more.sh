#!/bin/sh
# Runs inside the test container, after inside.sh: what the HAProxy section of
# docs/manual/14-integrations.md says about `mode tcp`, TLS to the backends and
# several name servers. Two backends (HAProxy answering on 9001 and 9002 with
# its port, in HTTP over TLS when asked to), emfor with the DNS server on
# 5300 (and a second emfor on 5301), and the HAProxy under test in front.
set -u

fail() {
    echo "FAIL: $*" >&2
    echo "--- emfor:" >&2; tail -4 /tmp/emfor.log >&2
    echo "--- haproxy:" >&2; tail -4 /tmp/hap.log >&2
    exit 1
}

echo "haproxy: $(haproxy -v 2>&1 | head -1)"
mkdir -p /etc/emfor /var/lib/emfor /tmp/s1 /tmp/s2
openssl req -x509 -newkey rsa:2048 -nodes -keyout /tmp/k.pem -out /tmp/c.pem -days 2 \
    -subj "/CN=api.internal" -addext "subjectAltName=DNS:api.internal" >/dev/null 2>&1 \
    || fail "openssl could not make a certificate"
cat /tmp/c.pem /tmp/k.pem > /tmp/api.pem

backend() { # <port> [bind options, for example "ssl crt /tmp/api.pem"]
    cat > "/tmp/b$1.cfg" <<CFG
defaults
  mode http
  timeout client 5s
  timeout server 5s
  timeout connect 1s
frontend f
  bind 127.0.0.1:$1 ${2:-}
  http-request return status 200 content-type text/plain string "$1"
CFG
    haproxy -f "/tmp/b$1.cfg" -D -p "/tmp/b$1.pid" >/dev/null 2>&1
}
stop_backend() { kill "$(cat "/tmp/b$1.pid")" 2>/dev/null; sleep 0.3; }

emfor_conf() { # <file> <dns port> <store> <backends>
    cat > "$1" <<CFG
[store]
path = "$3"

[dns]
listen = ["127.0.0.1:$2"]

[services."api.svc.emfor"]
check = "tcp"
backends = [$4]
interval = "200ms"
timeout = "100ms"
rise = 1
fall = 1
jitter = "0%"
CFG
}
ALL='{ addr = "127.0.0.1:9001" }, { addr = "127.0.0.1:9002" }'
ONE='{ addr = "127.0.0.1:9001" }, { addr = "127.0.0.1:9002", drain = true }'

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
    while [ "$t" -lt 60 ]; do
        out=$(count 10); a=$(field 9001 "$out"); b=$(field 9002 "$out"); other=$(field other "$out")
        if eval "$2"; then echo "ok: $1 ($out)"; return 0; fi
        t=$((t + 1)); sleep 0.2
    done
    fail "$1: $out"
}

# hap <mode> <nameserver lines> <server-template options>: (re)start HAProxy.
hap() {
    if [ -s /tmp/hap.pid ]; then kill "$(cat /tmp/hap.pid)"; sleep 1; fi
    : > /tmp/hap.pid
    cat > /tmp/hap.cfg <<CFG
defaults
    mode $1
    timeout client 5s
    timeout server 5s
    timeout connect 1s

resolvers emfor
$2
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
    server-template s 1-4 _http._tcp.api.svc.emfor resolvers emfor resolve-opts allow-dup-ip resolve-prefer ipv4 init-addr none $3
CFG
    haproxy -c -f /tmp/hap.cfg >/dev/null 2>&1 || { haproxy -c -f /tmp/hap.cfg; fail "the HAProxy config is not valid"; }
    haproxy -f /tmp/hap.cfg -D -p /tmp/hap.pid >/tmp/hap.log 2>&1 || fail "HAProxy did not start"
    sleep 2
}
NS1='    nameserver emfor 127.0.0.1:5300'

emfor_conf /etc/emfor/a.toml 5300 /tmp/s1/state.mdb "$ALL"
emfor server --config /etc/emfor/a.toml 2>/tmp/emfor.log &
EA=$!
reload_a() { kill -HUP "$EA"; }

# --- mode tcp: the same as in mode http (the backends answer HTTP, passed on)
backend 9001
backend 9002
hap tcp "$NS1" ""
wait_for "mode tcp: both backends get connections" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ] && [ "$other" = 0 ]'
stop_backend 9002
wait_for "mode tcp: a backend that is down gets nothing" '[ "$a" = 10 ]'
backend 9002
wait_for "mode tcp: the recovered backend gets connections again" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'
emfor_conf /etc/emfor/a.toml 5300 /tmp/s1/state.mdb "$ONE"; reload_a
wait_for "mode tcp: a drained backend gets nothing" '[ "$a" = 10 ]'
emfor_conf /etc/emfor/a.toml 5300 /tmp/s1/state.mdb "$ALL"; reload_a
wait_for "mode tcp: an undrained backend gets connections again" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'

# --- TLS to the backends (the checks of emfor only connect)
stop_backend 9001; stop_backend 9002
backend 9001 "ssl crt /tmp/api.pem"
backend 9002 "ssl crt /tmp/api.pem"
sleep 1
hap http "$NS1" "ssl verify none"
wait_for "TLS, verify none: both backends get requests" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ] && [ "$other" = 0 ]'
hap http "$NS1" "ssl verify required ca-file /tmp/c.pem sni str(api.internal)"
wait_for "TLS, verify required with sni str(api.internal): the certificate is checked against that name" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ] && [ "$other" = 0 ]'
# verifyhost is not the same in every HAProxy (see the manual): note what it does
hap http "$NS1" "ssl verify required ca-file /tmp/c.pem verifyhost api.internal"
echo "note: TLS, verify required with verifyhost api.internal and no sni: $(count 10)"
# without a name HAProxy 3.4 checks the certificate against the name of the SRV target and
# refuses it; 2.8 checks the chain only
hap http "$NS1" "ssl verify required ca-file /tmp/c.pem"
echo "note: TLS, verify required with neither sni nor verifyhost: $(count 10)"
hap http "$NS1" "ssl verify required ca-file /tmp/c.pem sni str(wrong.example)"
out=$(count 10)
[ "$(field other "$out")" = 10 ] || fail "a wrong sni still answered: $out"
echo "ok: TLS, an sni that is not in the certificate: nothing is served ($out)"
hap http "$NS1" "ssl verify required ca-file /tmp/c.pem verifyhost wrong.example sni str(api.internal)"
echo "note: TLS, verify required with verifyhost wrong.example and sni str(api.internal): $(count 10)"
stop_backend 9001; stop_backend 9002
backend 9001
backend 9002
sleep 1

# --- two name servers with the same answers
emfor_conf /etc/emfor/b.toml 5301 /tmp/s2/state.mdb "$ALL"
emfor server --config /etc/emfor/b.toml 2>/tmp/emfor-b.log &
EB=$!
NS2='    nameserver a 127.0.0.1:5300
    nameserver b 127.0.0.1:5301'
hap http "$NS2" ""
wait_for "two name servers with the same answer: both backends get requests" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ] && [ "$other" = 0 ]'
kill "$EA"; wait "$EA" 2>/dev/null
sleep 4
wait_for "the first name server stopped: the second one is enough" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ] && [ "$other" = 0 ]'
stop_backend 9002
wait_for "the first name server stopped: a down backend still leaves the list" '[ "$a" = 10 ]'
backend 9002
wait_for "the first name server stopped: it returns" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ]'
emfor server --config /etc/emfor/a.toml 2>>/tmp/emfor.log &
EA=$!
sleep 2
kill "$EB"; wait "$EB" 2>/dev/null
sleep 4
wait_for "the second name server stopped, the first one back: both backends get requests" '[ "$a" -ge 2 ] && [ "$b" -ge 2 ] && [ "$other" = 0 ]'

echo "ok: tests/haproxy (more)"
