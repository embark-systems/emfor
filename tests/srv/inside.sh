#!/bin/sh
# Runs inside the test container: emfor as the only resolver (port 53) with
# the services kdc, kdcf (failover), ldap and db, each with two backends that
# are plain listeners (a listener logs a connection and closes it), and three
# clients that find their servers with SRV records: MIT Kerberos (kinit with
# dns_lookup_kdc), OpenLDAP (ldapsearch with a DNS-based URI) and PyMongo
# (mongodb+srv://). None of the servers is real: what is checked is which
# servers the client asks emfor for and goes to.
set -u

fail() {
    echo "FAIL: $*" >&2
    echo "--- emfor:" >&2; tail -4 /tmp/emfor.log >&2
    exit 1
}

mkdir -p /etc/emfor /var/lib/emfor /tmp/l
cat > /tmp/listen.py <<'PY'
import socket, sys
addr, port = sys.argv[1], int(sys.argv[2])
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind((addr, port)); s.listen(16)
while True:
    c, a = s.accept()
    with open("/tmp/conn.log", "a") as f:
        f.write("%s:%d\n" % (addr, port))
    c.close()
PY
listener() { # <address> <port>
    python3 /tmp/listen.py "$1" "$2" &
    echo $! > "/tmp/l/$1-$2.pid"
}
unlisten() { kill "$(cat "/tmp/l/$1-$2.pid")"; wait "$(cat "/tmp/l/$1-$2.pid")" 2>/dev/null; }
for p in 88 389 27017; do listener 127.0.0.2 $p; listener 127.0.0.3 $p; done
sleep 1

cat > /etc/emfor/emfor.toml <<'CFG'
[dns]
listen = ["127.0.0.1:53"]

[services."kdc.svc.emfor"]
check = "tcp"
backends = ["127.0.0.2:88", "127.0.0.3:88"]
interval = "300ms"
timeout = "200ms"
rise = 1
fall = 1
jitter = "0%"

[services."kdcf.svc.emfor"]
check = "tcp"
balance = "failover"
backends = ["127.0.0.2:88", "127.0.0.3:88"]
interval = "300ms"
timeout = "200ms"
rise = 1
fall = 1
jitter = "0%"

[services."ldap.svc.emfor"]
check = "tcp"
backends = ["127.0.0.2:389", "127.0.0.3:389"]
interval = "300ms"
timeout = "200ms"
rise = 1
fall = 1
jitter = "0%"

[services."db.svc.emfor"]
check = "tcp"
backends = ["127.0.0.2:27017", "127.0.0.3:27017"]
interval = "300ms"
timeout = "200ms"
rise = 1
fall = 1
jitter = "0%"
CFG
emfor server --config /etc/emfor/emfor.toml 2>/tmp/emfor.log &
EMFOR=$!
printf 'nameserver 127.0.0.1\n' > /etc/resolv.conf
cat > /etc/krb5.conf <<'CFG'
[libdefaults]
 default_realm = KDC.SVC.EMFOR
 dns_lookup_kdc = true
 dns_lookup_realm = false
CFG

# wait until emfor offers <n> SRV records for a name
srv_count() { dig +short SRV "$1" @127.0.0.1 | grep -c .; }
wait_srv() { # <name> <n>
    t=0
    while [ "$t" -lt 50 ]; do
        [ "$(srv_count "$1")" = "$2" ] && return 0
        t=$((t + 1)); sleep 0.2
    done
    fail "emfor offers $(srv_count "$1") records for $1, not $2"
}
for n in _kerberos._tcp.kdc.svc.emfor _ldap._tcp.ldap.svc.emfor _mongodb._tcp.db.svc.emfor; do wait_srv "$n" 2; done

# --- Kerberos: SRV of the realm, UDP first, then TCP
kinit_trace() { # <realm>: a kinit with no KDC behind it; the trace is /tmp/krb.trace
    : > /tmp/conn.log; rm -f /tmp/krb.trace
    echo secret | KRB5_TRACE=/tmp/krb.trace timeout 30 kinit "user@$1" >/dev/null 2>&1
}
kinit_trace KDC.SVC.EMFOR
grep -q 'Sending DNS SRV query for _kerberos._udp.KDC.SVC.EMFOR' /tmp/krb.trace || fail "kinit did not ask for the _udp SRV name"
grep -q 'Sending DNS SRV query for _kerberos._tcp.KDC.SVC.EMFOR' /tmp/krb.trace || fail "kinit did not ask for the _tcp SRV name"
grep -q 'SRV answer: 0 1 88 "127-0-0-2.kdc.svc.emfor."' /tmp/krb.trace || fail "no SRV answer for the first KDC in the trace"
grep -q 'SRV answer: 0 1 88 "127-0-0-3.kdc.svc.emfor."' /tmp/krb.trace || fail "no SRV answer for the second KDC in the trace"
sort -u /tmp/conn.log | tr '\n' ' ' | grep -q '127.0.0.2:88 127.0.0.3:88' || fail "kinit did not go to both KDCs: $(sort -u /tmp/conn.log | tr '\n' ' ')"
echo "ok: Kerberos: realm KDC.SVC.EMFOR, the _udp name gets no answer, _tcp lists both KDCs, kinit goes to both ($(sort -u /tmp/conn.log | tr '\n' ' '))"

