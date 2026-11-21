#!/bin/sh
# Runs inside the test container: emfor with `exec` checks that run a
# stand-in for a Nagios plugin through examples/nagios/emfor-exec.sh, and
# examples/nagios/check_emfor.sh asked what emfor reports. The stand-in says
# what the file /tmp/state-<port> tells it: ok, warn, crit, unknown or hang.
set -u

fail() {
    echo "FAIL: $*" >&2
    echo "--- emfor:" >&2; tail -5 /tmp/emfor.log >&2
    exit 1
}

mkdir -p /etc/emfor /var/lib/emfor /usr/local/libexec
cat > /usr/local/libexec/fake_check <<'PLUGIN'
#!/bin/sh
# a stand-in for a Nagios plugin: fake_check -H <host> -p <port> [more]
port=
while [ $# -gt 0 ]; do case "$1" in -p) port=$2; shift 2 ;; *) shift ;; esac; done
echo "$EMFOR_IP:$port ${EMFOR_SERVICE:-}" > "/tmp/seen-$port"
case "$(cat "/tmp/state-$port" 2>/dev/null || echo ok)" in
    ok) echo "OK - fine on $port"; exit 0 ;;
    warn) echo "WARNING - slow on $port"; exit 1 ;;
    crit) echo "CRITICAL - broken on $port"; exit 2 ;;
    pipe) echo "CRITICAL - a|b on $port"; exit 2 ;;
    unknown) echo "UNKNOWN - cannot tell on $port"; exit 3 ;;
    hang) sleep 30; exit 0 ;;
esac
PLUGIN
chmod +x /usr/local/libexec/fake_check
for p in 7001 7002 7003 7101; do echo ok > "/tmp/state-$p"; done

W='"/usr/local/bin/emfor-exec.sh", "/usr/local/libexec/fake_check"'
config() { # <drain on 7001: yes|no>
    d=""; [ "$1" = yes ] && d=', drain = true'
    cat > /etc/emfor/emfor.toml <<CFG
[services."web.svc.emfor"]
check = "exec"
backends = [{ addr = "127.0.0.1:7001"$d }, "127.0.0.1:7002", "127.0.0.1:7003"]
interval = "1s"
timeout = "500ms"
rise = 1
fall = 1
jitter = "0%"
check_options = { command = [$W] }

[services."warn.svc.emfor"]
check = "exec"
backends = ["127.0.0.1:7101"]
interval = "1s"
timeout = "500ms"
rise = 1
fall = 1
jitter = "0%"
check_options = { command = ["/usr/local/bin/emfor-exec.sh", "--warning", "up", "/usr/local/libexec/fake_check"] }
CFG
}
config no
emfor server --config /etc/emfor/emfor.toml 2>/tmp/emfor.log &
EMFOR=$!
reload() { kill -HUP "$EMFOR"; }
settle() { sleep 3; }

# the state and the reason of a backend, as emfor reports them
backend() { emfor status --json | jq -r --arg id "$1" '[.services[].backends[] | select(.id == $id)][0] | "\(.state)|\(.reason // "")"'; }
expect_backend() { # <id> <state> <reason or empty>
    got=$(backend "$1")
    [ "$got" = "$2|$3" ] || fail "$1: '$got', expected '$2|$3'"
}

settle
# --- emfor-exec.sh
for p in 7001 7002 7003; do expect_backend 127.0.0.1:$p healthy ""; done
[ "$(cat /tmp/seen-7001)" = "127.0.0.1:7001 web.svc.emfor" ] || fail "the plugin saw '$(cat /tmp/seen-7001)'"
grep -q . /tmp/seen-7002 || fail "the plugin did not run for 7002"
echo "ok: emfor-exec.sh runs the plugin with -H and -p of the backend (it saw $(cat /tmp/seen-7001))"

echo warn > /tmp/state-7002; echo warn > /tmp/state-7101; settle
expect_backend 127.0.0.1:7002 unhealthy "WARNING - slow on 7002"
echo "ok: a plugin's WARNING (exit 1) is down for emfor, the first line is the reason"
expect_backend 127.0.0.1:7101 healthy ""
echo "ok: with --warning up a WARNING is up"

echo crit > /tmp/state-7003; settle
expect_backend 127.0.0.1:7003 unhealthy "CRITICAL - broken on 7003"
echo "ok: a plugin's CRITICAL (exit 2) is down for emfor"

echo unknown > /tmp/state-7001; settle
got=$(backend 127.0.0.1:7001)
case "$got" in healthy*) fail "a plugin's UNKNOWN (exit 3) left the backend healthy: $got" ;; esac
echo "ok: a plugin's UNKNOWN (exit 3) is an error for emfor, not up ($got)"

