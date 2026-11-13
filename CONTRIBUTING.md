# Contributing to emfor

Thank you for helping. This page explains how changes are made, what a pull
request needs, and how contributions are licensed.

## Before you start

- **Bugs:** open an issue with the bug report form. Security problems go
  through [SECURITY.md](SECURITY.md), never a public issue.
- **Small fixes** (a typo, a clear bug with an obvious fix, a missing test):
  send a pull request directly.
- **Anything that changes behavior** (a new config key, a new check, a
  change to what a lookup or a DNS answer returns, the store format, the
  plugin protocol): open an issue first and describe the problem you want
  to solve. emfor keeps its behavior written down as requirements, and a
  change in behavior starts there (see [Requirements](#requirements)).

## Development setup

You need Rust (edition 2024, so Rust 1.85 or later). For the clients you
also need [cargo-c](https://github.com/lu-zero/cargo-c), pkg-config, Go,
Python 3, Perl with its headers, and PHP with its development files. The
Docker image in [tests/clients/Dockerfile](tests/clients/Dockerfile) has
all of them:

```sh
docker build -t emfor-clients-test tests/clients
```

Linux and FreeBSD are the deployment platforms; macOS works for
development.

## Checks a pull request must pass

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
make -C clients test
```

`make -C clients test` builds `libemfor` and runs the C, Go, Python, Perl
and PHP clients against the shared conformance cases in
`tests/conformance/v1/`. To run it in the Docker image:

```sh
docker run --rm -v "$PWD":/src -w /src emfor-clients-test \
  sh -c 'cargo test --workspace --all-features && make -C clients test'
```

Say in the pull request which of these you ran and where (for example
"macOS: all four; Linux image: tests and clients"). Changes to the DNS
resolver recipes, the plugins or the rc.d script have their own scripts
under `tests/resolver/` and `tests/plugins/`; run the ones your change
touches, or say that you could not.

## What we look for in a change

- **Tests that fail without the change.** A test for a bug fix must fail
  on the code before the fix. Please check this, and say so.
- **No `unwrap()` in production code.** Handle the error, or use
  `expect()` with a message that states the full invariant ("the store was
  opened above, so the environment is set"), not just what failed.
- **Bounded input.** Anything from the network, a backend or a plugin is
  untrusted: limit its size and escape it before it reaches logs, the store
  or a terminal.
- **Compatibility.** Read [docs/compatibility.md](docs/compatibility.md)
  before changing the config, the command line, the store, the client API,
  the plugin protocol, DNS answers or metric names.
- **Documentation in the same pull request.** Update the README, the
  documents under `docs/`, `examples/emfor.toml` and the client READMEs
  where the change shows. Documentation is written in English; the
  Traditional Chinese translations (`README.zh-TW.md`,
  `docs/manual.zh_TW/`) may follow in a separate pull request.
- **A changelog line** under `## Unreleased` in
  [CHANGELOG.md](CHANGELOG.md), written for the people who run emfor:
  what changed for them and what they need to do.

## Requirements

Behavior is specified in `docs/spectra/specs/<capability>/spec.md`, one
file per capability (health checking, backend selection, client lookup,
DNS server, and so on). Each requirement has scenarios, and the tests
follow them. Planned changes live in `docs/spectra/changes/`, and finished
ones in `docs/spectra/changes/archive/`.

For a change in behavior, the pull request updates the affected
requirement and its scenarios together with the code. The maintainers
plan changes with the Spectra tool (`spectra new change`); you do not need
it, and editing the spec file by hand is fine. The specs are written in
English.

## Commits and pull requests

- One logical change per pull request. Keep refactoring separate from
  behavior changes.
- Commit subjects are short and in the imperative ("Free stale reader
  slots after a failed commit"); the body says why, and what was checked.
- Every commit carries a `Signed-off-by` line (see below).

## Developer Certificate of Origin

emfor uses the [Developer Certificate of Origin](https://developercertificate.org/)
(DCO) 1.1 instead of a contributor license agreement. By adding a
`Signed-off-by` line to a commit, you certify what the DCO states: in
short, that you wrote the change or otherwise have the right to submit it
under the project's license, and that you understand the contribution and
your sign-off are public and kept with the project.

Sign off with `git commit -s`, which adds:

```
Signed-off-by: Your Name <you@example.org>
```

Use your real name and an address you can be reached at. To sign off
commits you already made on a branch:

```sh
git rebase --signoff main
```

Pull requests with commits that are not signed off cannot be merged.

## License

emfor is licensed under the [BSD 2-Clause License](LICENSE). Contributions
are accepted under the same license.

## Conduct

Everyone taking part in the project is expected to follow the
[code of conduct](CODE_OF_CONDUCT.md).
