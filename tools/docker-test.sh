#!/bin/sh
# Run a command in the Debian test image (the routine gate of RELEASING.md):
#   tools/docker-test.sh 'cargo test --workspace --all-features'
# With no argument it runs the whole gate: fmt, clippy and the tests.
# The image is built from tests/clients (images get pruned: the script
# builds it when it is missing). The named volumes keep the cargo registry and
# the build output between runs.
set -eu
cd "$(dirname "$0")/.."
docker image inspect emfor-clients-test >/dev/null 2>&1 \
    || docker build -q -t emfor-clients-test tests/clients
cmd=${1:-'cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace --all-features && cargo test -p emfor-client --doc'}
exec docker run --rm -v "$PWD":/src -w /src \
    -v emfor-clients-cargo:/usr/local/cargo/registry \
    -v emfor-clients-target:/src/target \
    -v emfor-clients-gocache:/root/.cache/go-build \
    emfor-clients-test sh -c "rustup component add clippy rustfmt >/dev/null 2>&1; $cmd"
