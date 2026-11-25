#!/bin/sh
# Runs inside the systemd container (docker exec): emfor as a transient unit
# named emfor, Vector with examples/vector/vector-journald.yaml.
set -u
SVC=api.svc.emfor
fail() {
    echo "FAIL: $*" >&2
    journalctl -u emfor --no-pager -n 5 -o cat >&2
    tail -10 /tmp/vector.log >&2; tail -3 /tmp/out-journald.ndjson >&2
    exit 1
}
wait_for() { d=$1; shift; t=0
    while [ "$t" -lt 100 ]; do "$@" >/dev/null 2>&1 && { echo "ok: $d"; return 0; }; t=$((t + 1)); sleep 0.2; done
    fail "$d"; }

echo "vector: $(vector --version)"
mkdir -p /etc/emfor /var/lib/emfor /var/lib/vector
cat > /etc/emfor/emfor.toml <<CFG
[log]
format = "json"

[services."$SVC"]
check = "tcp"
backends = [{ addr = "127.0.0.1:9001" }]
interval = "200ms"
timeout = "100ms"
rise = 1
fall = 1
jitter = "0%"
CFG
chmod 0644 /etc/emfor/emfor.toml
(while :; do nc -l 127.0.0.1 9001 </dev/null >/dev/null 2>&1 || sleep 0.1; done) & BPID=$!
systemd-run --unit=emfor /usr/local/bin/emfor server --config /etc/emfor/emfor.toml >/dev/null 2>&1
wait_for "emfor writes its JSON log to the journal" sh -c 'journalctl -u emfor -o cat | grep -q backend_state_changed'
vector --config /etc/vector/emfor-journald.yaml >/tmp/vector.log 2>&1 &
wait_for "vector read a journal line" sh -c 'grep -q backend_state_changed /tmp/out-journald.ndjson'
kill "$BPID" 2>/dev/null; pkill nc 2>/dev/null
wait_for "the state change to unhealthy arrives parsed" \
    sh -c 'grep backend_state_changed /tmp/out-journald.ndjson | grep -q "\"to\":\"unhealthy\""'
l=$(grep backend_state_changed /tmp/out-journald.ndjson | grep '"to":"unhealthy"' | head -1)
echo "$l" | jq -e --arg s "$SVC" '.service == $s and (.backend|length>0) and (.from|length>0) and has("reason") and has("timestamp") and has("level")' >/dev/null \
    || fail "state change line lacks stable fields: $l"
echo "ok: $(echo "$l" | jq -c '{event,service,backend,from,to,reason,level}')"
echo "ok: tests/vector/journald"
