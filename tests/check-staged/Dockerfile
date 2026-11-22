# Tests examples/deploy/emfor-check-staged.sh (docs/manual/05-configuring.md,
# "Using a configuration management tool"), for tests/check-staged/run.sh.
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
RUN useradd --system --no-create-home emfor
COPY --from=build /src/target/debug/emfor /usr/bin/emfor-real
COPY examples/deploy/emfor-check-staged.sh /usr/local/sbin/emfor-check-staged.sh
COPY tests/check-staged/inside.sh /inside.sh
ENTRYPOINT ["/bin/sh", "/inside.sh"]
