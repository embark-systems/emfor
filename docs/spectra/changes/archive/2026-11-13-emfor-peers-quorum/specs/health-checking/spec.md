## MODIFIED Requirements

### Requirement: Health state machine with rise and fall

Each backend SHALL have a health state of `unknown`, `healthy`, or `unhealthy`, a consecutive-success counter, and a consecutive-failure counter, all starting at `unknown`, 0, 0 when the daemon starts. An `up` result SHALL increment the success counter and reset the failure counter; reaching `rise` SHALL set the state to `healthy`. A `down` result SHALL increment the failure counter and reset the success counter; reaching `fall` SHALL set the state to `unhealthy`. An `error` result SHALL change neither counter nor the state, except that when a backend has received only `error` results for longer than `fall × interval`, its state SHALL become `unknown` and both counters SHALL reset to 0. In a daemon with `[peers]`, the result of a check, as used by this requirement and by every other requirement of this spec (scheduling, fast recheck runs, logging, latency and counters), SHALL be the combined result of that check defined in spec peer-group ("Region verdict and combined result"); without `[peers]` it SHALL be the result the checker returned.

#### Scenario: Backend becomes healthy

- **WHEN** a backend in state `unknown` with `rise = 2` gets two `up` results in a row
- **THEN** its state is `healthy`

##### Example: state transitions with rise = 2, fall = 3

| Results in order | Final state |
| --- | --- |
| up | unknown |
| up, up | healthy |
| up, up, down, down | healthy |
| up, up, down, down, down | unhealthy |
| up, up, down, down, up, down, down | healthy (up reset the failure counter) |
| down, down, down | unhealthy |
| down, down, down, up, up | healthy |
| up, up, error, error | healthy |

#### Scenario: Persistent errors make the state unknown

- **WHEN** a `healthy` backend with `interval = "2s"` and `fall = 3` receives only `error` results for more than 6s
- **THEN** its state becomes `unknown` and both counters are 0

#### Scenario: Combined result feeds the state machine

- **WHEN** a daemon in a peer group of three has a `healthy` backend with `fall = 3`, its own checks give `up`, and the two other assigned checkers keep reporting `down`
- **THEN** after 3 checks the backend is `unhealthy` with a reason starting with `peers 2/`
