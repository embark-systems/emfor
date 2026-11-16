# nginx with emfor, for tests/nginx/run.sh: the recipe of
# docs/manual/07-dns.md, section "Using it with nginx".
#
#   docker build -f tests/nginx/Dockerfile --build-arg NGINX_IMAGE=nginx:stable .
ARG NGINX_IMAGE=nginx:mainline

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# Every workspace member must be present for cargo to load the workspace.
COPY tools tools
COPY clients/rust clients/rust
COPY tests/spike/probe tests/spike/probe
RUN cargo build --locked --bin emfor

FROM ${NGINX_IMAGE}
RUN apt-get update \
 && apt-get install -y --no-install-recommends dnsutils \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/debug/emfor /usr/local/bin/emfor
COPY tests/nginx/inside.sh /inside.sh
COPY tests/nginx/stream.sh /stream.sh
ENTRYPOINT ["/bin/sh", "/inside.sh"]
