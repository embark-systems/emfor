#!/bin/sh
# Time to seal and open a peer datagram in EMP1 to EMP4
# (crates/emfor-daemon/src/peers/crypto_bench.rs). Runs in the Debian
# container like the other tests; takes about a minute. Numbers depend on
# the CPU, so compare formats within one run only.
set -eu
cd "$(dirname "$0")/.."
exec tools/docker-test.sh 'cargo test --release -p emfor-daemon --lib peers::crypto_bench -- --ignored --nocapture --test-threads=1'
