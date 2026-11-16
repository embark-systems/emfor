#!/bin/sh
# Usage: tests/caddy/run.sh [caddy image]      (default caddy)
# Builds emfor in Docker next to a Caddy (its binary, in an nginx image that
# serves two backends) and checks the recipe of docs/manual/07-dns-recipes.md, section
# "Using it with Caddy": reverse_proxy takes its upstreams from emfor's SRV
# answers with `dynamic srv`.
set -eu
image="${1:-caddy}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
tag="emfor-caddy-test-$(echo "$image" | tr -c 'a-zA-Z0-9\n' '-')"
docker build -q -f tests/caddy/Dockerfile --build-arg "CADDY_IMAGE=$image" -t "$tag" . >/dev/null
docker run --rm "$tag"
