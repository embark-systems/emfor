# Caddy with emfor, for tests/caddy/run.sh: the recipe of
# docs/manual/07-dns.md, section "Using it with Caddy". The caddy binary
# (static) is taken from its image into an nginx image, which also serves as
# the two backends.
#
#   docker build -f tests/caddy/Dockerfile --build-arg CADDY_IMAGE=caddy:2 .
ARG CADDY_IMAGE=caddy
ARG NGINX_IMAGE=nginx:stable

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# Every workspace member must be present for cargo to load the workspace.
COPY tools tools
COPY clients/rust clients/rust
COPY tests/spike/probe tests/spike/probe
RUN cargo build --locked --bin emfor

FROM ${CADDY_IMAGE} AS caddy

FROM ${NGINX_IMAGE}
RUN apt-get update \
 && apt-get install -y --no-install-recommends dnsutils \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/debug/emfor /usr/local/bin/emfor
COPY --from=caddy /usr/bin/caddy /usr/local/bin/caddy
COPY tests/caddy/inside.sh /inside.sh
ENTRYPOINT ["/bin/sh", "/inside.sh"]
