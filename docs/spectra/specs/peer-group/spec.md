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

Every peer message SHALL be one UDP datagram of at most 1200 bytes: the four bytes `EMP1`, then a 32-byte HMAC-SHA256 tag computed with the secret over `EMP1` followed by the payload, then the payload, a UTF-8 JSON object with the fields `from` (the sender's `listen`), `site`, `boot` (16 lowercase hex digits chosen at random when the sender starts), `seq` (1 for the sender's first message after it starts, increased by 1 for each message), `ts` (Unix milliseconds when sent) and `kind`. A receiver SHALL accept a message only when all of these hold, and SHALL otherwise drop it and count it under the first failed reason in this order: the datagram is 36 to 1200 bytes, starts with `EMP1` and carries a JSON object with all common fields, and, for a `kind` the receiver knows, all fields of that kind with valid values (`malformed`); the tag matches the current secret or the secret it replaced less than 120 seconds ago (`auth`); `from` is a member other than the receiver and equals the datagram's source address (`sender`); `site` equals the receiver's `site` (`site`); `ts` is within 5 seconds of the receiver's clock (`stale`); and, against the last accepted message from that sender, `seq` is greater when `boot` is the same, or `ts` is greater when `boot` differs (`replay`). The known kinds SHALL be `heartbeat`, `down` and `result`. Unknown JSON fields SHALL be ignored. An accepted message whose `kind` the receiver does not know SHALL count only for liveness. A dropped message SHALL be logged at `warn` level at most once per 60 seconds for each source address and reason.

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
| `kind` = `"result"` with `outcome` = `"pending"` | rejected: malformed |
| tag computed with a different secret | rejected: auth |
| tag computed with the previous secret, replaced 30 seconds ago | accepted |
| tag computed with the previous secret, replaced 90 seconds ago | accepted |
| tag computed with the previous secret, replaced 130 seconds ago | rejected: auth |
| `from` = 10.0.0.6:5302 but sent from 10.0.0.8:5302 | rejected: sender |
| `from` not in `members` | rejected: sender |
| `site` = `dc2` at a receiver in `dc1` | rejected: site |
| `ts` 6 seconds behind the receiver's clock | rejected: stale |
| same `boot`, `seq` equal to the last accepted | rejected: replay |
| `kind` = `"assign"` (unknown to this version), otherwise valid | accepted, used only for liveness |


<!-- @trace
source: emfor-peers-quorum
updated: 2026-11-13
code:
  - crates/emfor-daemon/Cargo.toml
  - docs/manual/03-configuration.md
  - CHANGELOG.md
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/metrics.rs
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/peers.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/schedule.rs
  - crates/emfor-daemon/src/quorum.rs
  - crates/emfor-daemon/src/writer.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-format/src/lib.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/supervisor.rs
  - docs/manual/01-concepts.md
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

The service tag of a service SHALL be the first 4 bytes of SHA-256 over the UTF-8 bytes of the service name, written as 8 lowercase hex digits. When this daemon has services whose effective `peers` setting is `false` (see "Services left out of the peer group"), each heartbeat SHALL carry the field `off`: an array of the distinct service tags of those services in byte order, at most 80 of them; with more than 80 distinct tags only the first 80 in that order SHALL be sent, and the daemon SHALL log one line at `warn` level, at start and after each reload that changes those services, giving the number of tags not sent. Without such services the heartbeat SHALL NOT carry `off`. A `heartbeat` whose `off` is present and is not an array of strings of 8 lowercase hex digits each SHALL be dropped as `malformed`.

For each member the daemon SHALL keep the service tags of the latest heartbeat it accepted from that member, none when that heartbeat has no `off`, and SHALL forget them when the member stops being live. Messages of other kinds SHALL NOT change the kept tags.

#### Scenario: Member stops

- **WHEN** two daemons form a peer group and one of them stops
- **THEN** within 4 seconds the other logs that the stopped member is not live and its `emfor_peer_up` series for that member is 0

#### Scenario: Heartbeat announces services left out

- **WHEN** this daemon has `[peers]` and the services `api.svc.emfor` and `db.svc.emfor` have `peers = false` while `web.svc.emfor` has `peers = true`
- **THEN** every heartbeat it sends carries `"off":["a513a047","fa6f63b4"]`, and after a reload that sets `peers = true` for both, heartbeats carry no `off`

##### Example: service tags

