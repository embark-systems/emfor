# Builds emfor and a Debian image with dnsmasq and unbound, for run.sh.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --locked --bin emfor

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends dnsmasq unbound dns-root-data dnsutils netcat-openbsd ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/debug/emfor /usr/local/bin/emfor
COPY docs/deploy/resolver /resolver
COPY tests/resolver/inside.sh /inside.sh
ENTRYPOINT ["/bin/sh", "/inside.sh"]
