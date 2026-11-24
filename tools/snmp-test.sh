#!/bin/sh
# Try examples/snmp/emfor-snmp-trap.sh and the snmpd "extend" line of the manual
# (docs/manual/14-integrations.md, "Old network management systems") against
# net-snmp's snmptrapd and snmpd, in a Debian 12 container.
#
#   tools/snmp-test.sh [packages/dist/emfor_*_arm64.deb]
#
# It installs the deb (build it with packages/build.sh), runs a daemon with one
# tcp backend, stops and starts the backend, restarts the daemon, and prints
# the traps snmptrapd received and what snmpd gives for the extend line. It
# prints, it does not assert. Needs docker and network access for apt. Not run
# by the CI or docker-test.sh.
set -eu
if [ "${1:-}" = "--inside" ]; then
    shift
else
    cd "$(dirname "$0")/.."
    deb=${1:-$(ls packages/dist/emfor_*.deb | head -n 1)}
    [ -f "$deb" ] || { echo "usage: tools/snmp-test.sh [emfor.deb]" >&2; exit 2; }
    exec docker run --rm         -v "$PWD/tools/snmp-test.sh:/t.sh:ro"         -v "$PWD/$(dirname "$deb"):/deb:ro"         -v "$PWD/examples:/examples:ro"         -e DEB="/deb/$(basename "$deb")" debian:12 sh /t.sh --inside
fi
set +e
set -u
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq snmp snmpd snmptrapd jq python3 procps >/dev/null 2>&1 || { echo "apt failed"; exit 1; }
dpkg -i "$DEB" >/dev/null 2>&1; emfor --version
mkdir -p /srv/b && (cd /srv/b && python3 -m http.server 9001 --bind 127.0.0.1 >/dev/null 2>&1 & echo $! > /tmp/b.pid)
cat > /etc/emfor/emfor.toml <<CFG
[services."web.svc.emfor"]
check = "tcp"
backends = ["127.0.0.1:9001"]
interval = "200ms"
timeout = "100ms"
rise = 1
fall = 1
jitter = "0%"
CFG
mkdir -p /var/lib/emfor; chown emfor:emfor /var/lib/emfor; chmod 0750 /var/lib/emfor
su -s /bin/sh emfor -c 'emfor server --config /etc/emfor/emfor.toml >/tmp/emfor.log 2>&1 &'
sleep 3
cat /tmp/emfor.log | head -3
emfor status --config /etc/emfor/emfor.toml | sed -n 1,4p
# --- trap receiver
printf 'authCommunity log,net secret\n' > /etc/snmp/snmptrapd.conf
snmptrapd -f -Lf /tmp/traps.log -c /etc/snmp/snmptrapd.conf udp:127.0.0.1:1162 >/tmp/trapd.err 2>&1 &
sleep 2
EX=/examples/snmp/emfor-snmp-trap.sh
ST=/tmp/trapstate/last
run() { sh $EX -H 127.0.0.1:1162 -o 1.3.6.1.4.1.99999.1 -c secret -S $ST -C /etc/emfor/emfor.toml; echo "script exit=$?"; }
echo "== first run (records only)"; run
sleep 1; echo "traps so far: $(grep -c . /tmp/traps.log 2>/dev/null || echo 0)"
echo "== backend down"; kill $(cat /tmp/b.pid); sleep 2; run; sleep 1
echo "== backend up"; (cd /srv/b && python3 -m http.server 9001 --bind 127.0.0.1 >/dev/null 2>&1 & echo $! > /tmp/b.pid); sleep 2; run; sleep 1
echo "== received:"; cat /tmp/traps.log | cut -c1-300
echo "== run again, nothing new"; run; sleep 1; echo "traps total lines: $(grep -c . /tmp/traps.log)"
echo "== daemon restart (backend is up)"; before=$(grep -c . /tmp/traps.log)
pkill -x emfor; sleep 2
su -s /bin/sh emfor -c 'emfor server --config /etc/emfor/emfor.toml >/tmp/emfor2.log 2>&1 &'; sleep 3
run; sleep 1; echo "new trap lines after restart: $(( $(grep -c . /tmp/traps.log) - before ))"
grep -o 'STRING: "[a-z]*"' /tmp/traps.log | tail -4 | tr '\n' ' '; echo
emfor status --json --config /etc/emfor/emfor.toml | jq -c '.history' | cut -c1-400
# --- extend
cp /examples/nagios/check_emfor.sh /usr/local/bin/check_emfor.sh; chmod 755 /usr/local/bin/check_emfor.sh
cat > /etc/snmp/snmpd.conf <<CFG
agentaddress udp:127.0.0.1:1161
rocommunity public 127.0.0.1
extend emfor-web /usr/local/bin/check_emfor.sh -C /etc/emfor/emfor.toml -s web.svc.emfor
CFG
snmpd -f -Lf /tmp/snmpd.log -C -c /etc/snmp/snmpd.conf >/dev/null 2>&1 &
sleep 3
get() { snmpwalk -v2c -c public -On -m '' 127.0.0.1:1161 .1.3.6.1.4.1.8072.1.3.2 2>&1 | grep -E '\.(3|4)\.1\.[0-9]*\.9\.101|\.3\.2\.4\.1\.[0-9]' | cut -c1-200; }
echo "== extend, backend up (snmpd user: $(ps -o user= -C snmpd | head -1))"; get
kill $(cat /tmp/b.pid); sleep 8
echo "== extend, backend down (after the 5 s cache)"; get