| Service | Service tag |
| --- | --- |
| `api.svc.emfor` | `a513a047` |
| `db.svc.emfor` | `fa6f63b4` |
| `web.svc.emfor` | `99e6433f` |

#### Scenario: More services left out than fit

- **WHEN** this daemon starts with 81 services with `peers = false` whose tags are distinct
- **THEN** every heartbeat carries the 80 smallest of those tags in byte order, the datagram is at most 1200 bytes, and the daemon logs one `warn` line saying 1 tag is not sent

#### Scenario: Kept tags follow the latest heartbeat

- **WHEN** member `10.0.0.6:5302` sends a heartbeat with `"off":["a513a047"]`, then a `result` message, then a heartbeat without `off`
- **THEN** after the first heartbeat and after the `result` message this daemon keeps the tag `a513a047` for that member, and after the second heartbeat it keeps none

##### Example: heartbeat `off` field

| `off` in an otherwise valid heartbeat | Result |
| --- | --- |
| absent | accepted, no tags kept |
| `[]` | accepted, no tags kept |
| `["a513a047","fa6f63b4"]` | accepted, both tags kept |
| `["A513A047"]` | rejected: malformed |
| `["a513a04"]` | rejected: malformed |
| `"a513a047"` | rejected: malformed |
| `[42]` | rejected: malformed |


<!-- @trace
source: emfor-peers-off-announce
updated: 2026-11-13
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/peers.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/quorum.rs
  - CHANGELOG.md
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual/08-operations.md
  - docs/manual.zh_TW/01-concepts.md
  - crates/emfor-daemon/src/supervisor.rs
  - docs/manual/01-concepts.md
  - crates/emfor-daemon/src/metrics.rs
  - docs/manual.zh_TW/08-operations.md
  - crates/emfor-daemon/src/writer.rs
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

