## What and why

<!-- What does this change, and what problem does it solve? Link the issue. -->

## Checked

<!-- Which of these did you run, and where (macOS, Linux, the Docker image)? -->

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace --all-features`
- [ ] `make -C clients test`
- [ ] New tests fail without the change

## Also

- [ ] Docs, `examples/emfor.toml` and the requirements in `docs/spectra/specs/` updated where behavior changed
- [ ] A line under `## Unreleased` in `CHANGELOG.md`
- [ ] Breaking changes (see `docs/compatibility.md`) are called out above
- [ ] Every commit is signed off (`git commit -s`, see `CONTRIBUTING.md`)
