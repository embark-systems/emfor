## MODIFIED Requirements

### Requirement: Split checks

For a backend that has assigned checkers of which this daemon is not one, the daemon SHALL decide each scheduled check time as follows, counting the fresh votes of the other assigned checkers exactly as in "Region verdict and combined result" (`f` votes, `d` of them `down`, `u` of them `up`). It SHALL make its own check when any of these holds: it has not made an own check of the backend since the backend's task started; its last own result of the backend was not `up`; it decided the previous 2 check times of the backend by votes alone; a fast recheck run is in progress; neither `d` ≥ 2 nor `u` ≥ 2; or `d` < 2 and `u` ≥ 2 while the backend has no latency (spec state-store, "Check latency"), so that a backend never turns healthy here without a measured latency. The result of such an own check SHALL be combined as in "Region verdict and combined result". Otherwise it SHALL NOT check, and the applied result SHALL be `down` with the reason `peers <d>/<f>: <reason>`, `<reason>` being the reason of the highest-ranked `down` vote, when `d` ≥ 2, and `up` otherwise. The assigned checkers SHALL be chosen again at every check time, from the members live at that moment; a daemon that is an assigned checker, or a backend without assigned checkers, SHALL be checked at every check time as in "Region verdict and combined result".

When such an own check of a daemon that is not an assigned checker gives `down` while `d` < 2 and `u` ≥ 2, so that the combined result is `local: <reason>`, the daemon SHALL start a fast recheck run for the backend (see "Backend failure notification") when all of these hold after the result is applied: the backend is `healthy` in this daemon, no fast recheck run is in progress for it, and its last fast recheck run ended more than `fall × interval` ago. An own check that gives `error`, an own check of an assigned checker, and an own check made while `u` < 2 or `d` ≥ 2 SHALL NOT start a run. Starting such a run SHALL be logged at `info` level as `backend <backend> of <service> fails here while its assigned checkers see it up (<reason>); checking now`.

A result decided by votes alone SHALL be applied like a check result to the health state machine, the stored state and reason, the backend state history, state change logs and check counters. It SHALL NOT add a latency sample (the latency still becomes empty when the backend turns `unhealthy`, spec state-store "Check latency"), send a `result` or `down` message, or change whether the backend is a disagreement. The daemon SHALL count each check time decided by votes alone in `emfor_peer_checks_skipped_total`.

#### Scenario: Sparse own checks while the assigned checkers see the backend up

- **WHEN** this daemon is not an assigned checker of a backend with `interval = "1s"` and `jitter = "0%"`, every assigned checker's fresh vote is `up`, its own checks give `up`, and 10 check times pass starting with an own check
- **THEN** the daemon makes own checks at the 1st, 4th, 7th and 10th check times only, and the backend stays `healthy`

#### Scenario: Local failure while the region is up

- **WHEN** in the case above with `fall = 3` the own check at the 4th check time gives `down` with reason `connection refused`, and so do the following own checks
- **THEN** the daemon makes two more own checks 200 ms apart, the first 200 ms after the 4th check time, and after the second of them, about 400 ms after the 4th check time, the backend is `unhealthy` with reason `local: connection refused`

#### Scenario: Assigned checker does not start a run on its own failure

- **WHEN** this daemon is an assigned checker of a `healthy` backend with `interval = "1s"`, `jitter = "0%"` and `fall = 3`, the other two assigned checkers' fresh votes are `up`, and its own check gives `down` with reason `connection refused`
- **THEN** its next own check is 1 second later, and the backend is `unhealthy` after the third failed check, 2 seconds after the first

#### Scenario: Region down without an own check

- **WHEN** this daemon is not an assigned checker of a `healthy` backend with `fall = 3`, its own check at the previous check time gave `up`, and from now on two of the three assigned checkers' fresh votes are `down` with reason `refused` and the third is `up`
- **THEN** the next 2 check times are decided by votes alone, the 3rd is an own check whose result is `peers 2/3: refused` whatever the own check gives, and after the 3rd the backend is `unhealthy` with reason `peers 2/3: refused`; because that move is made by an own check, the daemon sends one `down` message (see "Backend failure notification")

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
| yes | up | 820µs | 1 | up, up, up | votes alone | up |
| yes | up | 820µs | 1 | down `refused`, down `refused`, up | votes alone | down `peers 2/3: refused` |
| yes | up | none | 1 | down `refused`, down `refused`, up | votes alone | down `peers 2/3: refused` |
| yes | up | none | 0 | up, up, up | own check, `up` | up |
| yes | up | 820µs | 2 | up, up, up | own check, `up` | up |
| yes | up | 820µs | 1 | up, down `refused` | own check, `up` | up |
| yes | up | 820µs | 1 | none | own check, `down` `timeout` | down `timeout` |
| yes | down `timeout` | 820µs | 0 | up, up, up | own check, `down` `timeout` | down `local: timeout` |
| yes | error `plugin exited` | 820µs | 0 | up, up | own check, `error` `plugin exited` | error `plugin exited` |

##### Example: whether a failed own check starts a fast recheck run

Same backend and daemon as above, `interval = "2s"`, `fall = 3`, so `fall × interval` is 6 s. "State after the result" is the backend's state once the own check's combined result is applied.

| Assigned checker | Votes | Own result | State after the result | Fast run in progress | Last fast run ended | Result |
| --- | --- | --- | --- | --- | --- | --- |
| no | up, up, up | down `refused` | healthy | no | never | start a fast run |
| no | up, up, down `refused` | down `refused` | healthy | no | 7s ago | start a fast run |
| no | up, up, up | down `refused` | healthy | no | 1s ago | no run |
| no | up, up, up | down `refused` | healthy | yes | — | no new run; the run in progress goes on |
| no | up, up, up | down `refused` | unhealthy | no | never | no run |
| no | up, up, up | error `plugin exited` | healthy | no | never | no run |
| no | up, down `refused` | down `refused` | healthy | no | never | no run |
| no | down `refused`, down `refused`, up | down `refused` | healthy | no | never | no run |
| yes | up, up | down `refused` | healthy | no | never | no run |
