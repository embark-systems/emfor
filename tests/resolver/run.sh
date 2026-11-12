#!/bin/sh
# Usage: tests/resolver/run.sh <dnsmasq|unbound>
# Builds emfor in Docker and checks that the resolver recipe from
# docs/deploy/dns-resolver.md forwards the emfor zone.
set -eu
resolver="${1:?usage: $0 <dnsmasq|unbound>}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

# The guide must show every snippet file verbatim.
python3 - <<'PY'
import pathlib, sys
guide = pathlib.Path("docs/deploy/dns-resolver.md").read_text()
bad = [p.name for p in sorted(pathlib.Path("docs/deploy/resolver").iterdir())
       if p.read_text().rstrip("\n") not in guide]
if bad:
    sys.exit("guide does not contain these snippets verbatim: " + ", ".join(bad))
PY

docker build -q -f tests/resolver/Dockerfile -t emfor-resolver-test . >/dev/null
docker run --rm emfor-resolver-test "$resolver"
