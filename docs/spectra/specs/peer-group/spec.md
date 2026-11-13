# peer-group Specification

## Purpose

A peer group is the set of emfor daemons in one `site` that list each other in `[peers]`. Members authenticate every message, track which members are live, and notify each other when a backend fails so that the whole site rechecks it at once; each daemon still decides every backend's state from its own checks.

## Requirements

### Requirement: Peer group configuration

The config SHALL accept an optional `[peers]` table with exactly the keys `listen`, `members` and `secret_file`, all required. `listen` SHALL be one `IPv4:port` or `[IPv6]:port` string with port 1–65535 whose address is not the unspecified address (`0.0.0.0` or `::`). `members` SHALL be an array of 2 to 64 such strings with no repeats, all of the address family (IPv4 or IPv6) of `listen`, and SHALL contain `listen`. `secret_file` SHALL be an absolute path to a file that the daemon can read, whose content after removing one trailing line feed is at least 32 bytes, and whose permissions do not allow other users to read it. When `[peers]` is present the top-level `site` SHALL be set. Every violation SHALL be reported together with the other config errors, prefixed with its TOML path. Without a `[peers]` table the daemon SHALL open no peer socket and send no peer message.

#### Scenario: Valid peer group

- **WHEN** the config has `site = "dc1"` and `[peers]` with `listen = "127.0.0.1:5302"`, `members = ["127.0.0.1:5302", "127.0.0.1:5303"]` and a `secret_file` of 64 bytes with mode 0600
- **THEN** `emfor check-config` exits 0

#### Scenario: No peers table

- **WHEN** the config has no `[peers]` table
- **THEN** the daemon binds no peer socket and `/metrics` contains no series whose name starts with `emfor_peer_`

##### Example: peer config validation

| Config | Result |
| --- | --- |
| `[peers]` without top-level `site` | error at `site`: site is required when [peers] is set |
| `listen = "0.0.0.0:5302"` | error at `peers.listen`: listen must be a specific address |
| `listen = "localhost:5302"` | error at `peers.listen`: listen address must be an IP literal with port |
| `listen = "10.0.0.9:5302"`, not in `members` | error at `peers.listen`: listen must be one of members |
| `members = ["10.0.0.5:5302"]` | error at `peers.members`: members must list 2 to 64 addresses |
| `members` with `"10.0.0.6:5302"` twice | error at `peers.members[<i>]` of the second one: duplicate member |
| `listen = "10.0.0.5:5302"`, `members = ["10.0.0.5:5302", "[2001:db8::6]:5302"]` | error at `peers.members[1]`: member must be of the same address family (IPv4 or IPv6) as listen |
| `secret_file = "peers.key"` | error at `peers.secret_file`: must be an absolute path |
| `secret_file` that does not exist | error at `peers.secret_file`: cannot read |
| `secret_file` with 31 bytes plus a line feed | error at `peers.secret_file`: secret must be at least 32 bytes |
| `secret_file` with mode 0644 | error at `peers.secret_file`: must not be readable by other users |
| `[peers] port = 5302` | error: unknown key |


<!-- @trace
source: emfor-peers
updated: 2026-11-13
code:
  - docs/manual/01-concepts.md
  - crates/emfor-daemon/Cargo.toml
  - crates/emfor-daemon/src/lib.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/peers.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/schedule.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/live.rs
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/metrics.rs
  - examples/emfor.toml
  - crates/emfor-daemon/src/config.rs
  - CHANGELOG.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Peer message authentication and replay protection

Every peer message SHALL be one UDP datagram of at most 1200 bytes: the four bytes `EMP1`, then a 32-byte HMAC-SHA256 tag computed with the secret over `EMP1` followed by the payload, then the payload, a UTF-8 JSON object with the fields `from` (the sender's `listen`), `site`, `boot` (16 lowercase hex digits chosen at random when the sender starts), `seq` (1 for the sender's first message after it starts, increased by 1 for each message), `ts` (Unix milliseconds when sent) and `kind`. A receiver SHALL accept a message only when all of these hold, and SHALL otherwise drop it and count it under the first failed reason in this order: the datagram is 36 to 1200 bytes, starts with `EMP1` and carries a JSON object with all common fields (`malformed`); the tag matches the current secret or the secret it replaced less than 120 seconds ago (`auth`); `from` is a member other than the receiver and equals the datagram's source address (`sender`); `site` equals the receiver's `site` (`site`); `ts` is within 5 seconds of the receiver's clock (`stale`); and, against the last accepted message from that sender, `seq` is greater when `boot` is the same, or `ts` is greater when `boot` differs (`replay`). Unknown JSON fields SHALL be ignored. An accepted message whose `kind` the receiver does not know SHALL count only for liveness. A dropped message SHALL be logged at `warn` level at most once per 60 seconds for each source address and reason.

#### Scenario: Tampered message

