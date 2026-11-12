#!/bin/sh
# Runs inside the emfor-plugins-test container: start redis, MariaDB and
# PostgreSQL as crates/emfor-check/tests/live.rs describes, run those tests,
# then run the emfor daemon with the pgsql plugin.
set -eu

fail() { echo "FAIL: $*" >&2; exit 1; }
step() { echo "== $*"; }

D=/live
mkdir -p "$D"
cd "$D"

step "certificates"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
    -keyout ca.key -out ca.pem -days 2 -subj "/CN=emfor live test CA" 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
    -keyout server.key -out server.csr -subj "/CN=db.test" 2>/dev/null
cat > server.ext <<'EXT'
subjectAltName = DNS:db.test
basicConstraints = CA:FALSE
keyUsage = digitalSignature
extendedKeyUsage = serverAuth
EXT
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
    -out server.crt -days 2 -extfile server.ext 2>/dev/null
chmod 0644 server.key
printf 's3cret\n' > good.pass
printf 'wrong-pw\n' > bad.pass

step "redis"
cat > redis-primary.conf <<CONF
port 6379
tls-port 6390
tls-cert-file $D/server.crt
tls-key-file $D/server.key
tls-ca-cert-file $D/ca.pem
tls-auth-clients no
bind 127.0.0.1
save ""
daemonize yes
logfile $D/redis-primary.log
user default on nopass ~* &* +@all
user emfor on >s3cret ~* &* +@all
CONF
cat > redis-replica.conf <<CONF
port 6380
bind 127.0.0.1
save ""
daemonize yes
logfile $D/redis-replica.log
replicaof 127.0.0.1 6379
user default on nopass ~* &* +@all
user emfor on >s3cret ~* &* +@all
CONF
redis-server redis-primary.conf
redis-server redis-replica.conf

step "mariadb"
mkdir -p /run/mysqld && chown mysql:mysql /run/mysqld
for n in 1 2; do
    mariadb-install-db --user=mysql --datadir=/var/lib/mysql$n >/dev/null 2>&1
done
mariadbd --user=mysql --datadir=/var/lib/mysql1 --port=3306 --bind-address=127.0.0.1 \
    --socket=/run/mysqld/m1.sock --pid-file=/run/mysqld/m1.pid --skip-log-bin \
    --ssl-cert="$D/server.crt" --ssl-key="$D/server.key" --ssl-ca="$D/ca.pem" \
    >"$D/mariadb1.log" 2>&1 &
mariadbd --user=mysql --datadir=/var/lib/mysql2 --port=3307 --bind-address=127.0.0.1 \
    --socket=/run/mysqld/m2.sock --pid-file=/run/mysqld/m2.pid --skip-log-bin \
    --read-only=1 >"$D/mariadb2.log" 2>&1 &
for n in 1 2; do
    i=0
    until mariadb-admin --socket=/run/mysqld/m$n.sock ping >/dev/null 2>&1; do
        i=$((i + 1)); [ $i -lt 60 ] || fail "mariadb $n did not start"; sleep 0.5
    done
    mariadb --socket=/run/mysqld/m$n.sock -e "
        CREATE USER 'emfor'@'localhost' IDENTIFIED BY 's3cret';
        CREATE USER 'emfor'@'127.0.0.1' IDENTIFIED BY 's3cret';"
done

