## ADDED Requirements

### Requirement: Silent checker line in status text

When the daemon is running and `meta/peers` has the shape defined in spec state-store and holds `silent_checkers`, `emfor status` SHALL print, after the lines `peers: check definition of <service> differs at <members>` and before the first service line, one line for each pair of a shown service and a member that has entries in `silent_checkers` for backends of that service: `peers: no results from <member> for <service> (<n> backends)`, where `<n>` is the number of those entries. The lines SHALL be ordered by service name and then by member in byte order. A `silent_checkers` that is absent SHALL count as empty and add no line.

#### Scenario: One member without results

- **WHEN** `meta/peers` holds `"silent_checkers":[{"service":"app.svc.emfor","backend":"10.0.0.21:8080","member":"10.0.0.13:5302"}]` and `app.svc.emfor` is shown
- **THEN** the output contains the line `peers: no results from 10.0.0.13:5302 for app.svc.emfor (1 backends)` after the `peers:` line

#### Scenario: Silent member of a service that is not shown

- **WHEN** `emfor status --service web.svc.emfor` is run and `silent_checkers` names only `app.svc.emfor`
- **THEN** no `no results from` line is printed

#### Scenario: Store from a daemon without the field

- **WHEN** `meta/peers` has no `silent_checkers`
- **THEN** no `no results from` line is printed
