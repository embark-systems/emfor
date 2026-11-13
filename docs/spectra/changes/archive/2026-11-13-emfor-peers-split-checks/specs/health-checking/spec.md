## MODIFIED Requirements

### Requirement: Check scheduling with jitter and no overlap

The daemon SHALL check every backend of every service repeatedly; in a peer group a daemon that is not an assigned checker of a backend SHALL instead decide some scheduled check times from the assigned checkers' votes without checking (spec peer-group, "Split checks"), and such a check time SHALL follow the same timing rules as a check whose result is the applied result. Let `fast` be the smaller of `interval` and 200ms. The first check of a backend SHALL start after a random delay in `[0, fast)`. When a check finishes and the backend's health state is `unknown` and that check's result was `up` or `down`, the next check SHALL start `fast` after the previous check finishes, without jitter. Otherwise each following check SHALL start after the previous check finishes plus `interval` adjusted by a uniformly random offset within `±jitter` of `interval`. When a fast recheck run starts because a peer reported the backend down (spec peer-group), the next check SHALL start at once, or right after the check in progress finishes; each following check of that run SHALL start `fast` after the previous one finishes, without jitter, until a check result is `up`, the backend becomes `unhealthy`, or the run has made `fall` checks, after which the rules above apply again. Two checks of the same backend SHALL NOT run at the same time.

#### Scenario: Spacing between checks

- **WHEN** a `healthy` backend has `interval = "2s"` and `jitter = "10%"`
- **THEN** the gap between the end of one check and the start of the next is between 1.8s and 2.2s

#### Scenario: Zero jitter

- **WHEN** a `healthy` backend has `interval = "1s"` and `jitter = "0%"`
- **THEN** the gap between the end of one check and the start of the next is 1s

#### Scenario: Fast checks until the state is known

- **WHEN** the daemon starts with a backend that passes every check, `interval = "2s"`, `rise = 2` and `jitter = "0%"`
- **THEN** the backend is `healthy` within 1 second of the start

##### Example: wait before the next check

| `interval` | state after the check | result of the check | wait |
| --- | --- | --- | --- |
| `2s` | unknown | up | 200ms |
| `2s` | unknown | down | 200ms |
| `2s` | unknown | error | 2s ± jitter |
| `2s` | healthy | up | 2s ± jitter |
| `2s` | unhealthy | down | 2s ± jitter |
| `100ms` | unknown | up | 100ms |

#### Scenario: Fast recheck run after a peer report

- **WHEN** a `healthy` backend with `interval = "5s"`, `fall = 3` and `jitter = "10%"` has just finished a check, a peer's `down` message for it starts a fast recheck run, and every check of the run returns `down`
- **THEN** three checks start, the first at once and each next one 200ms after the previous finishes, the backend becomes `unhealthy` after the third, and the following check starts 5s ± 10% later

#### Scenario: Check times decided by votes keep the schedule

- **WHEN** a `healthy` backend with `interval = "2s"` and `jitter = "0%"` is decided by votes alone at one check time in a peer group
- **THEN** the next check time is 2s after that one
