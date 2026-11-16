# emfor with the Nagios examples, for tests/nagios/run.sh: examples/nagios/
# check_emfor.sh and examples/nagios/emfor-exec.sh, as docs/manual/08-operations.md,
# section "Watching emfor from Nagios", says.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# Every workspace member must be present for cargo to load the workspace.
COPY tools tools
COPY clients/rust clients/rust
COPY tests/spike/probe tests/spike/probe
RUN cargo build --locked --bin emfor

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends jq \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/debug/emfor /usr/local/bin/emfor
COPY examples/nagios/check_emfor.sh /usr/local/bin/check_emfor.sh
COPY examples/nagios/emfor-exec.sh /usr/local/bin/emfor-exec.sh
COPY tests/nagios/inside.sh /inside.sh
ENTRYPOINT ["/bin/sh", "/inside.sh"]
