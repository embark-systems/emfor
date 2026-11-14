# Fuzzing

Fuzz targets for the input emfor reads from other hosts and processes,
built with [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer)
on a nightly toolchain:

```sh
cargo install cargo-fuzz
rustup toolchain install nightly
cd fuzz
cargo +nightly fuzz run -O plugins corpus/plugins seeds/plugins -- -len_control=0 -max_total_time=600
cargo +nightly fuzz run -O peers corpus/peers seeds/peers -- -max_total_time=600
cargo +nightly fuzz run -O dns corpus/dns seeds/dns -- -max_total_time=600
```

`-O` builds with the release profile, which `Cargo.toml` here sets to keep
debug assertions and overflow checks on (`cargo fuzz`'s `-a` is not
needed): an integer overflow panics, so the fuzzer reports it as a crash
instead of running on with a wrapped value. Each input runs on a thread
with a 2 MiB stack, the size of a tokio worker thread
(`emfor_fuzz::on_worker_stack`, `src/lib.rs`), not on libFuzzer's 8 MiB
main thread, so a recursion too deep for the daemon or a plugin crashes
here too. Address sanitizer frames are larger than release frames, so the
fuzzer is somewhat stricter than production; `EMFOR_FUZZ_STACK=<bytes>`
sets another size to tell such a case from a real one.

`seeds/plugins/` holds well-formed answers for each plugin, written by
`seeds/plugins.py`; the first directory is where the fuzzer keeps what it
finds. `seeds/peers/` holds a `heartbeat`, a `down` and a `result` message
as `EMP1` and `EMP2` datagrams and as bare payloads, written by
`seeds/peers.py`; the first byte of a `peers` input picks which (see
`peers::fuzz_decode`). `seeds/dns/` holds the queries of the DNS server's
tests, written by `seeds/dns.py`; the first byte of a `dns` input picks UDP
or TCP, `svc.emfor` or the longest zone allowed, and before or after the
first layout (see `dns::fuzz_request`, which also lists the services it
answers for). A mutated `EMP2` datagram almost never passes the
decryption, so bare payloads are encrypted by the target itself to reach
the JSON parsing behind it. Without the seeds the fuzzer rarely gets past
a length field and an identifier that must match, and without
`-len_control=0` it cuts the seeds short at first.

All plugins share one run, so each gets about a tenth of it.
`EMFOR_FUZZ_PLUGIN=<name>` gives every input to that plugin instead; pass
only its seeds (`seeds/plugins/<name>-*`). For `grpc` the fuzzer mostly
breaks the HTTP/2 framing and seldom reaches the health message inside it:
a planted out-of-bounds read in `grpc-message` decoding was found in under
a minute, one in the protobuf decoding not in a million runs.

`EMFOR_FUZZ_PRINT=1` prints the outcome of each input, for example to
check the seeds: `EMFOR_FUZZ_PRINT=1 cargo +nightly fuzz run -O plugins
seeds/plugins/*`.

| Target | Input |
| --- | --- |
| `plugins` | What a backend answers to an `emfor-check` plugin: redis, ldap, dns, radius, sip, diameter, smtp, amqp, mongodb and grpc, each with a few sets of options |
| `peers` | A datagram from another member of a peer group, `EMP1` or `EMP2`, or a payload to encrypt as `EMP2` |
| `dns` | A request to the DNS server, over UDP or TCP, through the same handler, answer and size trimming as the daemon; a one-question response that does not fit its transport's limit is a crash |
| `metrics` | A request to the metrics listener |
| `plugin_line` | A line a plugin writes to stdout |
| `config` | An `emfor.toml` |

`cargo fuzz` builds with `--cfg fuzzing`. In such a build `emfor-check`
connects to no backend: `net::connect` and `net::open` return a connection
that reads the fuzzer's bytes and takes whatever is written, UDP
exchanges get length-prefixed datagrams from the same bytes, and request
identifiers and nonces are zero, so the fuzzer can write replies that
match them (`crates/emfor-check/src/net.rs`). The plugins run on a
current-thread runtime with a paused clock, so their timeouts end at once.
TLS is not fuzzed, and `mysql` and `pgsql` are left out: `mysql_async`
and `tokio-postgres` parse their protocols and connect on their own. The
daemon's entry points are `fuzz_*` functions that exist only in fuzzing
builds.

A crash leaves its input in `artifacts/<target>/`; run it again with
`cargo +nightly fuzz run <target> artifacts/<target>/<file>`. `corpus/`,
`artifacts/` and `target/` are not committed.