echo hang > /tmp/state-7001; settle
got=$(backend 127.0.0.1:7001)
case "$got" in unhealthy\|timeout*) ;; *) fail "a plugin that hangs: '$got', expected unhealthy|timeout" ;; esac
echo "ok: a plugin that does not finish in time is down (timeout)"

# --- check_emfor.sh
for p in 7001 7002 7003 7101; do echo ok > "/tmp/state-$p"; done; settle
run() { out=$(check_emfor.sh "$@"); rc=$?; }
run -s web.svc.emfor
[ "$rc" = 0 ] || fail "all healthy gave $rc: $out"
case "$out" in "EMFOR OK - 1 service(s)"*"| 'web.svc.emfor_healthy'=3;3;1;0;3") ;; *) fail "all healthy: '$out'" ;; esac
echo "ok: check_emfor.sh, all healthy: exit 0 ($out)"

run
[ "$rc" = 0 ] && case "$out" in *"2 service(s)"*"'warn.svc.emfor_healthy'=1;1;1;0;1"*) true ;; *) false ;; esac || fail "every service, all healthy: rc=$rc '$out'"
echo "ok: check_emfor.sh without -s looks at every service ($out)"

echo warn > /tmp/state-7002; settle
run -s web.svc.emfor
[ "$rc" = 1 ] || fail "one backend down gave $rc: $out"
case "$out" in "EMFOR WARNING - web.svc.emfor: 2 of 3 healthy (127.0.0.1:7002 WARNING - slow on 7002) |"*) ;; *) fail "one down: '$out'" ;; esac
echo "ok: one backend down: WARNING, exit 1, with the reason ($out)"

run -s web.svc.emfor -w 2
[ "$rc" = 0 ] || fail "-w 2 with two healthy gave $rc: $out"
echo "ok: -w 2: two healthy backends are enough, exit 0"

echo crit > /tmp/state-7003; settle
run -s web.svc.emfor
[ "$rc" = 1 ] || fail "one healthy of three gave $rc: $out (the default -c is 1)"
run -s web.svc.emfor -c 2
[ "$rc" = 2 ] || fail "-c 2 with one healthy gave $rc: $out"
echo "ok: one healthy of three: WARNING, and CRITICAL with -c 2"

echo crit > /tmp/state-7001; settle
run -s web.svc.emfor
[ "$rc" = 2 ] || fail "all down gave $rc: $out"
case "$out" in "EMFOR CRITICAL - web.svc.emfor: 0 of 3 healthy ("*) ;; *) fail "all down: '$out'" ;; esac
echo "ok: every backend down: CRITICAL, exit 2 ($(echo "$out" | cut -c1-90)...)"

# a "|" in a backend's reason must not start the perfdata early
echo pipe > /tmp/state-7002; settle
run -s web.svc.emfor
[ "$(printf '%s' "$out" | tr -cd '|' | wc -c)" = 1 ] || fail "a pipe in the reason: '$out'"
case "$out" in *"CRITICAL - a/b on 7002"*" | 'web.svc.emfor_healthy'="*) ;; *) fail "a pipe in the reason: '$out'" ;; esac
echo "ok: a '|' in a reason is shown as '/', the only '|' is the one before the perfdata ($out)"

for p in 7001 7002 7003; do echo ok > "/tmp/state-$p"; done
config yes; reload; settle
run -s web.svc.emfor
[ "$rc" = 0 ] || fail "a drained backend gave $rc: $out"
case "$out" in *"drained: 127.0.0.1:7001"*"'web.svc.emfor_healthy'=2;2;1;0;2") ;; *) fail "a drained backend: '$out'" ;; esac
echo "ok: a drained backend is left out of the counts and named ($out)"

kill "$EMFOR"; wait "$EMFOR" 2>/dev/null
run -s web.svc.emfor
[ "$rc" = 2 ] || fail "a stopped daemon gave $rc: $out"
case "$out" in "EMFOR CRITICAL - the daemon is stopped"*) ;; *) fail "stopped: '$out'" ;; esac
echo "ok: a stopped daemon: CRITICAL ($out)"

run -s nothere.svc.emfor
[ "$rc" = 3 ] || fail "an unknown service gave $rc: $out"
echo "ok: a service that does not exist: UNKNOWN, exit 3 ($out)"
run -e /nonexistent/emfor
[ "$rc" = 3 ] || fail "no emfor binary gave $rc: $out"
echo "ok: no emfor binary: UNKNOWN, exit 3"

echo "ok: tests/nagios"
