# The image that builds the deb and rpm packages (packages/build.sh).
#
# Bullseye (glibc 2.31), so that the binaries run on Debian 11 and later,
# Ubuntu 20.04 and later, and RHEL 9 and its relatives (glibc 2.34).
#
#   docker build -t emfor-packages packages
# Pinned by digest, so that the packages are built on the same base until the
# pin is changed on purpose (Dependabot proposes the new digest). The tag is
# kept for the reader: `docker buildx imagetools inspect rust:1-bullseye`
# shows the digest it points to now.
FROM rust:1-bullseye@sha256:4730e387a220a08a365c77da3096544dde214f9d796c16284d4be45438cad4a9
RUN apt-get update && apt-get install -y --no-install-recommends \
      pkg-config build-essential dpkg-dev && \
    rm -rf /var/lib/apt/lists/*
# The versions of the two tools that make the packages, pinned like the base.
RUN cargo install --locked cargo-deb@3.8.0 cargo-generate-rpm@0.21.1 && \
    rm -rf /usr/local/cargo/registry
