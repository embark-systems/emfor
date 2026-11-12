## 1. Warning

- [x] 1.1 "Services with backends on different ports are reported": a pure function in crates/emfor-daemon/src/cli.rs returns one message per service under the `[dns]` zone whose backends (both address families together) do not share one port, naming the service, the distinct ports in ascending order joined by `, `, and saying that A/AAAA answers carry no port and SRV or a client gives it; it returns nothing without `[dns]` and nothing for services outside the zone. The startup path and the successful-reload path log each message at `warn`, next to the existing `warn_outside_zone` call. Verified by a unit test in cli.rs reproducing every row of the "which services are reported" example, and by an integration test in crates/emfor-daemon/tests/daemon.rs that starts the daemon with all backends on one port, finds no such warning, reloads (SIGHUP) with a backend on another port, and finds the `warn` line with the service name and both ports in stderr.

## 2. Documentation

- [x] 2.1 [after: 1.1] docs/deploy/dns-resolver.md "Check emfor first" says that the daemon warns about services in the zone whose backends use different ports, and that such services need SRV or a client to get the port; docs/spectra/config.yaml moves the port-consistency item from the follow-up list to the DNS description as implemented (emfor-dns-port-warning). Verified by content review and `grep -n "different ports" docs/deploy/dns-resolver.md`.

## 3. Final check

- [x] 3.1 [after: 1.1, 2.1] `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings` and `cargo test --workspace --all-features` pass on macOS and in the Debian test image. Verified by the command output.
