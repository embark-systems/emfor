# emfor client conformance data, version 1

Every client interface (the Rust API, the C API, and each language wrapper)
runs every case and vector here and must pass all of them. The contract is the
`client-lookup` spec; the format is described in design "Shared conformance
data" of the emfor-clients change.

- `cases.json`: a list of cases. Each has `case` (a name), `store` (the store
  contents, or `null` for a store file that does not exist),
  `max_staleness_ms`, optionally `max_staleness` (a raw `EMFOR_MAX_STALENESS`
  value that the runner passes through the client's own parser instead of
  `max_staleness_ms`), and `lookups`. Each lookup has `name`, an optional
  `affinity_key`, and exactly one of `expect` (an `ip:port`), `expect_error`
  (`UNAVAILABLE`, `INCOMPATIBLE`, `NOT_FOUND` or `ALL_DOWN`),
  `expect_all_of` (call the lookup that many times in a row; the results, as
  a set, equal the list) or `expect_counts` (an object `{address: count}`;
  call the lookup the sum of the counts in a row; each address appears
  exactly its count and nothing else appears, errors included).
- `store.heartbeat` is `fresh`, `stale` (now minus twice the maximum
  staleness), `zero` or `missing`; `store.raw` holds keys written verbatim,
  such as `meta/site` for `locality` or malformed records.
- `sticky-v1.json`: rendezvous hash vectors (`hashes`: key, id, big-endian
  u32 of the first 4 bytes of MD5(key ++ id)) and pick vectors (`picks`).
  Generated with Python's hashlib.

`emfor-fixture <cases.json> <index> <out.mdb>` (tools/emfor-fixture) writes a
case's store to an LMDB file with the daemon's LMDB.

## Running a client through the shared driver

The C library and the Go, Python, Perl and PHP wrappers are checked by
`emfor-conformance` (tools/emfor-fixture), which needs no code in the client
beyond a small lookup program:

```
emfor-conformance [--cases <cases.json>] [--vectors <sticky-v1.json>] [--timeout-ms <ms>] -- <program...>
```

For each case it writes the store, then runs the program once per lookup:

| Item | Value |
| --- | --- |
| Invocation | `<program...> <name> <affinity-key> <count>`; the key is `""` for no key |
| Output | `count` lines, one per lookup in the same process: `OK <address>` or `ERR <CODE> <message>` |
| Environment | `EMFOR_STORE` = the case's store; `EMFOR_MAX_STALENESS` = the case's `max_staleness`, or `<max_staleness_ms>ms` |
| Exit status | `0`; any other status, or running past the time limit (default 10 s), fails every lookup of that run |

`count` is the length of `expect_all_of`, the sum of `expect_counts`, or 1. Each pick vector becomes a
sticky service `pick-<i>.svc.emfor` in one extra store and is looked up with
its key. The hash vectors are checked only by the Rust runner, since no other
interface exposes the hash. The driver prints a summary and exits 1 if any
lookup fails. clients/libemfor.mk defines `$(EMFOR_CONFORMANCE)` for it.