- **WHEN** a member receives a `heartbeat` whose payload was changed after the tag was computed
- **THEN** the message is dropped and `emfor_peer_messages_rejected_total{reason="auth"}` increases by 1

#### Scenario: Sender restarts

- **WHEN** a member has accepted messages up to `seq` 900 with `boot` `aaaaaaaaaaaaaaaa`, and the sender restarts and sends `seq` 1 with `boot` `bbbbbbbbbbbbbbbb` and a later `ts`
- **THEN** the message is accepted

##### Example: acceptance checks

| Situation | Result |
| --- | --- |
| valid heartbeat from member 10.0.0.6:5302 | accepted |
| datagram of 20 bytes | rejected: malformed |
| tag computed with a different secret | rejected: auth |
| tag computed with the previous secret, replaced 30 seconds ago | accepted |
| tag computed with the previous secret, replaced 90 seconds ago | accepted |
| tag computed with the previous secret, replaced 130 seconds ago | rejected: auth |
| `from` = 10.0.0.6:5302 but sent from 10.0.0.8:5302 | rejected: sender |
| `from` not in `members` | rejected: sender |
| `site` = `dc2` at a receiver in `dc1` | rejected: site |
| `ts` 6 seconds behind the receiver's clock | rejected: stale |
| same `boot`, `seq` equal to the last accepted | rejected: replay |
| `kind` = `"result"` (unknown to this version), otherwise valid | accepted, used only for liveness |


<!-- @trace
source: emfor-peers
updated: 2026-11-13
code:
  - docs/manual/01-concepts.md
  - crates/emfor-daemon/Cargo.toml
  - crates/emfor-daemon/src/lib.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/peers.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/schedule.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/live.rs
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/metrics.rs
  - examples/emfor.toml
  - crates/emfor-daemon/src/config.rs
  - CHANGELOG.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Peer secret reload

The daemon SHALL check `secret_file` before sending each round of heartbeats. When its content changed and is valid, the new content SHALL become the current secret. For 60 seconds after the change the daemon SHALL still sign its messages with the secret it replaced, and SHALL still accept messages signed with the replaced secret for 120 seconds after the change, so that when every member's file changes within 60 seconds of the others no message between any two members is dropped. When the file cannot be read or its content is not valid, the daemon SHALL keep the last valid secret and log at `warn` level at most once per minute.

#### Scenario: Secret rotated on one host first

- **WHEN** host A's `secret_file` is changed to a new valid secret, and host B's file is changed to the same secret 40 seconds later
- **THEN** every message A sends to B and B sends to A is accepted, before, during and after the change, and A signs with the new secret from 60 seconds after its own change


<!-- @trace
source: emfor-peers
updated: 2026-11-13
code:
  - docs/manual/01-concepts.md
  - crates/emfor-daemon/Cargo.toml
  - crates/emfor-daemon/src/lib.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/peers.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/schedule.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/live.rs
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/metrics.rs
  - examples/emfor.toml
  - crates/emfor-daemon/src/config.rs
  - CHANGELOG.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Peer heartbeats and liveness

A daemon with `[peers]` SHALL send a `heartbeat` message every second to every member other than itself. A member SHALL be live while the daemon has accepted at least one message from it within the last 3 seconds. When a member becomes live or stops being live the daemon SHALL log one line at `info` level naming the member.

#### Scenario: Member stops

- **WHEN** two daemons form a peer group and one of them stops
- **THEN** within 4 seconds the other logs that the stopped member is not live and its `emfor_peer_up` series for that member is 0


<!-- @trace
source: emfor-peers
updated: 2026-11-13
code:
  - docs/manual/01-concepts.md
  - crates/emfor-daemon/Cargo.toml
  - crates/emfor-daemon/src/lib.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/peers.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/schedule.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/live.rs
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/metrics.rs
  - examples/emfor.toml
  - crates/emfor-daemon/src/config.rs
  - CHANGELOG.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Check definition digest

Every service SHALL have a check definition digest: the first 16 bytes, as 32 lowercase hex digits, of the SHA-256 of the JSON object `{"checker":<checker identity>,"options":<check_options>,"timeout_ms":<timeout in milliseconds>}` written with every object's keys in lexicographic order and no whitespace. The checker identity SHALL be the check name for a built-in checker and `plugin:<name>:<command as JSON>` for a plugin. Two configs whose service differs only in the order of keys, in `interval`, `rise`, `fall`, `jitter`, `balance`, `on_all_down`, `dns_ttl` or `backends` SHALL give that service the same digest.

#### Scenario: Key order does not matter

- **WHEN** host A writes a service's `check_options` as `{ path = "/health", expect_status = ["200"] }` and host B writes `{ expect_status = ["200"], path = "/health" }`, with the same `check` and `timeout`
- **THEN** both hosts compute the same digest for that service

##### Example: what changes the digest

