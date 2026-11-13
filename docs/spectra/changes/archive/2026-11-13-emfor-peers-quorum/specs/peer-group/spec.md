## ADDED Requirements

### Requirement: Assigned checkers and check results

For every backend of every service, a daemon with `[peers]` SHALL choose the backend's assigned checkers among the candidates: itself and every member that is currently live. For each candidate it SHALL compute MD5 over the UTF-8 bytes of `<service name>/<backend id>/<candidate listen>`, where the candidate listen is written as in the config (IPv6 as `[addr]:port`), read the first 4 bytes of the digest as a big-endian unsigned 32-bit integer, and take the 3 candidates with the largest values as the assigned checkers, ranked from the largest; on equal values the candidate whose listen string is smaller in byte order SHALL rank first. With fewer than 3 candidates the backend SHALL have no assigned checkers. The choice SHALL be made again for every check, from the members live at that moment.

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

#### Scenario: Not an assigned checker

- **WHEN** this daemon is `10.0.0.5:5302` in the group above and checks backend `10.0.0.12:8080`
- **THEN** it sends no `result` message for that backend

#### Scenario: Error results are not sent

- **WHEN** an assigned checker's own check of a backend gives `error`
- **THEN** it sends no `result` message for that check

#### Scenario: Two-member group

- **WHEN** a group has 2 members and both are live
- **THEN** no backend has assigned checkers and no `result` message is sent


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


## MODIFIED Requirements

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


### Requirement: Backend failure notification

When a check moves a backend from `healthy` to `unhealthy`, the daemon SHALL send one `down` message to every member other than itself, with the fields `service`, `digest` (the service's check definition digest), `backend` (the backend id) and `reason` (the transition's reason, cut at a character boundary to at most 256 bytes of UTF-8). A move from `unknown` to `unhealthy` SHALL NOT send a message. A message SHALL NOT be sent again.

When a daemon accepts a `down` message for a service it has with the same digest and a backend of that service, it SHALL start a fast recheck run for that backend when all of these hold: the backend is `healthy` in this daemon, no fast recheck run is in progress for it, and its last fast recheck run ended more than `fall × interval` ago. Otherwise it SHALL ignore the message. A `down` message SHALL NOT change any backend's state by itself; states change only through this daemon's checks and their combined results (see "Region verdict and combined result"). Starting a fast recheck run SHALL be logged at `info` level as `peer <member> reports backend <backend> of <service> down (<reason>); checking now`.

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
