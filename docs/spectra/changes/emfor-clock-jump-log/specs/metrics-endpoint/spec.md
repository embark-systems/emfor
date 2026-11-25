## ADDED Requirements

### Requirement: Clock jump counter

The `/metrics` output SHALL include the counter `emfor_clock_jumps_total`, with one `# HELP` line and one `# TYPE` line of type `counter` and no labels. The value SHALL be the number of host clock jumps the daemon has detected since it started, each detected jump counting once, including jumps for which the clock jump log line was suppressed by its rate limit. The value SHALL be 0 when none has been detected.

#### Scenario: No jump detected

- **WHEN** the daemon has run without detecting a clock jump and `/metrics` is requested
- **THEN** the output contains the line `emfor_clock_jumps_total 0` and exactly one `# TYPE emfor_clock_jumps_total counter` line

#### Scenario: Jumps counted when the log line is suppressed

- **WHEN** the daemon detects two jumps 5 seconds apart, so only the first one is logged
- **THEN** `/metrics` shows `emfor_clock_jumps_total 2`
