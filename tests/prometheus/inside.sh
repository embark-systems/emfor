#!/bin/sh
# Runs inside the test container: two backends (nginx on 9001 and 9002, each
# serving /metrics), emfor with the DNS server on 127.0.0.1:53 as the only
# resolver of the container (a Prometheus asks the system resolver and cannot
# be given another name server or port), and a Prometheus that finds its
# targets with dns_sd_configs of type SRV, as docs/manual/07-dns.md says.
set -u

SVC=api.svc.emfor
H=127-0-0-1.$SVC
EMFOR_CONF=/etc/emfor/emfor.toml

fail() {
    echo "FAIL: $*" >&2
    echo "--- emfor:" >&2; tail -5 /tmp/emfor.log >&2
    echo "--- prometheus:" >&2; tail -8 /tmp/prom.log >&2
    echo "--- targets:" >&2; targets >&2
    exit 1
}

echo "prometheus: $(prometheus --version 2>&1 | head -1)"
mkdir -p /etc/emfor /var/lib/emfor /tmp/prom

backend() { # <port>
    printf '# TYPE demo gauge\ndemo{port="%s"} 1\n' "$1" > "/tmp/metrics-$1.txt"
    cat > "/tmp/b$1.conf" <<CFG
daemon off; pid /tmp/b$1.pid; error_log /tmp/b$1.err; events {}
http { access_log off; server { listen 127.0.0.1:$1;
  location = /metrics { default_type text/plain; alias /tmp/metrics-$1.txt; } } }
CFG
    nginx -c "/tmp/b$1.conf" &
}
stop_backend() { kill "$(cat "/tmp/b$1.pid")"; }
backend 9001
backend 9002

config() { # <balance> <backend 1> <backend 2>
    cat > "$EMFOR_CONF" <<CFG
[dns]
listen = ["127.0.0.1:53"]

[services."$SVC"]
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
emfor server --config "$EMFOR_CONF" 2>/tmp/emfor.log &
EMFOR=$!
reload() { kill -HUP "$EMFOR"; }
printf 'nameserver 127.0.0.1\n' > /etc/resolv.conf

cat > /tmp/prometheus.yml <<CFG
global:
  scrape_interval: 1s
  scrape_timeout: 500ms
scrape_configs:
  - job_name: api
    dns_sd_configs:
      - names: ["_http._tcp.$SVC"]
        type: SRV
        refresh_interval: 1s
CFG
prometheus --config.file=/tmp/prometheus.yml --storage.tsdb.path=/tmp/prom \
    --web.listen-address=127.0.0.1:9090 2>/tmp/prom.log &
t=0
until curl -sf http://127.0.0.1:9090/-/ready >/dev/null 2>&1; do
    t=$((t + 1)); [ "$t" -lt 100 ] || fail "prometheus did not start"; sleep 0.2
done

# The targets of the job: "<instance>=<health>" sorted, on one line.
targets() {
    curl -s http://127.0.0.1:9090/api/v1/targets |
        jq -r '[.data.activeTargets[] | select(.scrapePool == "api") | "\(.labels.instance)=\(.health)"] | sort | join(" ")'
}
# The value of `up` for an instance: empty when there is no such series.
up_of() {
    curl -s --data-urlencode "query=up{instance=\"$1\"}" http://127.0.0.1:9090/api/v1/query |
        jq -r '.data.result[0].value[1] // empty'
}
wait_targets() { # <description> <expected targets line>   (20 s)
    t=0
    while [ "$t" -lt 100 ]; do
        [ "$(targets)" = "$2" ] && { echo "ok: $1 ($2)"; return 0; }
        t=$((t + 1)); sleep 0.2
    done
    fail "$1: targets are '$(targets)', expected '$2'"
}
T1=$H:9001
T2=$H:9002

wait_targets "both backends are targets, and up" "$T1=up $T2=up"
[ "$(up_of "$T1")" = 1 ] || fail "up of $T1 is not 1"

# A backend that fails its checks is not a target any more (not a target
# that is down): its up series ends.
stop_backend 9002
wait_targets "a backend that fails its checks leaves the targets" "$T1=up"
t=0
while [ -n "$(up_of "$T2")" ] && [ "$t" -lt 50 ]; do t=$((t + 1)); sleep 0.2; done
[ -z "$(up_of "$T2")" ] || fail "the up series of $T2 did not end"
echo "ok: it has no up series either, so up == 0 never fires for it"
backend 9002
wait_targets "the recovered backend is a target again" "$T1=up $T2=up"

# A drained backend is not a target either, though it is running.
config round-robin "$B1" '{ addr = "127.0.0.1:9002", drain = true }'; reload
wait_targets "a drained backend leaves the targets, though it still runs" "$T1=up"
curl -sf http://127.0.0.1:9002/metrics >/dev/null || fail "the drained backend does not answer"
config round-robin "$B1" "$B2"; reload
wait_targets "an undrained backend is a target again" "$T1=up $T2=up"

# failover: priorities 0 and 1 in the SRV answer; Prometheus takes both.
config failover "$B1" "$B2"; reload
sleep 2
wait_targets "failover: Prometheus takes the backup too, it ignores priorities" "$T1=up $T2=up"

# backup = true: an answer lists the backups only when no primary is healthy.
config round-robin "$B1" '{ addr = "127.0.0.1:9002", backup = true }'; reload
wait_targets "a backup backend is not a target while a primary is healthy" "$T1=up"
stop_backend 9001
wait_targets "the backup is the target when no primary is healthy" "$T2=up"
backend 9001
wait_targets "the primary replaces it when it is healthy again" "$T1=up"

echo "ok: tests/prometheus"
