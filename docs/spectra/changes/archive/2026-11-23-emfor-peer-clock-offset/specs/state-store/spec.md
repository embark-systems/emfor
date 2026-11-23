## ADDED Requirements

### Requirement: Clock offsets in the peer state

The key `meta/peers` SHALL hold, only while a member is in the state "clock offset", the field `clock_offsets`: an array of `{"member":<member>,"offset_ms":<integer>}` for every such member (spec peer-group, "Clock offset of members"), ordered by member in byte order, where `offset_ms` is the member's timestamp minus this daemon's clock in milliseconds at the latest measurement, negative when the member's clock is behind. The daemon SHALL write the field only when the array is not empty, so that `meta/peers` is unchanged while no member is in the state. A reader SHALL treat an absent field as an empty array.

#### Scenario: Member behind reaches the store

- **WHEN** a daemon in a peer group of three has member `127.0.0.1:5304` in the state with a latest offset of -3000 milliseconds
- **THEN** within 2 seconds `meta/peers` holds `"clock_offsets":[{"member":"127.0.0.1:5304","offset_ms":-3000}]`

#### Scenario: Field absent while the clocks agree

- **WHEN** no member is in the state
- **THEN** `meta/peers` has no `clock_offsets` key
