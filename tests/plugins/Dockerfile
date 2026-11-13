# Builds emfor, emfor-check and its live tests, and a Debian image with
# redis, MariaDB, PostgreSQL, OpenLDAP, unbound, FreeRADIUS, Kamailio,
# freeDiameter, Postfix, RabbitMQ and MongoDB, for run.sh.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# Every workspace member must be present for cargo to load the workspace.
COPY tools tools
COPY clients/rust clients/rust
COPY tests/spike/probe tests/spike/probe
RUN cargo build --locked --bin emfor --bin emfor-check
# The live test binary has the emfor-check path built in, so the runtime
# image keeps emfor-check at the same path.
RUN cargo test --locked -p emfor-check --test live --no-run --message-format=json \
    | grep -o '"executable":"[^"]*"' | grep '/live-' | cut -d'"' -f4 \
    | xargs -I{} cp {} /src/live-test

FROM debian:bookworm-slim
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends redis-server redis-tools mariadb-server mariadb-client \
    postgresql postgresql-client openssl dnsutils ca-certificates procps slapd ldap-utils unbound freeradius kamailio kamailio-tls-modules \
    freediameterd freediameter-extensions postfix rabbitmq-server \
 && rm -rf /var/lib/apt/lists/*
# MongoDB is not in Debian: use MongoDB's own repository. It has no arm64
# server package for Debian, so arm64 takes the Ubuntu 22.04 one.
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends curl gnupg \
 && curl -fsSL https://pgp.mongodb.com/server-8.0.asc | gpg --dearmor -o /usr/share/keyrings/mongodb-8.0.gpg \
 && if [ "$(dpkg --print-architecture)" = arm64 ]; then \
      echo "deb [arch=arm64 signed-by=/usr/share/keyrings/mongodb-8.0.gpg] https://repo.mongodb.org/apt/ubuntu jammy/mongodb-org/8.0 multiverse"; \
    else \
      echo "deb [signed-by=/usr/share/keyrings/mongodb-8.0.gpg] https://repo.mongodb.org/apt/debian bookworm/mongodb-org/8.0 main"; \
    fi > /etc/apt/sources.list.d/mongodb-org-8.0.list \
 && apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends mongodb-org-server mongodb-mongosh \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/debug/emfor /usr/local/bin/emfor
COPY --from=build /src/target/debug/emfor-check /src/target/debug/emfor-check
COPY --from=build /src/live-test /usr/local/bin/emfor-check-live-test
COPY tests/plugins/inside.sh /inside.sh
ENTRYPOINT ["/bin/sh", "/inside.sh"]
