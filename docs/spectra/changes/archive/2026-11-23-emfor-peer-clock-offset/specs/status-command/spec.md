## ADDED Requirements

### Requirement: Clock offset line in status text

When the daemon is running and `meta/peers` has the shape defined in spec state-store and holds `clock_offsets`, `emfor status` SHALL print, after the lines `peers: no results from <member> for <service> (<n> backends)` and before the first service line, one line for each entry: `peers: clock of <member> is <n.n> s behind this host (messages more than 5 s apart are dropped)` when `offset_ms` is negative, or `... s ahead of this host (...)` when it is positive, where `<n.n>` is the absolute value of `offset_ms` in seconds with one decimal. The lines SHALL be ordered by member in byte order.

When `clock_offsets` has at least two entries and as many entries as `members` of `meta/peers` (which lists the members other than this daemon), `emfor status` SHALL print instead one line `peers: clock of this host differs from every member by up to <n.n> s; check this host's clock (messages more than 5 s apart are dropped)`, where `<n.n>` is the largest absolute `offset_ms` in seconds with one decimal. A `clock_offsets` that is absent SHALL count as empty and add no line.

#### Scenario: One member behind

- **WHEN** `meta/peers` lists two other members and holds `"clock_offsets":[{"member":"10.0.0.13:5302","offset_ms":-3040}]`
- **THEN** the output contains the line `peers: clock of 10.0.0.13:5302 is 3.0 s behind this host (messages more than 5 s apart are dropped)`

#### Scenario: One member ahead

- **WHEN** `clock_offsets` holds `{"member":"10.0.0.12:5302","offset_ms":2460}`
- **THEN** the output contains `peers: clock of 10.0.0.12:5302 is 2.5 s ahead of this host (messages more than 5 s apart are dropped)`

#### Scenario: Every member differs

- **WHEN** `meta/peers` lists two other members in `members` and `clock_offsets` holds entries of both, with offsets -4100 and -3900
- **THEN** the output contains the single line `peers: clock of this host differs from every member by up to 4.1 s; check this host's clock (messages more than 5 s apart are dropped)` and no line for one member

#### Scenario: Only one other member

- **WHEN** `meta/peers` lists one other member in `members` and `clock_offsets` holds it
- **THEN** the output contains the line for that member and not the line about this host

#### Scenario: Store from a daemon without the field

- **WHEN** `meta/peers` has no `clock_offsets`
- **THEN** no `clock of` line is printed
