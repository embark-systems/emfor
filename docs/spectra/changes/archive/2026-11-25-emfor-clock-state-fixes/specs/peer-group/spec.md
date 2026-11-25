## MODIFIED Requirements

### Requirement: Clock offset of members

A daemon with `[peers]` SHALL measure, from every message of a member that passes the checks of format, authentication, sender and `site` (including a message that is then dropped because its timestamp is more than 5 seconds from the receiver's clock, but not a replay of a message of that member that was accepted before), the clock offset of that member: the timestamp of the message minus the receiver's clock in milliseconds at its arrival, positive when the member's clock is ahead. For each member the daemon SHALL keep the latest measurement and the time it was made, and SHALL drop the measurement when no such message has arrived for 10 seconds. The guard against a replay needs a message of that member that was accepted before: for a member from which no message has been accepted since the daemon started, and for a member whose messages are all dropped as `stale`, no datagram is known to be a replay, so a recorded datagram that is sent again with the member's source address can be measured. This SHALL affect only the state "clock offset", its log lines, `emfor status` and `emfor_peer_clock_offset_seconds`. Measuring SHALL NOT change which messages are accepted, the liveness of members, the assigned checkers or the votes.

Once per second the daemon SHALL put a member into the state "clock offset" when the absolute value of the offsets of its latest three messages are all at least 2000 milliseconds and have the same sign, and SHALL take it out of that state when the absolute value of the offset of its latest message is below 1500 milliseconds or its measurement is dropped. Between the two limits the state SHALL stay as it is.

The daemon SHALL log at `warn` once when a member enters the state, with the member, the direction (`behind` or `ahead` of this host) and the offset in seconds, and at `info` once when it leaves it. The `info` line SHALL say that the clock of the member is within limits only when the member left because the absolute value of the offset of its latest message fell below 1500 milliseconds; when it left because its measurement was dropped, the line SHALL say that no offset has been measured for 10 seconds and SHALL NOT say that the clock is within limits. These lines are in addition to the log line for dropped messages.

#### Scenario: Member behind by 3 seconds

- **WHEN** three messages of member C arrive whose timestamps are 3000 milliseconds before the receiver's clock
- **THEN** within 1 second C is in the state "clock offset" with offset -3000, and one `warn` line names C, `behind` and 3.0 seconds

#### Scenario: Offset beyond the limit is still measured

- **WHEN** messages of member C arrive whose timestamps are 6000 milliseconds before the receiver's clock and are dropped as `stale`
- **THEN** the dropped messages are counted as `stale` as before, and C is in the state "clock offset" with offset -6000

#### Scenario: One delayed message does not enter the state

- **WHEN** one message of member C arrives with an offset of -2500 milliseconds, between messages with an offset of -20 milliseconds
- **THEN** C is not in the state "clock offset" and nothing is logged

#### Scenario: Hysteresis

- **WHEN** C is in the state and its next message has an offset of -1800 milliseconds
- **THEN** C stays in the state; after a message with an offset of -400 milliseconds C is out of the state and one `info` line names C and says that its clock is within limits

#### Scenario: Ahead of this host

- **WHEN** three messages of member C arrive whose timestamps are 2500 milliseconds after the receiver's clock
- **THEN** C is in the state with offset 2500 and the `warn` line says `ahead`

#### Scenario: The measurement expires

- **WHEN** C is in the state and no message of C passes the checks for 10 seconds
- **THEN** the measurement of C is dropped, C is out of the state and one `info` line names C and says that no offset has been measured for 10 seconds, without saying that the clock is within limits

#### Scenario: A replay is not measured

- **WHEN** member C's messages with sequence numbers 1 to 4 were accepted with offsets of 0, and the datagram with sequence number 1 is sent to the receiver again 20 seconds later
- **THEN** it is dropped as `stale` as before, and the latest offset of C stays 0 and C is not in the state "clock offset"

#### Scenario: Offsets do not change the votes

- **WHEN** C is in the state "clock offset" and sends results that are fresh
- **THEN** the liveness of C, the assigned checkers and the combined results are the same as without the measurement

#### Scenario: Clocks agree

- **WHEN** all members have offsets below 500 milliseconds
- **THEN** no member is in the state and nothing is logged for offsets

<!-- @trace
source: emfor-peer-clock-offset
updated: 2026-11-23
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/metrics.rs
  - crates/emfor-daemon/src/peers/group.rs
  - crates/emfor-daemon/src/peers/stats.rs
  - crates/emfor-daemon/src/peers/task.rs
  - crates/emfor-daemon/src/quorum.rs
  - crates/emfor-daemon/src/status.rs
  - docs/manual.zh_TW/11-peer-groups.md
  - docs/manual.zh_TW/12-operating.md
  - docs/manual.zh_TW/16-troubleshooting.md
  - docs/manual/11-peer-groups.md
  - docs/manual/12-operating.md
  - docs/manual/16-troubleshooting.md
  - tools/peer-clock-test.sh
-->

---

#### Scenario: A replay for a member without an accepted message

- **WHEN** no message of member C has been accepted since the daemon started, and three recorded datagrams of C, signed with the current secret and 4 hours old, arrive with C's source address
- **THEN** each is dropped as `stale`, C is put into the state "clock offset" with an offset of about -14400000, and the votes, the liveness of C and the assigned checkers are as without the datagrams
