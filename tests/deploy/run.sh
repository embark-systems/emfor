#!/bin/sh
# Usage: tests/deploy/run.sh [nginx image]      (default nginx:mainline)
# Builds emfor in Docker next to an nginx image (1.27.3 or later: its upstream
# comes from emfor's SRV answers) and rolls a deploy over two backends with
# examples/deploy/emfor-drain.sh, as docs/manual/12-operating.md, section
# "A rolling deploy with drain", says: no request fails while a backend is
# drained and stopped, and a request already in flight finishes.
set -eu
image="${1:-nginx:mainline}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
tag="emfor-deploy-test-$(echo "$image" | tr -c 'a-zA-Z0-9\n' '-')"
docker build -q -f tests/deploy/Dockerfile --build-arg "NGINX_IMAGE=$image" -t "$tag" . >/dev/null
docker run --rm "$tag"
