# Vector with emfor, for tests/vector/run.sh: the recipe of
# docs/manual/14-integrations.md, section "Vector". The Vector image is the
# base; emfor is built on bookworm and copied in (it runs on the newer glibc
# of the Vector image).
#
#   docker build -f tests/vector/Dockerfile --build-arg VECTOR_IMAGE=timberio/vector:latest-debian .
ARG VECTOR_IMAGE=timberio/vector:latest-debian

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# Every workspace member must be present for cargo to load the workspace.
COPY tools tools
COPY clients/rust clients/rust
COPY tests/spike/probe tests/spike/probe
RUN cargo build --locked --bin emfor

FROM ${VECTOR_IMAGE}
RUN apt-get update \
 && apt-get install -y --no-install-recommends netcat-openbsd jq curl \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/debug/emfor /usr/local/bin/emfor
COPY examples/vector/vector.yaml /etc/vector/emfor.yaml
COPY examples/vector/vector-stdin.yaml /etc/vector/emfor-stdin.yaml
COPY tests/vector/inside.sh /inside.sh
ENTRYPOINT ["/bin/sh", "/inside.sh"]
