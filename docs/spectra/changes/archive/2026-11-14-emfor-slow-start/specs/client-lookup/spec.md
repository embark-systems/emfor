## ADDED Requirements

### Requirement: Slow start in lookups

When the store is fresh, after the candidates are found and, for `locality` and `fastest`, narrowed to the preferred ones, the client SHALL compute each remaining candidate's step and effective weight as defined by the backend-selection requirement "Slow start effective weight", using the service's `slow_start_ms`, the backend's `recovered_at` and the client's current Unix time in milliseconds. If no candidate is ramping, the lookup SHALL choose exactly as without slow start. If at least one is and `balance` is `round-robin`, `weighted`, `locality`, `fastest`, `sticky` without an affinity key, or a value the client does not know, the client SHALL choose the first candidate, in order, whose cumulative effective weight exceeds the round-robin counter modulo the sum of the effective weights. `failover` and `sticky` with a non-empty affinity key SHALL choose as without slow start. When the store is stale, no candidate SHALL be ramping.

#### Scenario: Ramping backend gets a smaller share

- **WHEN** a fresh `round-robin` service has healthy `10.0.0.11:8080` (not ramping) and `10.0.0.12:8080` (`recovered_at` 25 seconds ago, `slow_start_ms` 100000, so step 3), and one process looks it up 13 times
- **THEN** `10.0.0.11:8080` is returned 10 times and `10.0.0.12:8080` 3 times

#### Scenario: Weighted ramping backend

- **WHEN** a fresh `weighted` service has healthy `10.0.0.11:8080` (weight 3, step 3) and `10.0.0.12:8080` (weight 1, not ramping), and one process looks it up 19 times
- **THEN** `10.0.0.11:8080` is returned 9 times and `10.0.0.12:8080` 10 times

#### Scenario: Slow start over

- **WHEN** a fresh `round-robin` service has healthy `10.0.0.11:8080` and `10.0.0.12:8080` whose `recovered_at` is 200 seconds ago with `slow_start_ms` 100000, and one process looks it up 4 times
- **THEN** each backend is returned twice

#### Scenario: Failover and sticky with a key ignore slow start

- **WHEN** a fresh `failover` service lists healthy `10.0.0.11:8080` (step 3) then `10.0.0.12:8080`
- **THEN** every lookup returns `10.0.0.11:8080`

#### Scenario: Stale store does not ramp

- **WHEN** a stale `round-robin` service with `on_all_down = "open"` has healthy `10.0.0.11:8080` (not ramping) and `10.0.0.12:8080` (step 3 by its `recovered_at`), and one process looks it up 4 times
- **THEN** each backend is returned twice
