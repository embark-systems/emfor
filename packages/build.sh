#!/bin/sh
# Build the deb and rpm packages of emfor in a container, into packages/dist/:
#
#   packages/build.sh                       # for this machine's architecture
#   PLATFORM=linux/amd64 packages/build.sh  # for another one (slow if emulated)
#
# The container has an old glibc (packages/Dockerfile), so the packages run on
# Debian 11 and later, Ubuntu 20.04 and later and RHEL 9 and its relatives.
# The build keeps its target directory in a docker volume, so that the next
# build is incremental. The packages are not signed.
set -eu
cd "$(dirname "$0")/.."
platform=${PLATFORM:-}
tag=emfor-packages${platform:+-$(echo "$platform" | tr '/' '-')}
docker image inspect "$tag" >/dev/null 2>&1 \
    || docker build -q ${platform:+--platform "$platform"} -t "$tag" packages
mkdir -p packages/dist
exec docker run --rm ${platform:+--platform "$platform"} \
    -v "$PWD":/src -w /src \
    -v "$tag-cargo":/usr/local/cargo/registry \
    -v "$tag-target":/src/target \
    -e CARGO_TARGET_DIR=/src/target \
    "$tag" sh -c '
        set -eu
        cargo build --release --locked -p emfor-daemon -p emfor-check
        rm -f packages/dist/*.deb packages/dist/*.rpm
        cargo deb -p emfor-daemon --no-build -o packages/dist/
        cargo generate-rpm -p crates/emfor-daemon -o packages/dist/
        ls -l packages/dist
    '
