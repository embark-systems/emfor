#!/bin/sh
# How fast a changed peer secret file is noticed, and what it costs while
# nothing changes, with the watch and with reading the file every second
# (crates/emfor-daemon/src/peers/task/bench.rs). Runs in the Debian
# container like the other tests; takes about six minutes. On macOS or
# FreeBSD run the cargo command by hand: only the latency is measured there.
set -eu
cd "$(dirname "$0")/.."
exec tools/docker-test.sh 'cargo test --release -p emfor-daemon --lib peers::task::bench -- --ignored --nocapture --test-threads=1'
