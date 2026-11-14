#!/bin/sh
# Usage: tests/plugins/mysql.sh [MySQL version]   (default 8.4)
# Checks the mysql plugin against MySQL, which Debian does not package:
# builds emfor-check in Docker, starts three mysql:<version> containers
# that share one network namespace, and runs mysql_live from
# crates/emfor-check/tests/live.rs with the layout it expects for MariaDB
# in tests/plugins/run.sh: 3306 a primary with TLS, 3307 a read-only
# replica of 3306 without TLS, 3308 a read-only replica of 3399 (nothing
# there) without TLS. Every container and the volume are removed at exit.
set -eu
tag=${1:-8.4}
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
net=emfor-mysql-net; vol=emfor-mysql-live
fail() { echo "FAIL: $*" >&2; exit 1; }
cleanup() { docker rm -f emfor-mysql1 emfor-mysql2 emfor-mysql3 $net >/dev/null 2>&1 || true; docker volume rm $vol >/dev/null 2>&1 || true; }
cleanup
trap cleanup EXIT
docker build -q -f tests/plugins/Dockerfile.mysql -t emfor-mysql-test . >/dev/null
docker volume create $vol >/dev/null
docker run -d --name $net -v $vol:/live emfor-mysql-test sleep infinity >/dev/null
docker exec $net sh -c '
cd /live
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
    -keyout ca.key -out ca.pem -days 2 -subj "/CN=emfor live test CA" 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
    -keyout server.key -out server.csr -subj "/CN=db.test" 2>/dev/null
printf "subjectAltName = DNS:db.test\nbasicConstraints = CA:FALSE\nkeyUsage = digitalSignature\nextendedKeyUsage = serverAuth\n" > server.ext
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
    -out server.crt -days 2 -extfile server.ext 2>/dev/null
chmod 0644 server.key
printf "s3cret\n" > good.pass
printf "wrong-pw\n" > bad.pass'

# X Protocol is off so the three servers do not all claim port 33060. MySQL
# turns TLS on by itself; --tls-version= turns it off on the replicas.
common="--bind-address=127.0.0.1 --mysqlx=OFF"
docker run -d --name emfor-mysql1 --network container:$net -v $vol:/live -e MYSQL_ALLOW_EMPTY_PASSWORD=1 \
    mysql:$tag $common --port=3306 --server-id=1 --log-bin=mysql-bin \
    --ssl-cert=/live/server.crt --ssl-key=/live/server.key --ssl-ca=/live/ca.pem >/dev/null
for n in 2 3; do
    docker run -d --name emfor-mysql$n --network container:$net -e MYSQL_ALLOW_EMPTY_PASSWORD=1 \
        mysql:$tag $common --port=330$((n + 5)) --server-id=$n --read-only=1 --tls-version= >/dev/null
done
# The emfor user needs REPLICATION CLIENT for SHOW REPLICA STATUS; it is
# kept out of the binary log so that 3307 does not replay it.
sql() { docker exec emfor-mysql$1 mysql -uroot -h127.0.0.1 -P330$(($1 + 5)) -e "$2"; }
for n in 1 2 3; do
    i=0
    until sql $n 'SELECT 1' >/dev/null 2>&1; do
        i=$((i + 1)); [ $i -lt 120 ] || { docker logs emfor-mysql$n | tail -20; fail "mysql $n did not start"; }; sleep 1
    done
    sql $n "SET SESSION sql_log_bin = 0;
        CREATE USER 'emfor'@'127.0.0.1' IDENTIFIED BY 's3cret';
        GRANT REPLICATION CLIENT ON *.* TO 'emfor'@'127.0.0.1';"
done
sql 1 "SET SESSION sql_log_bin = 0;
    CREATE USER 'repl'@'127.0.0.1' IDENTIFIED BY 'repl-pw';
    GRANT REPLICATION SLAVE ON *.* TO 'repl'@'127.0.0.1';"
for n in 2 3; do
    port=3306; [ $n = 2 ] || port=3399
    sql $n "CHANGE REPLICATION SOURCE TO SOURCE_HOST='127.0.0.1', SOURCE_PORT=$port,
        SOURCE_USER='repl', SOURCE_PASSWORD='repl-pw', SOURCE_CONNECT_RETRY=1,
        GET_SOURCE_PUBLIC_KEY=1; START REPLICA;"
done
i=0
until [ "$(sql 2 'SHOW REPLICA STATUS\G' | grep -cE 'Replica_(IO|SQL)_Running: Yes')" = 2 ]; do
    i=$((i + 1)); [ $i -lt 60 ] || { sql 2 'SHOW REPLICA STATUS\G'; fail "replica did not start"; }; sleep 1
done
echo "== mysql $(sql 1 'SELECT VERSION()' | tail -1): servers ready"
# MySQL 8 logs users in with caching_sha2_password; show it.
sql 1 "SELECT user, plugin FROM mysql.user WHERE user IN ('emfor','repl')"
docker exec -e EMFOR_LIVE_DIR=/live $net emfor-check-live-test --ignored --exact mysql_live
