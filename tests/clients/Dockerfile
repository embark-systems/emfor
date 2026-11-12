# The Debian image for the local checks (RELEASING.md, step 4): Rust,
# cargo-c, and the toolchains of the Go, Python, Perl and PHP clients.
#
#   docker build -t emfor-clients-test tests/clients
FROM rust:1-trixie
RUN apt-get update && apt-get install -y --no-install-recommends \
      pkg-config golang python3 perl libperl-dev php-dev php-cli php-dba && \
    rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-c --locked --features vendored-openssl && rm -rf /usr/local/cargo/registry
