#!/bin/sh
# Usage: tests/check-staged/run.sh
# Builds emfor in Docker and runs examples/deploy/emfor-check-staged.sh, the
# validation command for Ansible, Puppet and Salt, on good and bad candidate
# files (docs/manual/05-configuring.md, "Using a configuration management
# tool"). The tools themselves are not part of this test.
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
docker build -q -f tests/check-staged/Dockerfile -t emfor-check-staged-test . >/dev/null
docker run --rm emfor-check-staged-test
