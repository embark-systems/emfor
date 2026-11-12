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
  (`UNAVAILABLE`, `INCOMPATIBLE`, `NOT_FOUND` or `ALL_DOWN`) or
  `expect_all_of` (call the lookup that many times in a row; the results, as
  a set, equal the list).
- `store.heartbeat` is `fresh`, `stale` (now minus twice the maximum
  staleness), `zero` or `missing`; `store.raw` holds keys written verbatim.
- `sticky-v1.json`: rendezvous hash vectors (`hashes`: key, id, big-endian
  u32 of the first 4 bytes of MD5(key ++ id)) and pick vectors (`picks`).
  Generated with Python's hashlib.

`emfor-fixture <cases.json> <index> <out.mdb>` (tools/emfor-fixture) writes a
case's store to an LMDB file with the daemon's LMDB.
