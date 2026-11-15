## ADDED Requirements

### Requirement: Drain metric

The metrics endpoint SHALL expose `emfor_backend_drain{service,backend}`, a gauge that is 1 for a drained backend and 0 for every other backend. `emfor_backend_state` and `emfor_backend_transitions_total` SHALL keep reporting the state the checks gave, whether or not the backend is drained, and SHALL NOT gain a state label.

#### Scenario: Drained backend

- **WHEN** a backend is drained and its checks pass
- **THEN** `emfor_backend_drain` is 1 for it and `emfor_backend_state{state="healthy"}` is still 1

#### Scenario: Backend without drain

- **WHEN** a backend is not drained
- **THEN** `emfor_backend_drain` is 0 for it