When one of this daemon's own checks moves a backend from `healthy` to `unhealthy`, the daemon SHALL send one `down` message to every member other than itself, with the fields `service`, `digest` (the service's check definition digest), `backend` (the backend id) and `reason` (the transition's reason, cut at a character boundary to at most 256 bytes of UTF-8). A move from `unknown` to `unhealthy`, or a move made at a check time decided by votes alone (see "Split checks"), SHALL NOT send a message. A message SHALL NOT be sent again.

When a daemon accepts a `down` message for a service it has with the same digest and a backend of that service, it SHALL start a fast recheck run for that backend when all of these hold: the backend is `healthy` in this daemon, no fast recheck run is in progress for it, and its last fast recheck run ended more than `fall × interval` ago. Otherwise it SHALL ignore the message. A `down` message SHALL NOT change any backend's state by itself; states change only through this daemon's checks and their combined results (see "Region verdict and combined result"). Starting a fast recheck run SHALL be logged at `info` level as `peer <member> reports backend <backend> of <service> down (<reason>); checking now`.

#### Scenario: Peer sees the failure first

- **WHEN** daemons A and B in one peer group both check a TCP backend of `api.svc.emfor` with `fall = 3`, A with `interval = "1s"` and B with `interval = "30s"` (the digest leaves `interval` out), both see it `healthy`, and the backend stops listening
- **THEN** B, which on its own would need at least a minute, marks it `unhealthy` within 2 seconds after A did

#### Scenario: Digest differs

- **WHEN** A and B check the same backend but B's service has `timeout = "500ms"` and A's has `timeout = "1s"`, and A sends a `down` message for it
- **THEN** B ignores the message and checks the backend on its normal schedule

#### Scenario: Moves decided by votes are not announced

- **WHEN** a daemon that is not an assigned checker of a `healthy` backend marks it `unhealthy` at a check time decided by votes alone
- **THEN** it sends no `down` message for it

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
source: emfor-peers-split-checks
updated: 2026-11-13
code:
  - crates/emfor-daemon/src/schedule.rs
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/peers.rs
  - docs/manual/01-concepts.md
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/quorum.rs
  - crates/emfor-daemon/src/metrics.rs
  - CHANGELOG.md
  - docs/manual/03-configuration.md
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

---
### Requirement: Assigned checkers and check results

For every backend of every service, a daemon with `[peers]` SHALL choose the backend's assigned checkers among the candidates: itself and every member that is currently live and whose kept service tags (see "Peer heartbeats and liveness") do not include the service tag of the backend's service. For each candidate it SHALL compute MD5 over the UTF-8 bytes of `<service name>/<backend id>/<candidate listen>`, where the candidate listen is written as in the config (IPv6 as `[addr]:port`), read the first 4 bytes of the digest as a big-endian unsigned 32-bit integer, and take the 3 candidates with the largest values as the assigned checkers, ranked from the largest; on equal values the candidate whose listen string is smaller in byte order SHALL rank first. With fewer than 3 candidates the backend SHALL have no assigned checkers. The choice SHALL be made again for every check, from the members live and the tags kept at that moment. When two services of this daemon have the same service tag, a member that announces the tag SHALL be left out of the candidates of both.

When the daemon is an assigned checker of a backend and its own check of that backend gives `up` or `down`, it SHALL send one `result` message to every member other than itself, with the fields `service`, `digest` (the service's check definition digest), `backend` (the backend id), `outcome` (`"up"` or `"down"`, the result of its own check, never the combined result), `reason` (only when `outcome` is `"down"`: the check's reason cut at a character boundary to at most 256 bytes of UTF-8) and `interval_ms` (the service's `interval` in milliseconds). An `error` result SHALL NOT send a message. A `result` message that lacks one of these fields, has another `outcome`, or has an `interval_ms` that is not a positive integer SHALL be dropped as `malformed`. An accepted `result` message for a service and backend the receiver has SHALL be kept as that sender's latest result for the backend, together with its digest, its `interval_ms` and the time the receiver accepted it; one for a service or backend the receiver does not have SHALL be ignored.

#### Scenario: Assigned checkers among five live members

- **WHEN** members `10.0.0.5:5302` (this daemon) through `10.0.0.9:5302` are all live and the daemon checks backend `10.0.0.11:8080` of `api.svc.emfor`
- **THEN** the assigned checkers are, in rank order, `10.0.0.5:5302`, `10.0.0.8:5302` and `10.0.0.6:5302`, and this daemon sends a `result` message after each `up` or `down` check of that backend

##### Example: rendezvous values for api.svc.emfor

| Backend | Member | Value |
| --- | --- | --- |
| `10.0.0.11:8080` | `10.0.0.5:5302` | 3974634306 |
| `10.0.0.11:8080` | `10.0.0.8:5302` | 3444584285 |
| `10.0.0.11:8080` | `10.0.0.6:5302` | 1483479382 |
| `10.0.0.11:8080` | `10.0.0.7:5302` | 1161570282 |
| `10.0.0.11:8080` | `10.0.0.9:5302` | 367848677 |
| `10.0.0.12:8080` | `10.0.0.9:5302` | 4163305238 |
| `10.0.0.12:8080` | `10.0.0.6:5302` | 3934586029 |
| `10.0.0.12:8080` | `10.0.0.7:5302` | 2645478105 |
| `10.0.0.12:8080` | `10.0.0.8:5302` | 2352845107 |
| `10.0.0.12:8080` | `10.0.0.5:5302` | 729780139 |

#### Scenario: A member stops being live

- **WHEN** in the group above `10.0.0.8:5302` stops being live
- **THEN** the assigned checkers of `10.0.0.11:8080` become `10.0.0.5:5302`, `10.0.0.6:5302` and `10.0.0.7:5302`

#### Scenario: A member announces a service left out

- **WHEN** in the group above `10.0.0.8:5302` stays live and its heartbeats carry `"off":["a513a047"]`, the service tag of `api.svc.emfor`
- **THEN** the assigned checkers of `10.0.0.11:8080` become `10.0.0.5:5302`, `10.0.0.6:5302` and `10.0.0.7:5302`, and those of `10.0.0.12:8080` stay `10.0.0.9:5302`, `10.0.0.6:5302` and `10.0.0.7:5302`

#### Scenario: Not an assigned checker

- **WHEN** this daemon is `10.0.0.5:5302` in the group above and checks backend `10.0.0.12:8080`
- **THEN** it sends no `result` message for that backend

#### Scenario: Error results are not sent

- **WHEN** an assigned checker's own check of a backend gives `error`
- **THEN** it sends no `result` message for that check

#### Scenario: Two-member group

- **WHEN** a group has 2 members and both are live
- **THEN** no backend has assigned checkers and no `result` message is sent


<!-- @trace
source: emfor-peers-off-announce
updated: 2026-11-13
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/peers.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/quorum.rs
  - CHANGELOG.md
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual/08-operations.md
  - docs/manual.zh_TW/01-concepts.md
  - crates/emfor-daemon/src/supervisor.rs
  - docs/manual/01-concepts.md
  - crates/emfor-daemon/src/metrics.rs
  - docs/manual.zh_TW/08-operations.md
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Region verdict and combined result

After each of its own checks of a backend, a daemon with `[peers]` SHALL count the votes of the backend's assigned checkers. The vote of this daemon, when it is an assigned checker, SHALL be the result of this check when it is `up` or `down`. The vote of another assigned checker SHALL be its latest kept `result` when that result's digest equals this daemon's current digest for the service, it was accepted less than `3 × interval_ms` of that result ago, and the sender is live; otherwise that checker SHALL have no vote. With `f` votes of which `d` are `down`:

- with no assigned checkers or `f` = 0, the combined result SHALL be this daemon's own result, with its reason unchanged;
- with `d` ≥ 2, the combined result SHALL be `down` with the reason `peers <d>/<f>: <reason>`, where `<reason>` is the reason of the highest-ranked `down` vote, whatever this daemon's own result is;
- otherwise the combined result SHALL be this daemon's own result, and when that result is `down` its reason SHALL be `local: <own reason>`.

The combined result SHALL take the place of the check result everywhere a check result is used: the health state machine, the stored state and reason, the backend state history, state change logs, check counters and fast recheck runs. Reasons with a prefix SHALL be normalized and bounded like any other reason. The check latency SHALL still be measured from this daemon's own check and SHALL count only when the combined result is `up`.

A backend whose combined result is `down` with `d` ≥ 2 while this daemon's own result was `up` SHALL be a disagreement until a later combined result of that backend is not, or until the backend or its service is removed. When a backend becomes a disagreement the daemon SHALL log one line at `info` level: `backend <backend> of <service> is down at <d>/<f> peers but up here`. When the daemon accepts a `down` or `result` message for a service it has whose digest differs from its own, it SHALL log at `debug` level and record a digest mismatch for that service and member, which SHALL last until no such message from that member for that service has been accepted for 60 seconds, or the member is removed.

#### Scenario: Two of three assigned checkers see the backend down

- **WHEN** in a group of three daemons with the same check definition, two daemons' own checks of a backend give `down` with reason `connect: Connection refused` and the third daemon's own checks give `up`
- **THEN** the third daemon marks the backend `unhealthy` after `fall` checks, its stored reason starts with `peers 2/`, and the backend is a disagreement there

#### Scenario: Only one daemon sees the backend down

- **WHEN** in a group of three daemons only one daemon's own checks of a backend give `down` with reason `timeout`
- **THEN** that daemon marks the backend `unhealthy` with reason `local: timeout`, and the other two keep it `healthy`

#### Scenario: Too few live members

- **WHEN** a group has 3 members and one of them stops being live
- **THEN** each remaining daemon decides every backend from its own checks only, with reasons as its checks give them

##### Example: combined result

"This daemon assigned" tells whether this daemon is one of the three assigned checkers; when it is, its own `up` or `down` result is a vote too. "Kept votes" are the fresh kept results of the other assigned checkers, in rank order.

| This daemon assigned | Kept votes | Own result | Combined result | Reason |
| --- | --- | --- | --- | --- |
| — (2 candidates, no assigned checkers) | — | down `timeout` | down | `timeout` |
| no | none | down `timeout` | down | `timeout` |
| yes | down `refused`, down `refused` | up | down | `peers 2/3: refused` |
| no | down `refused`, down `reset` | up | down | `peers 2/2: refused` |
| yes | down `refused`, down `reset` | error `plugin exited` | down | `peers 2/2: refused` |
| yes | up, up | down `timeout` | down | `local: timeout` |
| yes | down `refused` | up | up | — |
| no | down `refused` (only fresh vote) | up | up | — |
| no | down `refused`, down `refused`, up | error `plugin exited` | down | `peers 2/3: refused` |
| yes | up, up | error `plugin exited` | error | `plugin exited` |

##### Example: which kept results are votes

| Kept result of another assigned checker | Vote |
| --- | --- |
| same digest, accepted 1s ago, `interval_ms` 2000, sender live | yes |
| same digest, accepted 7s ago, `interval_ms` 2000, sender live | no (older than 6s) |
| same digest, accepted 7s ago, `interval_ms` 5000, sender live | yes |
| different digest, accepted 1s ago | no, and a digest mismatch is recorded |
| same digest, accepted 1s ago, sender not live | no |

<!-- @trace
source: emfor-peers-quorum
updated: 2026-11-13
code:
  - crates/emfor-daemon/Cargo.toml
  - docs/manual/03-configuration.md
  - CHANGELOG.md
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/metrics.rs
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/peers.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/schedule.rs
  - crates/emfor-daemon/src/quorum.rs
  - crates/emfor-daemon/src/writer.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-format/src/lib.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/supervisor.rs
  - docs/manual/01-concepts.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Split checks

For a backend that has assigned checkers of which this daemon is not one, the daemon SHALL decide each scheduled check time as follows, counting the fresh votes of the other assigned checkers exactly as in "Region verdict and combined result" (`f` votes, `d` of them `down`, `u` of them `up`). It SHALL make its own check when any of these holds: it has not made an own check of the backend since the backend's task started; its last own result of the backend was not `up`; it decided the previous 4 check times of the backend by votes alone; a fast recheck run is in progress; neither `d` ≥ 2 nor `u` ≥ 2; or `d` < 2 and `u` ≥ 2 while the backend has no latency (spec state-store, "Check latency"), so that a backend never turns healthy here without a measured latency. The result of such an own check SHALL be combined as in "Region verdict and combined result". Otherwise it SHALL NOT check, and the applied result SHALL be `down` with the reason `peers <d>/<f>: <reason>`, `<reason>` being the reason of the highest-ranked `down` vote, when `d` ≥ 2, and `up` otherwise. The assigned checkers SHALL be chosen again at every check time, from the members live at that moment; a daemon that is an assigned checker, or a backend without assigned checkers, SHALL be checked at every check time as in "Region verdict and combined result".

A result decided by votes alone SHALL be applied like a check result to the health state machine, the stored state and reason, the backend state history, state change logs and check counters. It SHALL NOT add a latency sample (the latency still becomes empty when the backend turns `unhealthy`, spec state-store "Check latency"), send a `result` or `down` message, or change whether the backend is a disagreement. The daemon SHALL count each check time decided by votes alone in `emfor_peer_checks_skipped_total`.

#### Scenario: Sparse own checks while the assigned checkers see the backend up

- **WHEN** this daemon is not an assigned checker of a backend with `interval = "1s"` and `jitter = "0%"`, every assigned checker's fresh vote is `up`, its own checks give `up`, and 10 check times pass starting with an own check
- **THEN** the daemon makes own checks at the 1st and 6th check times only, and the backend stays `healthy`

#### Scenario: Local failure while the region is up

- **WHEN** in the case above the own check at the 6th check time gives `down` with reason `connection refused` and `fall = 3`
- **THEN** the daemon makes own checks at the 7th and 8th check times too, and after the 8th the backend is `unhealthy` with reason `local: connection refused`

#### Scenario: Region down without an own check

- **WHEN** this daemon is not an assigned checker of a `healthy` backend with `fall = 3`, its own check at the previous check time gave `up`, and from now on two of the three assigned checkers' fresh votes are `down` with reason `refused` and the third is `up`
- **THEN** the next 3 check times are decided by votes alone and after the 3rd the backend is `unhealthy` with reason `peers 2/3: refused`

#### Scenario: Recovery decided by votes waits for a latency

- **WHEN** a backend turned `unhealthy` at check times decided by votes alone, so its latency is empty, and the assigned checkers' fresh votes become `up` again
- **THEN** the daemon makes its own check at the next check time, and the backend is `healthy` again only with a latency from that check

#### Scenario: Assigned checkers stop being live

- **WHEN** in a group of four members two members stop being live while this daemon is not an assigned checker of a backend
- **THEN** the backend has no assigned checkers and the daemon checks it at every check time

##### Example: what this daemon does at one check time

Backend `10.0.0.12:8080` of `api.svc.emfor`; this daemon is `10.0.0.5:5302`; the assigned checkers are `10.0.0.9:5302`, `10.0.0.6:5302` and `10.0.0.7:5302`, and "Votes" lists their fresh votes in that order.

| Own check made since start | Last own result | Latency | Check times decided by votes in a row | Votes | Action | Applied result |
| --- | --- | --- | --- | --- | --- | --- |
| no | — | none | 0 | up, up, up | own check, `up` | up |
| yes | up | 820µs | 0 | up, up, up | votes alone | up |
| yes | up | 820µs | 2 | down `refused`, down `refused`, up | votes alone | down `peers 2/3: refused` |
| yes | up | none | 2 | down `refused`, down `refused`, up | votes alone | down `peers 2/3: refused` |
| yes | up | none | 0 | up, up, up | own check, `up` | up |
| yes | up | 820µs | 4 | up, up, up | own check, `up` | up |
| yes | up | 820µs | 1 | up, down `refused` | own check, `up` | up |
| yes | up | 820µs | 1 | none | own check, `down` `timeout` | down `timeout` |
| yes | down `timeout` | 820µs | 0 | up, up, up | own check, `down` `timeout` | down `local: timeout` |
| yes | error `plugin exited` | 820µs | 0 | up, up | own check, `error` `plugin exited` | error `plugin exited` |

<!-- @trace
source: emfor-peers-split-checks
updated: 2026-11-13
code:
  - crates/emfor-daemon/src/schedule.rs
  - docs/manual/08-operations.md
  - crates/emfor-daemon/src/peers.rs
  - docs/manual/01-concepts.md
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/quorum.rs
  - crates/emfor-daemon/src/metrics.rs
  - CHANGELOG.md
  - docs/manual/03-configuration.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Services left out of the peer group

A service whose effective `peers` setting (spec service-config) is `false` SHALL take no part in the peer group on this daemon, while the daemon and its other services stay members. For such a service the daemon SHALL:

- not send `down` or `result` messages for any of its backends;
- check every backend at every check time and apply its own result with its reason unchanged, without counting votes, deciding by votes alone, or marking a disagreement;
- ignore every accepted `down` or `result` message for the service as one for a service it does not have: start no fast recheck run, keep no result, and record no digest mismatch;
- not count check times of the service in `emfor_peer_checks_skipped_total`.

When a reload turns `peers` of a service from `true` to `false`, the daemon SHALL drop the kept results, disagreements and digest mismatches of that service, and a fast recheck run already in progress SHALL run to its end. When a reload turns it back to `true`, the service SHALL take part again from its next check time, starting with no kept results. This daemon SHALL announce the services it leaves out in the `off` field of its heartbeats (see "Peer heartbeats and liveness"), so that other members leave it out of the candidates for those services' backends; a member that does not read `off` keeps counting this daemon as a candidate. Tags not sent because of the limit of 80 SHALL leave this daemon a candidate for those services on every member. Without a `[peers]` table the setting SHALL have no effect.

#### Scenario: A service left out on one member

- **WHEN** a group has three live members with the same check definition for `api.svc.emfor`, `api.svc.emfor` has `peers = false` on member A only, and the other two members' own checks of a backend give `down` with reason `refused` while A's give `up`
- **THEN** the backend stays `healthy` on A with no disagreement, A sends no `result` or `down` message for `api.svc.emfor`, the backend has no assigned checkers on the other two members because A is not a candidate there, and the other two members apply their own results and mark it `unhealthy`

#### Scenario: Down message for a service left out

- **WHEN** a member sends a `down` message for a `healthy` backend of a service that has `peers = false` on this daemon
- **THEN** this daemon starts no fast recheck run and checks the backend at its next `interval`

#### Scenario: Leaving the group on reload drops kept state

- **WHEN** a backend of `api.svc.emfor` is a disagreement on this daemon and a reload sets `peers = false` for `api.svc.emfor`
- **THEN** within 2 seconds the disagreement is no longer in `meta/peers`, and the next applied result of the backend is this daemon's own result

##### Example: what this daemon does for one backend

This daemon is not an assigned checker of the backend; the assigned checkers' fresh votes are `down` `refused`, `down` `refused`, `up`; this daemon makes its own check at this check time and it gives `up`.

| `peers` of the service | Votes counted | Applied result | `result` message sent | Disagreement |
| --- | --- | --- | --- | --- |
| `true` | yes | down `peers 2/3: refused` | no | yes |
| `false` | no | up | no | no |

<!-- @trace
source: emfor-peers-off-announce
updated: 2026-11-13
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/peers.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/quorum.rs
  - CHANGELOG.md
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual/08-operations.md
  - docs/manual.zh_TW/01-concepts.md
  - crates/emfor-daemon/src/supervisor.rs
  - docs/manual/01-concepts.md
  - crates/emfor-daemon/src/metrics.rs
  - docs/manual.zh_TW/08-operations.md
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/peers.rs
-->