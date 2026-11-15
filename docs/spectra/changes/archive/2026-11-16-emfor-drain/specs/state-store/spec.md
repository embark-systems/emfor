## ADDED Requirements

### Requirement: Drained backend record

A backend record MAY have `drain` (boolean) and `checked_state` (`"unknown"`, `"healthy"` or `"unhealthy"`). A record of a backend with `drain = true` in the config SHALL be written with `drain` true, `state` `"unhealthy"`, `reason` `"drained"` and `checked_state` set to the state the checks gave. A record of a backend without drain SHALL NOT contain `drain` or `checked_state`, so that a store without drained backends is unchanged. Readers SHALL read a missing `drain` as `false` and a missing `checked_state` as absent. The daemon SHALL keep the checked state and reason in memory while a backend is drained, and when `drain` is removed SHALL write them back at once. `all_down` SHALL NOT count a drained backend as healthy. `meta/schema_version` SHALL stay `2`, because an older reader reads a drained backend as an unhealthy one.

#### Scenario: Drained backend as stored

- **WHEN** a backend that is healthy has `drain = true` in the config
- **THEN** its record contains `"state":"unhealthy"`, `"reason":"drained"`, `"drain":true` and `"checked_state":"healthy"`

#### Scenario: Backend without drain is unchanged

- **WHEN** no backend of a service sets `drain`
- **THEN** no record of that service contains `drain` or `checked_state`

#### Scenario: All backends drained

- **WHEN** every backend of a service is drained
- **THEN** `all_down` is true

#### Scenario: Older reader

- **WHEN** a reader that does not know `drain` reads a drained backend
- **THEN** it reads an unhealthy backend and does not fail
