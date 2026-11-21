#!/bin/sh
# Usage: tests/srv/run.sh
# Builds emfor in Docker next to MIT Kerberos, OpenLDAP's ldapsearch and PyMongo
# (Debian bookworm packages) and checks that each finds its servers through
# emfor's SRV answers, and follows a failing server out of the list: the
# recipe of docs/manual/14-integrations.md, section "Programs that ask for SRV records
# themselves". emfor is the container's only resolver, on port 53.
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
docker build -q -f tests/srv/Dockerfile -t emfor-srv-test . >/dev/null
docker run --rm emfor-srv-test
