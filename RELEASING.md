# Releasing emfor

One release covers the daemon, `emfor-check`, `libemfor` and every client:
they share one version number, and `libemfor` must come from the same
release as the daemon. A release is a tag `vX.Y.Z` with a GitHub Release,
plus a tag `clients/go/vX.Y.Z` for the Go client. Releases ship source only.

The steps below use `X.Y.Z` for the new version.

## 1. Start from a clean main

```sh
git switch main
git pull
git status            # nothing to commit
```

## 2. Set the version

Change the current version to `X.Y.Z` in each of these:

| File | What |
| --- | --- |
| `Cargo.toml` | `version` under `[workspace.package]`; every crate inherits it |
| `clients/python/pyproject.toml` | `version` |
| `clients/perl/lib/Emfor.pm` | `our $VERSION` |
| `clients/php/emfor.c` | `PHP_EMFOR_VERSION` |
| `clients/go/README.md` | the `go get ...@vX.Y.Z` line |

Then run `cargo build` so `Cargo.lock` takes the new version. The test
`clients/rust/tests/versions.rs` fails until the four client files match
`Cargo.toml`.

If the release changes the store format (`meta/schema_version`, see
`docs/spectra/specs/state-store/spec.md`) or the plugin protocol, say so at
the top of the changelog entry: every client and plugin must then be
upgraded with the daemon.

## 3. Write the changelog

Add a section at the top of `CHANGELOG.md`, below the title:

```markdown
## X.Y.Z — YYYY-MM-DD
```

Group the changes as the 0.1.0 entry does (Daemon, emfor-check, Clients,
Deployment), and update **Known limits**: drop what was fixed, add what is
new. Links go to files in the repository by relative path
(`docs/performance.md`); step 6 makes them absolute for the GitHub Release.

## 4. Check

On macOS and in the Debian image `emfor-clients-test`:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
make -C clients test
```

`make -C clients test` needs every client's toolchain; on a Mac without
PHP, run `make -C clients perl` there (it runs C, Go, Python and Perl in
turn) and leave PHP to the Debian run.

For the Debian run, build the image from `tests/clients/Dockerfile` once,
then run the same commands in it. The named volumes keep the cargo
registry and build output between runs:

```sh
docker build -t emfor-clients-test tests/clients
docker run --rm -v "$PWD":/src -w /src \
  -v emfor-clients-cargo:/usr/local/cargo/registry \
  -v emfor-clients-target:/src/target \
  -v emfor-clients-gocache:/root/.cache/go-build \
  emfor-clients-test sh -c '
    rustup component add clippy rustfmt &&
    cargo fmt --check &&
    cargo clippy --workspace --all-targets --all-features -- -D warnings &&
    cargo test --workspace --all-features &&
    make -C clients test'
```

The container installs `libemfor` into `clients/build/prefix`, which macOS
uses too, and its pkg-config file points at `/src`. `make` reinstalls it
on every run, so the checks are not affected; but before building against
`clients/build/prefix` by hand on macOS (step 7), run
`make -C clients/go libemfor` there first.

## 5. Commit, tag and push

```sh
git commit -am "Release X.Y.Z"
git push
git tag -a vX.Y.Z -m "emfor X.Y.Z"
git tag -a clients/go/vX.Y.Z -m "emfor Go client X.Y.Z"
git push origin vX.Y.Z clients/go/vX.Y.Z
```

Never move or delete a pushed tag: the Go module proxy keeps what it first
saw for `clients/go/vX.Y.Z`, so a moved tag gives Go users a different
package than the repository shows. Fix a bad release with a new version.

## 6. Publish the GitHub Release

Take the version's changelog section, make its relative links absolute
for the tag, and publish it:

```sh
awk '/^## X.Y.Z /{f=1;next} /^## /{f=0} f' CHANGELOG.md \
  | sed -E 's#\]\(([^)#:]+)\)#](https://github.com/embark-systems/emfor/blob/vX.Y.Z/\1)#g' \
  > /tmp/emfor-notes.md
grep -n '](' /tmp/emfor-notes.md   # every link starts with https://
gh release create vX.Y.Z --title "emfor X.Y.Z" --notes-file /tmp/emfor-notes.md --verify-tag
```

Open each link in the published Release once.

## 7. Check the Go client

Outside the repository, with `libemfor` X.Y.Z installed:

```sh
mkdir /tmp/emfor-go && cd /tmp/emfor-go
go mod init example.com/check
go get github.com/embark-systems/emfor/clients/go@vX.Y.Z
```

`go get` must download `vX.Y.Z`; a program importing the package must
build with `pkg-config emfor` finding the installed `libemfor`.
