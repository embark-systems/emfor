## ADDED Requirements

### Requirement: Status shows drained backends

`emfor status` SHALL mark a backend with `drain` true as drained in the text output and show the state the checks gave (`checked_state`) next to it, for example `drained (checks: healthy)`. The `--json` output SHALL add `drain` (boolean) and `checked_state` to the backend object only for a drained backend, and SHALL keep all existing fields. A service whose backends are all drained has `all_down` true, so the exit status rules for a service with no healthy backend apply to it. The reason of a drained backend is `drained`.

#### Scenario: Text output

- **WHEN** a backend is drained and its checks pass
- **THEN** its line contains `drained` and `checks: healthy`

#### Scenario: JSON output

- **WHEN** `emfor status --json` runs with a drained backend
- **THEN** that backend object has `"drain":true` and `"checked_state":"healthy"`, and other backend objects have neither field
