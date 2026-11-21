#!/bin/sh
# Usage: tests/nginx/run.sh [nginx image]      (default nginx:mainline)
# Builds emfor in Docker next to an nginx image and checks the recipe of
# docs/manual/14-integrations.md, section "nginx": nginx takes its
# upstream list from emfor's SRV answers (nginx 1.27.3 or later), follows
# health and drain without a reload, honours weights and priorities, and keeps
# its last list when emfor cannot answer; an older nginx uses the A record.
# stream.sh checks the `stream` module (TCP) the same way.
set -eu
image="${1:-nginx:mainline}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
tag="emfor-nginx-test-$(echo "$image" | tr -c 'a-zA-Z0-9\n' '-')"
docker build -q -f tests/nginx/Dockerfile --build-arg "NGINX_IMAGE=$image" -t "$tag" . >/dev/null
docker run --rm "$tag"
if docker run --rm --entrypoint nginx "$tag" -V 2>&1 | grep -q -- --with-stream; then
    docker run --rm --entrypoint /bin/sh "$tag" /stream.sh
fi
