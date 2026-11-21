#!/bin/sh
# Usage: tests/prometheus/run.sh [prometheus image]   (default prom/prometheus)
# Builds emfor in Docker next to a Prometheus (its binary, in an nginx image
# that serves two scraped backends) and checks the recipe of
# docs/manual/14-integrations.md, section "Prometheus": Prometheus finds
# its scrape targets with dns_sd_configs of type SRV on emfor's answers.
set -eu
image="${1:-prom/prometheus}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
tag="emfor-prometheus-test-$(echo "$image" | tr -c 'a-zA-Z0-9\n' '-')"
docker build -q -f tests/prometheus/Dockerfile --build-arg "PROM_IMAGE=$image" -t "$tag" . >/dev/null
docker run --rm "$tag"
