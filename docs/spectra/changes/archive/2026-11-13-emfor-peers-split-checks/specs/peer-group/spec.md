## ADDED Requirements

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

## MODIFIED Requirements

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
