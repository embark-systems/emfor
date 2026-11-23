## ADDED Requirements

### Requirement: Clock offset metric

When `[peers]` is configured, the body SHALL contain `emfor_peer_clock_offset_seconds{peer="<ip:port>"}` (gauge) for every member other than this daemon that has a clock offset measurement (spec peer-group, "Clock offset of members"), the offset in seconds, negative when the member's clock is behind, whether or not the member is in the state "clock offset". The label value SHALL be escaped as the other label values are. A member without a measurement SHALL have no series. Without `[peers]` the body SHALL contain no series of this name.

#### Scenario: Member behind by three seconds

- **WHEN** a daemon with `[peers]` has a measurement of -3000 milliseconds for member `127.0.0.1:5303`
- **THEN** the body contains `emfor_peer_clock_offset_seconds{peer="127.0.0.1:5303"} -3`

#### Scenario: Small offset is shown too

- **WHEN** the measurement for member `127.0.0.1:5303` is 12 milliseconds
- **THEN** the body contains `emfor_peer_clock_offset_seconds{peer="127.0.0.1:5303"} 0.012`

#### Scenario: Member never heard from

- **WHEN** no message of member `127.0.0.1:5303` has passed the checks
- **THEN** the body has no `emfor_peer_clock_offset_seconds` series for it
