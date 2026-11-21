# emfor with clients that look for their servers with SRV records, for
# tests/srv/run.sh: MIT Kerberos (dns_lookup_kdc), OpenLDAP (a DNS-based URI)
# and PyMongo (mongodb+srv://), as docs/manual/14-integrations.md, section "Programs that
# ask for SRV records themselves", says.
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
 && apt-get install -y --no-install-recommends krb5-user ldap-utils python3 \
        python3-pymongo python3-dnspython dnsutils \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/debug/emfor /usr/local/bin/emfor
COPY tests/srv/inside.sh /inside.sh
ENTRYPOINT ["/bin/sh", "/inside.sh"]
