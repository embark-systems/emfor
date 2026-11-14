## ADDED Requirements

### Requirement: Slow start effective weight

A backend's slow start step SHALL be 10 when the store or the daemon's state gives it `recovered_at` `null`, when its service's `slow_start_ms` is 0, when the current Unix time in milliseconds is earlier than `recovered_at`, or when the time elapsed since `recovered_at` is at least `slow_start_ms`. Otherwise the step SHALL be `max(1, ceil(10 × elapsed / slow_start_ms))`. A backend is ramping when its step is below 10. Its effective weight SHALL be its base weight times its step, where the base weight is the backend's `weight` for `balance = "weighted"` and 1 for every other `balance`.

#### Scenario: Step during a 30 second slow start

- **WHEN** a backend's `recovered_at` is 1762905600000 and its service's `slow_start_ms` is 30000
- **THEN** its step is 1 at 1762905600000, 1 at 1762905603000, 4 at 1762905610000, 10 at 1762905630000 and 10 at 1762905640000

##### Example: step and effective weight

| `recovered_at` | `slow_start_ms` | Now | `balance` | `weight` | Step | Effective weight |
| --- | --- | --- | --- | --- | --- | --- |
| 1762905600000 | 30000 | 1762905600000 | round-robin | 1 | 1 | 1 |
| 1762905600000 | 30000 | 1762905601000 | round-robin | 1 | 1 | 1 |
| 1762905600000 | 30000 | 1762905607500 | round-robin | 1 | 3 | 3 |
| 1762905600000 | 30000 | 1762905629900 | weighted | 4 | 10 | 40 |
| 1762905600000 | 30000 | 1762905630000 | weighted | 4 | 10 | 40 |
| 1762905600000 | 30000 | 1762905607500 | weighted | 4 | 3 | 12 |
| null | 30000 | 1762905607500 | weighted | 4 | 10 | 40 |
| 1762905600000 | 0 | 1762905607500 | round-robin | 1 | 10 | 10 |
| 1762905600000 | 30000 | 1762905500000 | round-robin | 1 | 10 | 10 |

### Requirement: Slow start choice in the daemon

When choosing an A or AAAA answer, after the candidates are found and, for `locality` and `fastest`, narrowed to the preferred ones, the daemon SHALL check whether any remaining candidate is ramping. If none is, the choice SHALL be made exactly as without slow start. If at least one is and `balance` is `round-robin`, `weighted`, `locality`, `fastest`, `sticky` or a value the daemon does not recognize, the daemon SHALL choose by smooth weighted round-robin over the candidates' effective weights, keeping its state per service and family and resetting it whenever the list of candidates and their effective weights differs from the previous choice. `failover` SHALL always return the first candidate.

#### Scenario: Ramping backend gets a smaller share

- **WHEN** a `round-robin` service has healthy backends 10.0.0.1 (not ramping) and 10.0.0.2 (step 3), and 13 IPv4 answers are chosen while the step stays 3
- **THEN** 10.0.0.1 is chosen 10 times and 10.0.0.2 3 times

#### Scenario: Failover ignores slow start

- **WHEN** a `failover` service has healthy backends 10.0.0.1 (step 1) and 10.0.0.2 (not ramping) in config order
- **THEN** every IPv4 answer is 10.0.0.1

#### Scenario: Weighted ramping backend

- **WHEN** a `weighted` service has healthy backends 10.0.0.1 (weight 3, step 2) and 10.0.0.2 (weight 1, not ramping), and 16 IPv4 answers are chosen while the step stays 2
- **THEN** 10.0.0.1 is chosen 6 times and 10.0.0.2 10 times