unlisten 127.0.0.2 88
wait_srv _kerberos._tcp.kdc.svc.emfor 1
kinit_trace KDC.SVC.EMFOR
sort -u /tmp/conn.log | tr '\n' ' ' | grep -q '^127.0.0.3:88 $' || fail "with the first KDC down kinit went to: $(sort -u /tmp/conn.log | tr '\n' ' ')"
if grep -q 'SRV answer: 0 1 88 "127-0-0-2' /tmp/krb.trace; then fail "emfor still offered the KDC that is down"; fi
echo "ok: Kerberos: a KDC that fails its checks is not offered, kinit goes to the other only"
listener 127.0.0.2 88
wait_srv _kerberos._tcp.kdc.svc.emfor 2

# --- Kerberos with a failover service: the primary first (priority 0 before 1)
sed -i 's/^ default_realm = .*/ default_realm = KDCF.SVC.EMFOR/' /etc/krb5.conf
wait_srv _kerberos._tcp.kdcf.svc.emfor 2
kinit_trace KDCF.SVC.EMFOR
grep -q 'SRV answer: 0 1 88 "127-0-0-2.kdcf' /tmp/krb.trace && grep -q 'SRV answer: 1 1 88 "127-0-0-3.kdcf' /tmp/krb.trace || fail "no priority 0 and 1 in the trace: $(grep 'SRV answer' /tmp/krb.trace | tr '\n' ' ')"
first=$(grep 'Initiating TCP connection' /tmp/krb.trace | head -1)
case "$first" in *127.0.0.2:88) ;; *) fail "with failover the first attempt was not the primary: $first" ;; esac
echo "ok: Kerberos with balance = \"failover\": priorities 0 and 1, kinit tries the primary first"
sed -i 's/^ default_realm = .*/ default_realm = KDC.SVC.EMFOR/' /etc/krb5.conf

# --- OpenLDAP: ldap:///dc=ldap,dc=svc,dc=emfor, the commas percent-encoded
URI='ldap:///dc=ldap%2Cdc=svc%2Cdc=emfor'
ldap_try() { : > /tmp/conn.log; ldapsearch -d1 -x -H "$URI" -b "" -s base -o nettimeout=3 > /tmp/ldap.out 2>&1; }
ldap_try
grep -q 'ldap_url_parse_ext(ldap://127-0-0-2.ldap.svc.emfor:389)' /tmp/ldap.out && grep -q 'ldap_url_parse_ext(ldap://127-0-0-3.ldap.svc.emfor:389)' /tmp/ldap.out || fail "ldapsearch did not turn the DN into the two servers: $(grep ldap_url_parse /tmp/ldap.out | tr '\n' ' ')"
[ "$(sort -u /tmp/conn.log | wc -l)" -ge 1 ] || fail "ldapsearch connected nowhere"
echo "ok: OpenLDAP: the URI $URI becomes the two servers of _ldap._tcp.ldap.svc.emfor, ldapsearch connected to $(sort -u /tmp/conn.log | tr '\n' ' ')"
unlisten 127.0.0.2 389
wait_srv _ldap._tcp.ldap.svc.emfor 1
for i in 1 2 3; do ldap_try; [ "$(sort -u /tmp/conn.log | tr '\n' ' ')" = "127.0.0.3:389 " ] || fail "with the first server down ldapsearch went to: $(sort -u /tmp/conn.log | tr '\n' ' ')"; done
echo "ok: OpenLDAP: a server that fails its checks is not offered (3 of 3 went to the other)"
listener 127.0.0.2 389
wait_srv _ldap._tcp.ldap.svc.emfor 2

# --- PyMongo: mongodb+srv://db.svc.emfor
mongo_hosts() {
    python3 - <<'PY'
import pymongo.uri_parser as up
r = up.parse_uri("mongodb+srv://db.svc.emfor/")
print(sorted("%s:%d" % h for h in r["nodelist"]), r["options"])
PY
}
out=$(mongo_hosts 2>&1)
case "$out" in "['127-0-0-2.db.svc.emfor:27017', '127-0-0-3.db.svc.emfor:27017']"*) ;; *) fail "pymongo: $out" ;; esac
case "$out" in *"'ssl': True"* | *"'tls': True"*) ;; *) fail "mongodb+srv did not turn TLS on: $out" ;; esac
echo "ok: PyMongo: mongodb+srv://db.svc.emfor gives the two servers, TXT is answered without records, and TLS is on by default ($out)"
unlisten 127.0.0.2 27017
wait_srv _mongodb._tcp.db.svc.emfor 1
out=$(mongo_hosts 2>&1)
case "$out" in "['127-0-0-3.db.svc.emfor:27017']"*) ;; *) fail "pymongo with one server down: $out" ;; esac
echo "ok: PyMongo: a server that fails its checks is not in the list ($out)"
listener 127.0.0.2 27017
wait_srv _mongodb._tcp.db.svc.emfor 2

# drain leaves the list too
cat > /etc/emfor/emfor.toml.new <<'CFG'
CFG
sed 's|backends = \["127.0.0.2:27017", "127.0.0.3:27017"\]|backends = [{ addr = "127.0.0.2:27017", drain = true }, "127.0.0.3:27017"]|' /etc/emfor/emfor.toml > /etc/emfor/emfor.toml.new && mv /etc/emfor/emfor.toml.new /etc/emfor/emfor.toml
kill -HUP "$EMFOR"
wait_srv _mongodb._tcp.db.svc.emfor 1
out=$(mongo_hosts 2>&1)
case "$out" in "['127-0-0-3.db.svc.emfor:27017']"*) ;; *) fail "pymongo with a drained server: $out" ;; esac
echo "ok: PyMongo: a drained server is not in the list ($out)"

echo "ok: tests/srv"