step "postgresql"
PG=$(ls -d /usr/lib/postgresql/*/bin | tail -1)
mkdir -p /tmp/pg && chown postgres:postgres /tmp/pg
cp server.key /tmp/pg/server.key && chown postgres:postgres /tmp/pg/server.key && chmod 0600 /tmp/pg/server.key
runuser -u postgres -- "$PG/initdb" -D /tmp/pg/primary -U postgres >/dev/null
cat >> /tmp/pg/primary/postgresql.conf <<CONF
listen_addresses = '127.0.0.1'
port = 5432
unix_socket_directories = '/tmp/pg'
ssl = on
ssl_cert_file = '$D/server.crt'
ssl_key_file = '/tmp/pg/server.key'
CONF
cat > /tmp/pg/primary/pg_hba.conf <<'HBA'
local all all trust
host replication postgres 127.0.0.1/32 trust
host all postgres 127.0.0.1/32 trust
host all all 127.0.0.1/32 scram-sha-256
HBA
runuser -u postgres -- "$PG/pg_ctl" -D /tmp/pg/primary -l /tmp/pg/primary.log -w start >/dev/null
runuser -u postgres -- psql -q -h /tmp/pg -p 5432 -U postgres \
    -c "CREATE ROLE emfor LOGIN PASSWORD 's3cret'" -c "CREATE DATABASE app OWNER emfor"
runuser -u postgres -- "$PG/pg_basebackup" -h 127.0.0.1 -p 5432 -U postgres \
    -D /tmp/pg/standby -R -X stream
cat >> /tmp/pg/standby/postgresql.conf <<'CONF'
port = 5433
ssl = off
CONF
runuser -u postgres -- "$PG/pg_ctl" -D /tmp/pg/standby -l /tmp/pg/standby.log -w start >/dev/null
i=0
until [ "$(runuser -u postgres -- psql -tA -h /tmp/pg -p 5433 -U postgres -c 'SELECT pg_is_in_recovery()')" = t ]; do
    i=$((i + 1)); [ $i -lt 60 ] || fail "postgres standby did not start"; sleep 0.5
done
i=0
until redis-cli -p 6380 ROLE | head -1 | grep -qx slave; do
    i=$((i + 1)); [ $i -lt 60 ] || fail "redis replica did not start"; sleep 0.5
done

step "emfor-check against the servers"
EMFOR_LIVE_DIR="$D" emfor-check-live-test --ignored --test-threads=1

step "emfor rejects an unknown plugin argument"
mkdir -p /etc/emfor /var/lib/emfor
cat > /etc/emfor/bad.toml <<'CFG'
[plugins."kv"]
command = ["/src/target/debug/emfor-check", "memcached"]

[services."kv.svc.emfor"]
check = "kv"
backends = ["127.0.0.1:11211"]
CFG
if emfor --check-config --config /etc/emfor/bad.toml >"$D/bad.out" 2>&1; then
    fail "config with emfor-check memcached was accepted"
fi
grep -q 'kv' "$D/bad.out" || fail "error does not name plugin kv: $(cat "$D/bad.out")"

step "emfor --check-config shows emfor-check validation errors"
cat > /etc/emfor/mysql-bad.toml <<'CFG'
[plugins."mysql"]
command = ["/src/target/debug/emfor-check", "mysql"]

[services."app.svc.emfor"]
check = "mysql"
backends = ["127.0.0.1:3306"]
check_options = {}
CFG
if emfor --check-config --config /etc/emfor/mysql-bad.toml >"$D/mysql-bad.out" 2>&1; then
    fail "mysql service without user was accepted"
fi
grep -qF 'services."app.svc.emfor".check_options.user: missing required key' "$D/mysql-bad.out" \
    || fail "unexpected error: $(cat "$D/mysql-bad.out")"

step "emfor failover follows the PostgreSQL primary"
cat > /etc/emfor/emfor.toml <<CFG
[dns]
listen = ["127.0.0.1:5300"]

[plugins."pgsql"]
command = ["/src/target/debug/emfor-check", "pgsql"]

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
backends = ["127.0.0.1:5433", "127.0.0.1:5432"]
interval = "1s"
timeout = "900ms"
rise = 1
fall = 1
check_options = { user = "emfor", database = "app", role = "primary", password_file = "$D/good.pass" }
CFG
emfor --check-config --config /etc/emfor/emfor.toml
emfor --config /etc/emfor/emfor.toml >"$D/emfor.log" 2>&1 &
i=0
until dig @127.0.0.1 -p 5300 +short SRV db.svc.emfor | grep -q '^0 .* 5432 '; do
    i=$((i + 1))
    if [ $i -ge 60 ]; then
        dig @127.0.0.1 -p 5300 SRV db.svc.emfor || true
        cat "$D/emfor.log"
        fail "SRV priority 0 is not the primary on port 5432"
    fi
    sleep 0.25
done
dig @127.0.0.1 -p 5300 +short SRV db.svc.emfor
if dig @127.0.0.1 -p 5300 +short SRV db.svc.emfor | grep -q ' 5433 '; then
    fail "the standby on port 5433 is a candidate"
fi
i=0
until grep -q 'role is standby, want primary' "$D/emfor.log"; do
    i=$((i + 1)); [ $i -lt 20 ] || fail "emfor log has no standby reason: $(cat "$D/emfor.log")"; sleep 0.25
done
if grep -q 's3cret' "$D/emfor.log"; then
    fail "emfor log shows the password"
fi

echo "PASS all plugin tests"
