## MODIFIED Requirements

### Requirement: Configuration audit log

After each accepted reload the daemon SHALL log, at `info` level, one line for each difference between the running config and the new one, before the line saying the config was reloaded, in this order: services by name; within a service, the service-level lines, then its backend lines by backend id; then the peer group lines. Each line SHALL carry the field `change` and the fields listed for that change; the field names and the values of `change` are stable, the wording of the line is not. The changes are: `service_added` and `service_removed` (`service`, `backends`); `backend_added` and `backend_removed` (`service`, `backend`); `backend_changed` (`service`, `backend`, `field` one of `weight`, `site`, `backup`, `drain`, and `from`, `to`); `service_changed` (`service`, `field` one of `check`, `interval`, `timeout`, `rise`, `fall`, `jitter`, `on_all_down`, `balance`, `peers`, `slow_start`, `dns_ttl`, and `from`, `to`; for `check` the values are the check names only); `check_definition_changed` (`service`, `digest_from`, `digest_to`, and `resets_state`, `true` when the checker or its `check_options` differ, which restarts the state of the service's backends from `unknown`, `false` when only the digest differs, as for `timeout`); `peer_member_added` and `peer_member_removed` (`peer`); `peers_changed` (`field` one of `encrypt`, `accept_plaintext`, `cipher`, `secret_file`, `previous_secret_file`, and `from`, `to`, and `applies`, `now`, or `next_start` for `previous_secret_file`, which is read only at start; a `cipher` that is not set is written as `chacha20-poly1305`). A `site` that is not set SHALL be written as `-`. The daemon SHALL log at most 50 such lines per reload; when there are more differences it SHALL replace the rest by one line with `change=more` and `omitted` set to their number. No line SHALL contain a `check_options` value, the content of a secret file, or a hash of it. A reload with no difference SHALL log no such line.

#### Scenario: Backend added and another drained

- **WHEN** a reload adds backend `10.0.0.13:8080` to service `api.svc.emfor` and sets `drain = true` on `10.0.0.11:8080`
- **THEN** the log has a line with `change=backend_added`, `service=api.svc.emfor`, `backend=10.0.0.13:8080`, and a line with `change=backend_changed`, `field=drain`, `from=false`, `to=true` for `10.0.0.11:8080`, both before the `config reloaded` line

#### Scenario: Check options changed without exposing them

- **WHEN** a reload changes `check_options` of a service, for example `expect = "ready"` to `expect = "ok"`
- **THEN** the log has one `change=check_definition_changed` line with `digest_from`, `digest_to` and `resets_state=true`, and no log line contains `ready` or `ok` as an option value

#### Scenario: Timeout change keeps the state

- **WHEN** a reload changes only a service's `timeout`
- **THEN** the log has a `change=service_changed` line with `field=timeout` and a `change=check_definition_changed` line with `resets_state=false`

#### Scenario: Peer group changes

- **WHEN** a reload adds a member, sets `encrypt = true`, and changes `previous_secret_file`
- **THEN** the log has `peer_member_added`, a `peers_changed` line with `field=encrypt` and `applies=now`, and a `peers_changed` line with `field=previous_secret_file` and `applies=next_start`, and no line holds the content of a secret file or a hash of it

#### Scenario: More than fifty differences

- **WHEN** a reload adds 80 services
- **THEN** the log has 50 `service_added` lines and one line with `change=more` and `omitted=30`

##### Example: change values

| Reload changes | Lines with `change` |
| --- | --- |
| nothing | none |
| service `a` removed | `service_removed` |
| backend `weight` 1 to 3 | `backend_changed` with `field=weight` |
| `[defaults] interval` 2s to 5s, three services use it | three `service_changed` lines with `field=interval` |
| `check = "tcp"` to `check = "http"` | `service_changed` with `field=check`, `check_definition_changed` with `resets_state=true` |
| `[peers] members` loses one address | `peer_member_removed` |

#### Scenario: Cipher changed

- **WHEN** a reload sets `cipher = "aes-256-gcm"` in `[peers]` where it was not set
- **THEN** the log has a `peers_changed` line with `field=cipher`, `from=chacha20-poly1305`, `to=aes-256-gcm` and `applies=now`

#### Scenario: No difference

- **WHEN** a reload applies a file with the same effective config
- **THEN** the log has the `config reloaded` line and no `change=` line
