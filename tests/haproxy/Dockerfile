# HAProxy with emfor, for tests/haproxy/run.sh: the recipe of
# docs/manual/14-integrations.md, section "HAProxy".
#
#   docker build -f tests/haproxy/Dockerfile --build-arg HAPROXY_IMAGE=haproxy:2.8 .
ARG HAPROXY_IMAGE=haproxy:lts

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# Every workspace member must be present for cargo to load the workspace.
COPY tools tools
COPY clients/rust clients/rust
COPY tests/spike/probe tests/spike/probe
RUN cargo build --locked --bin emfor

FROM ${HAPROXY_IMAGE}
# The HAProxy images run as the haproxy user.
USER root
RUN apt-get update \
 && apt-get install -y --no-install-recommends curl dnsutils \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/debug/emfor /usr/local/bin/emfor
COPY tests/haproxy/inside.sh /inside.sh
COPY tests/haproxy/more.sh /more.sh
ENTRYPOINT ["/bin/sh", "/inside.sh"]
