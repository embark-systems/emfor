## MODIFIED Requirements

### Requirement: Check definition digest

Every service SHALL have a check definition digest: the first 16 bytes, as 32 lowercase hex digits, of the SHA-256 of the JSON object `{"checker":<checker identity>,"options":<check_options>,"timeout_ms":<timeout in milliseconds>}` written with every object's keys in lexicographic order and no whitespace. The checker identity SHALL be the check name for a built-in checker, `plugin:<name>:<command as JSON>` for a plugin without `env_pass`, and `plugin:<name>:<command as JSON>:<env_pass as JSON>` for a plugin whose `env_pass` is not empty. For an `exec` check, `env_pass` is a key of `check_options` and so is part of the digest like every other option. Two configs whose service differs only in the order of keys, in `interval`, `rise`, `fall`, `jitter`, `balance`, `on_all_down`, `dns_ttl` or `backends` SHALL give that service the same digest.

#### Scenario: Key order does not matter

- **WHEN** host A writes a service's `check_options` as `{ path = "/health", expect_status = ["200"] }` and host B writes `{ expect_status = ["200"], path = "/health" }`, with the same `check` and `timeout`
- **THEN** both hosts compute the same digest for that service

#### Scenario: Plugin env_pass changes the digest

- **WHEN** host A declares `[plugins."redis"]` with `env_pass = ["http_proxy"]` and host B declares it with the same `command` and no `env_pass`, and a service uses that plugin on both
- **THEN** the two hosts compute different digests for that service, and a plugin without `env_pass` has the identity it had before `env_pass` existed

##### Example: what changes the digest

| Difference between two configs of one service | Same digest |
| --- | --- |
| `interval = "2s"` vs `"5s"` | yes |
| `rise = 2` vs `3` | yes |
| `backends` with one more entry | yes |
| `timeout = "1s"` vs `"500ms"` | no |
| `check = "tcp"` vs `"http"` | no |
| `check_options.path = "/health"` vs `"/ready"` | no |
| `exec` `check_options.env_pass = ["HOME"]` vs no `env_pass` | no |
| plugin `env_pass = ["http_proxy"]` vs none | no |
