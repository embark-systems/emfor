#!/bin/sh
# Usage: tests/vector/journald.sh [vector image]   (default timberio/vector:latest-debian)
# emfor as a systemd unit in a container, Vector reading the journal.
# Needs a privileged container with systemd as PID 1.
set -eu
image="${1:-timberio/vector:latest-debian}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
tag="emfor-vector-journald-test"
docker build -q -f tests/vector/Dockerfile.journald --build-arg "VECTOR_IMAGE=$image" -t "$tag" . >/dev/null
cid=$(docker run -d --rm --privileged --cgroupns=host -v /sys/fs/cgroup:/sys/fs/cgroup:rw "$tag")
trap 'docker stop -t 2 "$cid" >/dev/null 2>&1 || true' EXIT
t=0
until docker exec "$cid" systemctl is-system-running 2>/dev/null | grep -qE 'running|degraded'; do
    t=$((t + 1)); [ "$t" -lt 60 ] || { echo "FAIL: systemd did not start" >&2; exit 1; }
    sleep 1
done
docker exec "$cid" sh /journald-inside.sh
