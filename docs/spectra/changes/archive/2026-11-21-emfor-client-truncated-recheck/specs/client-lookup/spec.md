## MODIFIED Requirements

### Requirement: Lookup failures use four shared error codes

A lookup that cannot return an address SHALL fail with exactly one of these codes, decided by the first step that fails, in this order:

1. `UNAVAILABLE` — the store file cannot be opened (missing, permission denied, LMDB lock or data format different from the client's, a data file shorter than the last page its LMDB meta page records, any other open error), the key `meta/schema_version` is absent, or `EMFOR_MAX_STALENESS` holds an invalid value.
2. `INCOMPATIBLE` — `meta/schema_version` is neither `1` nor `2`, `meta/heartbeat` is missing or not a decimal integer, or the service record cannot be parsed as a version 2 record. A `balance`, backend `state` or `on_all_down` string the client does not know SHALL NOT count as a parse failure (see "Unknown balance falls back to round-robin" and "Candidates follow health and on_all_down").
3. `NOT_FOUND` — no `svc/<name>` key exists for the normalized name.
4. `ALL_DOWN` — no backend is usable and the service's `on_all_down` is `closed`.

Each failure SHALL carry a human-readable message naming the cause. Each language SHALL expose the code in its own idiom (a status value in the C library, an error value in Rust) with the code's name available as the exact strings above. A store that LMDB reports as damaged, and a data file that is already shorter than the last page its LMDB meta page records when the client opens it, SHALL NOT make the lookup crash, panic or abort. When the client opens the store it SHALL compare the length of the file it opened, measured after reading the meta page, with (last page number + 1) × page size as recorded by that meta page, before any read of a data page. A shorter file can be one that the daemon has committed and not yet padded to its recorded length, so the client SHALL read the meta page and measure the length again after a short pause, up to 5 times with 20 milliseconds between them, and SHALL open the store when a check finds the file long enough; it SHALL fail a file that is still shorter after the last check with `UNAVAILABLE` and a message containing the store path, `truncated`, the file length and the recorded length; such a failure counts as a store file that exists but cannot be opened. Any other open error SHALL fail at once, without these checks. A data file changed in place (overwritten or truncated) after the client opened it, and a disk read error, are outside this guarantee: reading it through the memory map can end the process with `SIGBUS`.

#### Scenario: Error code by store condition

- **WHEN** a lookup runs against each store condition below
- **THEN** it fails with the listed code

##### Example: store conditions

| Store condition | Code |
| --- | --- |
| store file does not exist | `UNAVAILABLE` |
| store exists, `meta/schema_version` absent | `UNAVAILABLE` |
| `EMFOR_MAX_STALENESS=fast` | `UNAVAILABLE` |
| `meta/schema_version` = `3` | `INCOMPATIBLE` |
| `meta/heartbeat` absent | `INCOMPATIBLE` |
| `svc/api.svc.emfor` = `{not json` | `INCOMPATIBLE` |
| `svc/api.svc.emfor` has `"balance":5` (not a string) | `INCOMPATIBLE` |
| `svc/api.svc.emfor` has `"balance":"some-future-method"` | no error: chosen as `round-robin` |
| no `svc/nope.svc.emfor` key, lookup `nope.svc.emfor` | `NOT_FOUND` |
| every backend `unhealthy`, `on_all_down` = `closed` | `ALL_DOWN` |

#### Scenario: Schema check wins over a missing service

- **WHEN** `meta/schema_version` is `3` and the looked-up name has no record
- **THEN** the lookup fails with `INCOMPATIBLE`, not `NOT_FOUND`

#### Scenario: Missing store is retried

- **WHEN** a lookup fails with `UNAVAILABLE` because the store file does not exist, and the daemon then creates the store
- **THEN** the next lookup in the same process opens the store and returns an address

#### Scenario: Unopenable store is retried after a pause

- **WHEN** a lookup fails with `UNAVAILABLE` because the store file exists but cannot be opened, and the file is then replaced by a valid store
- **THEN** lookups in the same process within 1 second of the failed open fail with the same error without opening the file, and the first lookup after that opens the store and returns an address

#### Scenario: Truncated store fails instead of crashing

- **WHEN** a store with a record of `api.svc.emfor` is cut short by its last page (the length becomes the recorded length minus one page) before any client opens it, and it stays that short, and a client looks up `api.svc.emfor`
- **THEN** the lookup fails with `UNAVAILABLE`, its message contains the store path and `truncated`, and the process goes on running

#### Scenario: Store padded while the client opens it

- **WHEN** a client opens a store whose file is one page shorter than its meta page records, and the daemon pads the file to the recorded length 10 milliseconds later, and the client looks up `api.svc.emfor`
- **THEN** the lookup returns an address, and a lookup right after it does not fail with a cached `UNAVAILABLE`

##### Example: length check

| File length | Last page number | Page size | Result |
| --- | --- | --- | --- |
| 16384 | 3 | 4096 | opened |
| 12288 | 3 | 4096 | `UNAVAILABLE`, `truncated: 12288 bytes, LMDB records 16384 bytes` (after the checks) |