| Difference between two configs of one service | Same digest |
| --- | --- |
| `interval = "2s"` vs `"5s"` | yes |
| `rise = 2` vs `3` | yes |
| `backends` with one more entry | yes |
| `timeout = "1s"` vs `"500ms"` | no |
| `check = "tcp"` vs `"http"` | no |
| `check_options.path = "/health"` vs `"/ready"` | no |


<!-- @trace
source: emfor-peers
updated: 2026-11-13
code:
  - docs/manual/01-concepts.md
  - crates/emfor-daemon/Cargo.toml
  - crates/emfor-daemon/src/lib.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/peers.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/schedule.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/live.rs
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/metrics.rs
  - examples/emfor.toml
  - crates/emfor-daemon/src/config.rs
  - CHANGELOG.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Backend failure notification

When a backend's own check moves it from `healthy` to `unhealthy`, the daemon SHALL send one `down` message to every member other than itself, with the fields `service`, `digest` (the service's check definition digest), `backend` (the backend id) and `reason` (the transition's reason, cut at a character boundary to at most 256 bytes of UTF-8). A move from `unknown` to `unhealthy` SHALL NOT send a message. A message SHALL NOT be sent again.

When a daemon accepts a `down` message for a service it has with the same digest and a backend of that service, it SHALL start a fast recheck run for that backend when all of these hold: the backend is `healthy` in this daemon, no fast recheck run is in progress for it, and its last fast recheck run ended more than `fall × interval` ago. Otherwise it SHALL ignore the message. A received message SHALL NOT change any backend's state by itself; only this daemon's own checks change it. Starting a fast recheck run SHALL be logged at `info` level as `peer <member> reports backend <backend> of <service> down (<reason>); checking now`.

#### Scenario: Peer sees the failure first

- **WHEN** daemons A and B in one peer group both check a TCP backend of `api.svc.emfor` with `fall = 3`, A with `interval = "1s"` and B with `interval = "30s"` (the digest leaves `interval` out), both see it `healthy`, and the backend stops listening
- **THEN** B, which on its own would need at least a minute, marks it `unhealthy` within 2 seconds after A did

#### Scenario: Digest differs

- **WHEN** A and B check the same backend but B's service has `timeout = "500ms"` and A's has `timeout = "1s"`, and A sends a `down` message for it
- **THEN** B ignores the message and checks the backend on its normal schedule

#### Scenario: Startup failures are not announced

- **WHEN** a daemon starts and a backend goes from `unknown` to `unhealthy`
- **THEN** the daemon sends no `down` message for it

##### Example: whether a received down message starts a fast recheck run

| Local state of the backend | Fast run in progress | Last fast run ended | Result |
| --- | --- | --- | --- |
| healthy | no | never | start a fast run |
| healthy | no | `fall × interval` + 1s ago | start a fast run |
| healthy | no | 1s ago (`fall × interval` = 6s) | ignore |
| healthy | yes | — | ignore |
| unhealthy | no | never | ignore |
| unknown | no | never | ignore |


<!-- @trace
source: emfor-peers
updated: 2026-11-13
code:
  - docs/manual/01-concepts.md
  - crates/emfor-daemon/Cargo.toml
  - crates/emfor-daemon/src/lib.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/peers.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/schedule.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/live.rs
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/metrics.rs
  - examples/emfor.toml
  - crates/emfor-daemon/src/config.rs
  - CHANGELOG.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Peer listen requires restart

A change to `[peers] listen`, including adding or removing the `[peers]` table, SHALL be a validation error on reload with the message "peers listen settings require restart". Changes to `[peers] members` and `[peers] secret_file` SHALL take effect on reload: a removed member's replay state and metrics series SHALL be dropped, and an added member SHALL start as not live. Binding `listen` at start SHALL be a startup error when it fails.

#### Scenario: Member added on reload

- **WHEN** a daemon runs with `members = ["127.0.0.1:5302", "127.0.0.1:5303"]`, `127.0.0.1:5304` is added to `members` and the daemon receives `SIGHUP`
- **THEN** the reload succeeds, `emfor_peer_up{peer="127.0.0.1:5304"}` appears with value 0, and the daemon sends heartbeats to 127.0.0.1:5304

#### Scenario: Listen change rejected

- **WHEN** `[peers] listen` is changed from `"127.0.0.1:5302"` to `"127.0.0.1:5305"` and the daemon receives `SIGHUP`
- **THEN** the reload is rejected with the error "peers listen settings require restart" and the daemon keeps receiving on 127.0.0.1:5302

<!-- @trace
source: emfor-peers
updated: 2026-11-13
code:
  - docs/manual/01-concepts.md
  - crates/emfor-daemon/Cargo.toml
  - crates/emfor-daemon/src/lib.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/peers.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/schedule.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/live.rs
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/metrics.rs
  - examples/emfor.toml
  - crates/emfor-daemon/src/config.rs
  - CHANGELOG.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->