## MODIFIED Requirements

### Requirement: Logging of state changes

Every health state change SHALL be logged at `info` level with the service name, backend id, old state, new state, and the reason from the result that caused it. The line SHALL carry these as the fields `service`, `backend`, `from`, `to` and `reason`, and the field `event` with the value `backend_state_changed`; the names of these fields and the value of `event` are stable, the wording of the line is not. A state change caused by the votes of the peer group SHALL be logged the same way, with the reason of the result the votes produced.

#### Scenario: Transition is logged

- **WHEN** backend `10.0.0.11:8080` of `api.svc.emfor` changes from `healthy` to `unhealthy` after `down` results with reason `connection refused`
- **THEN** an `info` log line has `event` `backend_state_changed`, `service` `api.svc.emfor`, `backend` `10.0.0.11:8080`, `from` `healthy`, `to` `unhealthy`, and `reason` `connection refused`

#### Scenario: Transition caused by peers

- **WHEN** a backend that passes its own check changes to `unhealthy` because two of its three assigned checkers vote it down
- **THEN** the same line is logged, with a `reason` that starts with `peers `
