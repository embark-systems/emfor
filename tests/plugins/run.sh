#!/bin/sh
# Usage: tests/plugins/run.sh
# Builds emfor and emfor-check in Docker and checks the redis, mysql,
# pgsql, ldap, dns, radius, sip, diameter, smtp, amqp, mongodb and grpc
# plugins against real servers (see crates/emfor-check/tests/live.rs).
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
docker build -q -f tests/plugins/Dockerfile -t emfor-plugins-test . >/dev/null
docker run --rm emfor-plugins-test
