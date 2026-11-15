## ADDED Requirements

### Requirement: Draining a backend

A backend given as a table in `backends` MAY set `drain`, a boolean that defaults to `false`: `{ addr = "10.0.0.11:8080", drain = true }`. A `drain` value that is not a boolean SHALL be a validation error whose path is that backend's `drain` key and whose message is `drain must be true or false`. A reload that sets, changes or removes `drain` SHALL be applied and SHALL NOT restart the backend's check or reset its state, latency or recovery time. A backend with `drain = true` SHALL keep being checked. `drain` SHALL NOT be part of the service definition digest, so hosts of one peer group MAY drain different backends. A service in which every backend is drained SHALL be valid.

#### Scenario: Drain value is validated

- **WHEN** a backend table sets `drain = "yes"`
- **THEN** the config is rejected with the error `services."api.svc.emfor".backends[1].drain: drain must be true or false`

#### Scenario: Reload applies drain without resetting state

- **WHEN** a healthy backend is changed to `drain = true` and the daemon is reloaded, and later changed back to `drain = false` and reloaded
- **THEN** its check keeps running, and after the second reload it is selected again at once with its recovery time unchanged and without slow start

#### Scenario: Drain does not change the digest

- **WHEN** two hosts have the same service and one host sets `drain = true` on a backend
- **THEN** both hosts compute the same service definition digest
