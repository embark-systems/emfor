#!/bin/sh
# Usage: tests/vector/run.sh [vector image]   (default timberio/vector:latest-debian)
# Builds emfor in Docker next to Vector and checks the recipe of
# docs/manual/14-integrations.md, section "Vector": Vector scrapes /metrics
# and reads the JSON log of emfor.
set -eu
image="${1:-timberio/vector:latest-debian}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
tag="emfor-vector-test-$(echo "$image" | tr -c 'a-zA-Z0-9\n' '-')"
docker build -q -f tests/vector/Dockerfile --build-arg "VECTOR_IMAGE=$image" -t "$tag" . >/dev/null
docker run --rm "$tag"
