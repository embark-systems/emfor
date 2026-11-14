# emfor

English | [繁體中文](README.zh-TW.md)

emfor is a health-check daemon that runs on every application host. It checks the backends of your services (TCP, HTTP, scripts, plugins for redis, MySQL, PostgreSQL and LDAP) and publishes their health in a local state store. Applications ask a small client library for a healthy backend; the answer comes from a local file, so a lookup sends nothing over the network and keeps working when another host is down.

```
              checks over the network
  backends <------------------------------ emfor daemon ---------> DNS on 127.0.0.1
                                               |  writes           (programs without a
                                               v                    client; answered from
                                     local LMDB state store         the daemon's memory)
                                               |  reads
                             +-----------------+-----------+
                             |                             |
                  client library (Rust, C,           emfor status
                  Go, Python, Perl, PHP)
```

- **Health checks**: `tcp` (with optional TLS, send and expect), `http`/`https`, `exec` for Nagios-style scripts, and long-running plugins. `emfor-check` ships plugins for redis, MySQL/MariaDB, PostgreSQL and LDAP. A backend turns healthy after `rise` passes and unhealthy after `fall` failures.
- **Choosing a backend**: per service, `balance` is `round-robin`, `sticky` (by an affinity key), `failover`, `weighted`, `locality` (same site first) or `fastest`. `on_all_down` decides what happens when no backend is healthy.
- **Clients**: one implementation in Rust, also built as the C library `libemfor` that the Go, Python, Perl and PHP clients wrap. Every client returns `ip:port` or the same error codes.
- **DNS**: an optional server for programs without a client, answering only its own zone (default `svc.emfor`) behind the host's resolver.
- **Operations**: `emfor status` shows what the daemon published, `/metrics` serves Prometheus metrics, and `SIGHUP` reloads the config when it is fully valid.

## Quick start

Build the daemon (Rust 2024 edition):

```sh
cargo build --release --bin emfor
```

Write a config with one service. This one keeps the store under `/tmp` so it runs without root:

```sh
mkdir -p /tmp/emfor
cat > /tmp/emfor/emfor.toml <<'EOF'
[store]
path = "/tmp/emfor/state.mdb"

[dns]

[metrics]

[services."web.svc.emfor"]
check = "http"
backends = ["127.0.0.1:8080"]
EOF
```

Give it a backend, check the config, and start the daemon:

```sh
python3 -m http.server 8080 --bind 127.0.0.1 &
target/release/emfor check-config --config /tmp/emfor/emfor.toml
target/release/emfor server --config /tmp/emfor/emfor.toml
```

In another terminal, look at the result:

```console
$ target/release/emfor status --config /tmp/emfor/emfor.toml
store: /tmp/emfor/state.mdb
daemon: running, heartbeat 0.0s ago, up 4.0s, dns 127.0.0.1:5300, metrics 127.0.0.1:5301

web.svc.emfor  round-robin  on_all_down=open
  127.0.0.1:8080  healthy  1.6s ago  4.5ms

$ dig +short @127.0.0.1 -p 5300 web.svc.emfor
127.0.0.1

$ curl -s 127.0.0.1:5301/metrics | grep 'state="healthy"'
emfor_backend_state{service="web.svc.emfor",backend="127.0.0.1:8080",state="healthy"} 1
```

Stop the server on port 8080: the next check shows the failure at once, and after three failed checks in a row (`fall = 3`, 2 seconds apart by default) the backend turns `unhealthy`. [examples/emfor.toml](examples/emfor.toml) shows every option with comments.

## Looking up a backend

```rust
match emfor_client::lookup("web.svc.emfor", None) {
    Ok(addr) => println!("connect to {addr}"), // "127.0.0.1:8080"
    Err(e) => eprintln!("{e}"),                // e.g. "ALL_DOWN: ..."
}
```

The clients for other languages work the same way; each README covers installing and the error codes:

| Language | Client |
| --- | --- |
| Rust | [clients/rust](clients/rust/README.md) |
| C (`libemfor`) | [clients/c](clients/c/README.md) |
| Go | [clients/go](clients/go/README.md) |
| Python | [clients/python](clients/python/README.md) |
| Perl | [clients/perl](clients/perl/README.md) |
| PHP | [clients/php](clients/php/README.md) |

The C library, and every client built on it, must come from the same emfor release as the daemon: both use the same LMDB build, and a system liblmdb cannot read the store.

## Documentation

| Topic | Document |
| --- | --- |
| User manual: concepts, installation, configuration, checks, clients, DNS, operations, troubleshooting | [docs/manual/](docs/manual/README.md) |
| Installing, `emfor status`, systemd, metrics and alerting | [docs/deploy/daemon.md](docs/deploy/daemon.md) |
| Forwarding the DNS zone from dnsmasq, unbound, systemd-resolved or macOS | [docs/deploy/dns-resolver.md](docs/deploy/dns-resolver.md) |
| redis, MySQL, PostgreSQL and LDAP checks with `emfor-check` | [docs/deploy/plugins.md](docs/deploy/plugins.md) |
| Lookup latency and throughput, measured | [docs/performance.md](docs/performance.md) |
| Writing a checker plugin | [docs/plugins/protocol-v1.md](docs/plugins/protocol-v1.md) |
| Every config option | [examples/emfor.toml](examples/emfor.toml) |
| systemd unit, FreeBSD rc.d script and port, Prometheus alerting rules | [examples/](examples/) |
| What stays compatible across upgrades | [docs/compatibility.md](docs/compatibility.md) |
| Making a release | [RELEASING.md](RELEASING.md) |
| Requirements (specs) | [docs/spectra/specs/](docs/spectra/specs/) |

## Platforms

Linux and FreeBSD are the deployment platforms; macOS is for development. On FreeBSD, the tests, the rc.d script and the example port have been run on FreeBSD 15.1-RELEASE (arm64, in a VM); 14.x and amd64 are covered only by the optional CI job.

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/emfor-daemon` | The daemon and the `emfor` command |
| `crates/emfor-format` | State store keys and record format (version 2) |
| `crates/emfor-check` | The `emfor-check` plugin program |
| `crates/emfor-plugin` | Writing plugins in Rust |
| `crates/emfor-tls` | TLS shared by the `tcp` checker and `emfor-check` |
| `clients/` | Client libraries |
| `tests/` | Cross-language conformance cases, plugin, resolver and container tests |
| `tools/emfor-fixture` | Writes conformance cases into store files |

## Development

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
make -C clients test
```

Changes are planned as specs and change proposals with [Spectra](docs/spectra/): requirements live in `docs/spectra/specs/`, finished changes in `docs/spectra/changes/archive/`.

To contribute, read [CONTRIBUTING.md](CONTRIBUTING.md) (commits are signed off under the DCO) and the [code of conduct](CODE_OF_CONDUCT.md). Report security problems privately as described in [SECURITY.md](SECURITY.md).

## License

BSD 2-Clause; see [LICENSE](LICENSE).
