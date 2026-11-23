## ADDED Requirements

### Requirement: Silent assigned checkers

A daemon with `[peers]` SHALL keep, for every backend it checks, the assigned checkers other than itself that it chose at its latest own check of that backend, with the check definition digest and the service's `interval` of that check and the time of the check. A record whose check is older than the window below SHALL be dropped.

Once per second the daemon SHALL treat a member M as silent for a backend when all of the following hold: M is among the kept assigned checkers of the backend; M is live now and its kept service tags do not include the service tag of the backend's service; the service has no digest mismatch with M (see "Check definition digest"); the daemon keeps no fresh result of M for the backend that has the digest of the kept record, fresh as defined in "Assigned checkers and check results"; and these conditions have held at every scan from the first scan at which M was assigned and had no fresh result for the backend, for at least the window. The window SHALL be the larger of 10 seconds and 6 times the service's `interval`. A fresh result of M for the backend, M not being live, M announcing the service tag, M no longer being among the kept assigned checkers, a digest mismatch with M, or the record being dropped SHALL end the silence and restart the count.

A silent checker SHALL NOT change the assigned checkers, the votes or the combined result. A member whose check gives `error` sends no result and SHALL be reported like any other silent member.

The daemon SHALL log at `warn` once when the first backend of a service becomes silent for a member, with the member, the service and that backend, and at `info` once when no backend of that service is silent for that member any more.

#### Scenario: Member without the backend becomes silent

- **WHEN** members A, B and C are live, `interval` is 1 second, A and B check backend `10.0.0.21:8080` of `app.svc.emfor`, C does not have that backend, C is among the assigned checkers, and 10 seconds pass
- **THEN** A reports C as silent for `app.svc.emfor` backend `10.0.0.21:8080` and logs one `warn` line naming C, `app.svc.emfor` and the backend

#### Scenario: Not reported before the window

- **WHEN** the same group has run for 9 seconds with C sending no result for the backend
- **THEN** no silent checker is reported

#### Scenario: A result ends the silence

- **WHEN** C is reported silent and then sends a fresh result for the backend
- **THEN** within 2 seconds C is no longer reported and A logs one `info` line naming C and `app.svc.emfor`

#### Scenario: A slower service waits longer

- **WHEN** the service has `interval = "30s"` and C sends no result
- **THEN** C is not reported before 180 seconds without a fresh result

#### Scenario: Member with another digest is not silent

- **WHEN** C sends results whose digest differs from A's for the service
- **THEN** C is reported in `digest_mismatch` and not as silent

#### Scenario: Member that leaves the service out is not silent

- **WHEN** C announces the service tag of `app.svc.emfor` in its heartbeats
- **THEN** C is not among the candidates and is not reported as silent for that service

#### Scenario: All members send results

- **WHEN** A, B and C all list the backend and check it
- **THEN** no silent checker is reported and nothing is logged for it
