## ADDED Requirements

### Requirement: Silent checker metric

When `[peers]` is configured, the body SHALL contain `emfor_peer_silent_checkers` (gauge), the number of (service, backend, member) triples that are currently silent checkers (spec peer-group, "Silent assigned checkers"), 0 when there are none.

#### Scenario: One silent member

- **WHEN** a daemon with `[peers]` reports one member silent for one backend
- **THEN** the body contains `emfor_peer_silent_checkers 1`

#### Scenario: Nobody is silent

- **WHEN** a daemon with `[peers]` reports no silent member
- **THEN** the body contains `emfor_peer_silent_checkers 0`
