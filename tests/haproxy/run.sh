#!/bin/sh
# Usage: tests/haproxy/run.sh [haproxy image]      (default haproxy:lts)
# Builds emfor in Docker next to a HAProxy image and checks the recipe of
# docs/manual/07-dns.md, section "Using it with HAProxy": HAProxy fills the
# slots of a server-template from emfor's SRV answers, follows health and
# drain without a reload, and keeps its servers while emfor does not answer.
# What HAProxy does not do with the answer (priorities, weights) is checked
# too, so that the manual stays true.
set -eu
image="${1:-haproxy:lts}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
tag="emfor-haproxy-test-$(echo "$image" | tr -c 'a-zA-Z0-9\n' '-')"
docker build -q -f tests/haproxy/Dockerfile --build-arg "HAPROXY_IMAGE=$image" -t "$tag" . >/dev/null
docker run --rm "$tag"
