# Prometheus with emfor, for tests/prometheus/run.sh: the recipe of
# docs/manual/07-dns.md, section "Using it with Prometheus". The prometheus
# binary (static) is taken from its image into an nginx image, which also
# serves as the scraped backends.
#
#   docker build -f tests/prometheus/Dockerfile --build-arg PROM_IMAGE=prom/prometheus .
ARG PROM_IMAGE=prom/prometheus
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

FROM ${PROM_IMAGE} AS prom

FROM ${NGINX_IMAGE}
RUN apt-get update \
 && apt-get install -y --no-install-recommends dnsutils jq \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/debug/emfor /usr/local/bin/emfor
COPY --from=prom /bin/prometheus /usr/local/bin/prometheus
COPY tests/prometheus/inside.sh /inside.sh
ENTRYPOINT ["/bin/sh", "/inside.sh"]
