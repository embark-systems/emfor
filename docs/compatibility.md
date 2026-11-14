# Compatibility policy

This page says what you can rely on when you upgrade emfor: which
interfaces stay the same, which may change, and how a change is announced.
It covers every component of a release: the daemon (`emfor`),
`emfor-check`, `libemfor`, the Rust crates and the Go, Python, Perl and
PHP clients.

## Versions

All components share one version number, `MAJOR.MINOR.PATCH`, and are
released together (see [RELEASING.md](../RELEASING.md)).

- **Patch releases** (0.2.0 → 0.2.1) fix bugs and security problems. They
  never require a change to your config, your scripts, your applications
  or your plugins.
- **Minor releases before 1.0** (0.2 → 0.3) may change an interface listed
  below as stable. Every such change is listed at the top of its
  [changelog](../CHANGELOG.md) entry under **Breaking**, with what to do
  before you upgrade.
- **From 1.0 on**, stable interfaces change only in a major release;
  minor releases only add to them.

## Upgrade the daemon and the clients together

`libemfor` contains the daemon's own LMDB build: the daemon and every
reader on the host must agree on LMDB's lock file format and on the number
of reader slots. Install `libemfor` and the client wrappers from the same
release as the daemon, and upgrade them together. A mismatch shows up as
`UNAVAILABLE` with a message that says so; it never returns wrong data.

The shared library's name carries the minor version (`libemfor.so.0.2`),
so programs linked against one minor release do not silently load another.

## Interfaces

| Interface | Stable | What may change in a compatible release |
| --- | --- | --- |
| Config file (`emfor.toml`) | Yes | New tables, keys and values. |
| Commands, options and exit statuses (`emfor server`, `check-config`, `status`) | Yes | New options and subcommands. |
| `emfor status --json` | Yes | New fields. |
| `emfor status` text output | No | Layout and wording, at any release. Use `--json` or the exit status in scripts. |
| Store format (`meta/schema_version` 1) | Yes | New fields in records, new keys. |
| Client API (lookup call, the four error codes) | Yes | New functions. |
| Error messages of clients and the daemon | No | Wording, at any release. Match on the error code, not the message. |
| Plugin protocol (version 1) | Yes | New optional fields in messages. |
| `emfor-plugin` Rust crate | Yes, from 1.0 | Before 1.0 it follows the minor-release rule above. |
| DNS answers | Yes | Answers for names and query types that got no records before. |
| Metric names, types and labels | Yes | New metrics and new label values. |
| Log lines | No | Wording and level, at any release. Use metrics or `emfor status` for alerting. |
| Rust crates other than `emfor-client` and `emfor-plugin` | No | They are internal to emfor. |

### Config file

The daemon rejects unknown keys, so that a typo is an error rather than
a silently ignored setting. The consequence: a config that uses a key
added in a newer release fails `emfor check-config` on an older one. When
you downgrade, remove the new keys first.

Defaults are part of the stable interface: a release that changes a
default (for example `interval`, `lock_file_mode` or `[store] sync`) lists
it under **Breaking**, with the setting that keeps the old behavior.

### Store format

Records are JSON values under the keys described in
`docs/spectra/specs/state-store/spec.md`. Within format version 1, a
release may add fields and keys; readers ignore what they do not know, and
a `balance` value a client does not know is treated as `round-robin`, a
backend `state` as `unhealthy` and an `on_all_down` as `open`. A
change that older readers would misread raises `meta/schema_version`;
older clients then fail with `INCOMPATIBLE` instead of guessing, and the
changelog says so at the top of the entry.

The store is rebuilt on every daemon start, so there is never a store to
migrate; only the state history of `emfor status --history` is carried
over.

### Client API

The lookup call in each language (`emfor_client::lookup`, `emfor_lookup`,
`emfor.Lookup`, `emfor.lookup`, `Emfor::lookup`, `Emfor\lookup`), its
arguments, the `ip:port` / `[ip]:port` result and the four error codes
`UNAVAILABLE`, `INCOMPATIBLE`, `NOT_FOUND` and `ALL_DOWN` are stable. The
environment variables `EMFOR_STORE`, `EMFOR_MAX_STALENESS` and, for
Python, `EMFOR_LIBRARY` are stable. The conformance cases in
`tests/conformance/v1/` define the expected results; a case is changed
only together with a breaking change.

### Plugin protocol

Version 1 is described in [docs/plugins/protocol-v1.md](plugins/protocol-v1.md).
Within version 1, emfor may add optional fields to the messages it sends.
Plugins must ignore fields they do not know; plugins built with the
`emfor-plugin` crate do. A change a version 1 plugin could not handle gets
a new protocol version, announced in the `hello` handshake, and the
changelog lists it under **Breaking**.

## Platforms

| Platform | Status |
| --- | --- |
| Linux | Supported |
| FreeBSD | Supported; see the known limits in the latest changelog entry for what has been tested |
| macOS | Development only |

## What counts as a bug

When emfor does something other than what its documentation or its
requirements (`docs/spectra/specs/`) say, that is a bug, and a fix that
restores the documented behavior may ship in a patch release even if some
setup relied on the old behavior. The changelog says so when that is
likely to matter.
