## MODIFIED Requirements

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
