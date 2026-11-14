## ADDED Requirements

### Requirement: Slow start setting

A service SHALL accept `slow_start`, a duration, in its own table or in `[defaults]`, with the service's value taking precedence and a built-in default of `"0s"`. `slow_start` SHALL be `0s` (slow start off) or from 1s to 1h; any other value SHALL be a validation error prefixed with its TOML path, reported together with every other violation as in "Config validation rules". `slow_start` SHALL be accepted under every `balance` value. A change of `slow_start` SHALL take effect on reload, including for backends whose slow start is in progress.

#### Scenario: Default and override

- **WHEN** `[defaults]` sets `slow_start = "30s"`, service `a.svc.emfor` sets nothing and service `b.svc.emfor` sets `slow_start = "0s"`
- **THEN** `a.svc.emfor` has slow start 30s and `b.svc.emfor` has slow start off

#### Scenario: Out of range

- **WHEN** service `api.svc.emfor` sets `slow_start = "2h"`
- **THEN** check-config fails with an error starting `services."api.svc.emfor".slow_start:`

##### Example: slow_start validation

| `slow_start` | Result |
| --- | --- |
| absent | ok, off |
| `"0s"` | ok, off |
| `"500ms"` | error |
| `"1s"` | ok |
| `"30s"` | ok |
| `"1h"` | ok |
| `"2h"` | error |
| `30` | error: not a duration string |
