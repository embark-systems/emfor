#!/bin/sh
# Usage: tests/nagios/run.sh
# Builds emfor in Docker and checks the examples of docs/manual/12-operating.md,
# section "Nagios, Icinga and other monitoring": check_emfor.sh gives the Nagios status
# of emfor's state, and emfor-exec.sh runs a Nagios-style plugin as an `exec`
# check. The plugin is a stand-in that says what a file tells it to.
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
docker build -q -f tests/nagios/Dockerfile -t emfor-nagios-test . >/dev/null
docker run --rm emfor-nagios-test
