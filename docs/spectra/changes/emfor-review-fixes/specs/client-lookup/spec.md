## ADDED Requirements

### Requirement: Naming the store is not public API

The function that makes a client for an explicit store path (`Client::for_store`) SHALL exist only when the `daemon-internal` cargo feature of the Rust crate is enabled, which `emfor` (for `emfor lookup`) enables and `libemfor` does not: `libemfor` is built from the `emfor-client` package alone (`cargo cinstall --features capi` in `clients/rust`), so the daemon and its feature request are not part of that build, and a Rust method is never an exported symbol of the C ABI. The constructor that takes a path and a staleness limit SHALL NOT be public. In a build without the `daemon-internal` and `testing` features, which is the default build and the one `libemfor` is built with (cargo-c, `capi` only), the public API of the Rust crate and of `libemfor` SHALL contain no function that configures a client other than through the environment variables. The `testing` feature, which exposes an explicit path and staleness for the crate's own tests, is the second exception and is not part of the public API.

#### Scenario: Default build

- **WHEN** the Rust crate is built without the `daemon-internal` and `testing` features
- **THEN** `Client::for_store` and `Client::new` are not part of its public API

#### Scenario: emfor lookup

- **WHEN** `emfor lookup` is built
- **THEN** it names the store with `Client::for_store`, through the `daemon-internal` feature
