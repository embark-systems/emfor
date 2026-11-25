#!/bin/sh
# Runs inside the test container: a backend (nc on 9001), emfor with the JSON
# log in /tmp/emfor.log and /metrics on 5301, and Vector with
# examples/vector/vector.yaml writing what it read to two files.
set -u

SVC=api.svc.emfor
fail() {
    echo "FAIL: $*" >&2
    echo "--- emfor:" >&2; tail -5 /tmp/emfor.log >&2
    echo "--- vector:" >&2; tail -10 /tmp/vector.log >&2
    echo "--- out-log:" >&2; tail -3 /tmp/out-log.ndjson >&2
    echo "--- out-metrics:" >&2; tail -2 /tmp/out-metrics.ndjson >&2
    exit 1
}
wait_for() { # <description> <command...>   (20 s)
    d=$1; shift; t=0
    while [ "$t" -lt 100 ]; do
        "$@" >/dev/null 2>&1 && { echo "ok: $d"; return 0; }
        t=$((t + 1)); sleep 0.2
    done
    fail "$d"
}

echo "vector: $(vector --version)"
mkdir -p /etc/emfor /var/lib/emfor
cat > /etc/emfor/emfor.toml <<CFG
[log]
format = "json"

[metrics]
listen = ["127.0.0.1:5301"]

[services."$SVC"]
check = "tcp"
backends = [{ addr = "127.0.0.1:9001" }]
interval = "200ms"
timeout = "100ms"
rise = 1
fall = 1
jitter = "0%"
CFG

backend() { (while :; do nc -l 127.0.0.1 9001 </dev/null >/dev/null 2>&1 || sleep 0.1; done) & BPID=$!; }
backend
emfor server --config /etc/emfor/emfor.toml 2>/tmp/emfor.log &
EMFOR=$!
wait_for "emfor serves /metrics" curl -sf http://127.0.0.1:5301/metrics

vector --config /etc/vector/emfor.yaml >/tmp/vector.log 2>&1 &
wait_for "vector wrote a scraped metric" \
    sh -c 'grep -q emfor_build_info /tmp/out-metrics.ndjson'

# Metrics: name, tags and a type Vector understood.
m=$(grep '"name":"emfor_backend_state"' /tmp/out-metrics.ndjson | tail -1)
[ -n "$m" ] || fail "no emfor_backend_state metric"
echo "$m" | jq -e --arg s "$SVC" '.tags.service == $s and (.tags.backend | length > 0) and (.tags.state | length > 0)' >/dev/null \
    || fail "emfor_backend_state has not the tags service, backend and state: $m"
echo "ok: emfor_backend_state arrives with service, backend and state"
echo "$(grep -c '"name":"emfor_' /tmp/out-metrics.ndjson) emfor metric events"

# Log: kill the backend, wait for the state change line.
kill "$BPID" 2>/dev/null; pkill nc 2>/dev/null
wait_for "the state change to unhealthy arrives parsed" \
    sh -c 'grep "\"event\":\"backend_state_changed\"" /tmp/out-log.ndjson | grep -q "\"to\":\"unhealthy\""'
l=$(grep '"event":"backend_state_changed"' /tmp/out-log.ndjson | grep '"to":"unhealthy"' | head -1)
echo "$l" | jq -e --arg s "$SVC" '.service == $s and (.backend|length>0) and (.from|length>0) and (.to|length>0) and has("reason") and has("timestamp") and has("level")' >/dev/null \
    || fail "state change line lacks stable fields: $l"
jq -s -e 'length > 0 and all(.[]; has("level") and has("timestamp"))' /tmp/out-log.ndjson >/dev/null \
    || fail "a line that is not emfor JSON got through the remap: $(grep -v '"level"' /tmp/out-log.ndjson | head -1)"
echo "ok: only JSON lines arrive (the text lines before the config is read are dropped)"
echo "ok: stable fields of the state change line: $(echo "$l" | jq -c '{event,service,backend,from,to,reason,level}')"

# FreeBSD: syslogd gives each line a prefix; sed cuts it, Vector reads stdin.
cp /etc/vector/emfor-stdin.yaml /tmp/stdin.yaml
{
    echo 'Nov 24 22:38:29 host emfor[3262]: config file read: /etc/emfor/emfor.toml'
    echo 'Nov 24 22:38:30 host emfor[3262]: {"timestamp":"2026-11-24T22:38:30Z","level":"INFO","message":"x","event":"backend_state_changed","service":"api.svc.emfor","backend":"10.0.0.1:80","from":"healthy","to":"unhealthy","reason":"timeout","target":"t"}'
} | sed -un 's/^[^{]*{/{/p' | vector --config /tmp/stdin.yaml >/tmp/vector-stdin.log 2>&1
n=$(wc -l < /tmp/out-stdin.ndjson)
[ "$n" = 1 ] || fail "stdin: expected 1 event (the text line dropped), got $n"
jq -e '.event == "backend_state_changed" and .to == "unhealthy"' /tmp/out-stdin.ndjson >/dev/null || fail "stdin: wrong event"
echo "ok: syslog-prefixed lines through sed and the stdin source: one event, the text line dropped"

kill "$EMFOR" 2>/dev/null
echo "ok: tests/vector"
