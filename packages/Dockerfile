# The image that builds the deb and rpm packages (packages/build.sh).
#
# Bullseye (glibc 2.31), so that the binaries run on Debian 11 and later,
# Ubuntu 20.04 and later, and RHEL 9 and its relatives (glibc 2.34).
#
#   docker build -t emfor-packages packages
FROM rust:1-bullseye
RUN apt-get update && apt-get install -y --no-install-recommends \
      pkg-config build-essential dpkg-dev && \
    rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-deb cargo-generate-rpm --locked && \
    rm -rf /usr/local/cargo/registry
