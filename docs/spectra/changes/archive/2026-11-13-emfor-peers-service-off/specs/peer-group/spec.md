## ADDED Requirements

### Requirement: Services left out of the peer group

A service whose effective `peers` setting (spec service-config) is `false` SHALL take no part in the peer group on this daemon, while the daemon and its other services stay members. For such a service the daemon SHALL:

- not send `down` or `result` messages for any of its backends;
- check every backend at every check time and apply its own result with its reason unchanged, without counting votes, deciding by votes alone, or marking a disagreement;
- ignore every accepted `down` or `result` message for the service as one for a service it does not have: start no fast recheck run, keep no result, and record no digest mismatch;
- not count check times of the service in `emfor_peer_checks_skipped_total`.

When a reload turns `peers` of a service from `true` to `false`, the daemon SHALL drop the kept results, disagreements and digest mismatches of that service, and a fast recheck run already in progress SHALL run to its end. When a reload turns it back to `true`, the service SHALL take part again from its next check time, starting with no kept results. Other members SHALL NOT be told which services this daemon leaves out: they keep counting this daemon as a candidate when choosing assigned checkers. Without a `[peers]` table the setting SHALL have no effect.

#### Scenario: A service left out on one member

- **WHEN** a group has three live members with the same check definition for `api.svc.emfor`, `api.svc.emfor` has `peers = false` on member A only, and the other two members' own checks of a backend give `down` with reason `refused` while A's give `up`
- **THEN** the backend stays `healthy` on A with no disagreement, A sends no `result` or `down` message for `api.svc.emfor`, and the other two members mark it `unhealthy`

#### Scenario: Down message for a service left out

- **WHEN** a member sends a `down` message for a `healthy` backend of a service that has `peers = false` on this daemon
- **THEN** this daemon starts no fast recheck run and checks the backend at its next `interval`

#### Scenario: Leaving the group on reload drops kept state

- **WHEN** a backend of `api.svc.emfor` is a disagreement on this daemon and a reload sets `peers = false` for `api.svc.emfor`
- **THEN** within 2 seconds the disagreement is no longer in `meta/peers`, and the next applied result of the backend is this daemon's own result

##### Example: what this daemon does for one backend

This daemon is not an assigned checker of the backend; the assigned checkers' fresh votes are `down` `refused`, `down` `refused`, `up`; this daemon makes its own check at this check time and it gives `up`.

| `peers` of the service | Votes counted | Applied result | `result` message sent | Disagreement |
| --- | --- | --- | --- | --- |
| `true` | yes | down `peers 2/3: refused` | no | yes |
| `false` | no | up | no | no |
