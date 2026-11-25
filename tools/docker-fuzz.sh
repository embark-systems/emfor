#!/bin/sh
# Usage: tools/docker-fuzz.sh [seconds per target]   (default 600)
# RELEASING.md step 5 in a Debian container: every fuzz target of fuzz/ for
# the given time, one after the other, on nightly with cargo-fuzz 0.13.2 (the
# version of .github/workflows/fuzz.yml). Named volumes keep the toolchain,
# cargo-fuzz and the build between runs; fuzz/corpus is on the host and is
# not committed. Ends 0 only when every target ended with "Done N runs" and
# fuzz/artifacts/ is empty.
set -eu
secs="${1:-600}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
exec docker run --rm -v "$root":/src -w /src/fuzz \
  -v emfor-fuzz-cargo:/usr/local/cargo/registry \
  -v emfor-fuzz-rustup:/usr/local/rustup \
  -v emfor-fuzz-cargobin:/usr/local/cargo/bin \
  -v emfor-fuzz-target:/src/fuzz/target \
  -e SECS="$secs" rust:1-bookworm sh -c '
    set -eu
    rustup toolchain install nightly --profile minimal >/dev/null
    command -v cargo-fuzz >/dev/null || cargo install cargo-fuzz --locked --version 0.13.2
    mkdir -p corpus/plugins corpus/peers corpus/dns
    run() { t=$1; shift; cargo +nightly fuzz run -O "$t" "$@" -- -max_total_time=$SECS 2>&1 | tail -3; }
    cargo +nightly fuzz run -O plugins corpus/plugins seeds/plugins -- -len_control=0 -max_total_time=$SECS 2>&1 | tail -3
    cargo +nightly fuzz run -O peers corpus/peers seeds/peers -- -max_total_time=$SECS 2>&1 | tail -3
    cargo +nightly fuzz run -O dns corpus/dns seeds/dns -- -max_total_time=$SECS 2>&1 | tail -3
    for t in metrics plugin_line config; do run "$t"; done
    test -z "$(ls artifacts 2>/dev/null)" && echo "no artifacts"'
