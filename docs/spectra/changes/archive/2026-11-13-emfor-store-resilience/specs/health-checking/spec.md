## MODIFIED Requirements

### Requirement: Check scheduling with jitter and no overlap

The daemon SHALL check every backend of every service repeatedly. Let `fast` be the smaller of `interval` and 200ms. The first check of a backend SHALL start after a random delay in `[0, fast)`. When a check finishes and the backend's health state is `unknown` and that check's result was `up` or `down`, the next check SHALL start `fast` after the previous check finishes, without jitter. Otherwise each following check SHALL start after the previous check finishes plus `interval` adjusted by a uniformly random offset within `±jitter` of `interval`. Two checks of the same backend SHALL NOT run at the same time.

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

## ADDED Requirements

### Requirement: Reasons are bounded and printable

Every check result's reason, from every checker and plugin, SHALL be normalized before it reaches the health state, the store, the logs of state changes, or the metrics: tab, carriage return and line feed SHALL become the two characters `\t`, `\r` and `\n`; every other character in Unicode category Cc (U+0000–U+001F, U+007F, U+0080–U+009F) SHALL become `\u{` followed by its code point in lowercase hex without leading zeros and `}`; then a reason longer than 512 characters SHALL be cut to its first 511 characters followed by `…`. All other characters, including `\`, SHALL stay as they are.

#### Scenario: Escape sequence from a backend

- **WHEN** an `exec` check prints `bad\x1b[2Jstate` (with a real ESC byte) and exits 1
- **THEN** the backend's reason in the store is `bad\u{1b}[2Jstate`

##### Example: normalization

| Reason produced by the check | Stored reason |
| --- | --- |
| `connection refused` | `connection refused` |
| `a<TAB>b` | `a\tb` |
| `x<ESC>y` | `x\u{1b}y` |
| `x<U+0085>y` | `x\u{85}y` |
| 600 × `a` | 511 × `a` followed by `…` |
| 512 × `a` | 512 × `a` |
