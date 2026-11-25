## ADDED Requirements

### Requirement: Clock jump log line

The daemon SHALL compare, at each heartbeat tick, the wall-clock time elapsed since the previous tick with the monotonic time elapsed since the previous tick. When the two differ by more than 2000 milliseconds, the daemon SHALL treat it as a host clock jump and write one `warn` log line with the fields `event` set to `clock_jumped`, `direction` set to `forward` when the wall clock advanced more than the monotonic clock and to `backward` otherwise, and `jump_ms` set to the absolute difference in milliseconds. The message SHALL say that the host clock jumped, in which direction and by how many seconds, and that slewing is preferred to stepping. The first tick after start SHALL only record the baseline and SHALL NOT be compared. The daemon SHALL write at most one `clock_jumped` line per 10 seconds; jumps inside that period SHALL still be counted by the clock jump counter. A tick that arrives late while both clocks advance equally SHALL NOT produce a line. The line SHALL NOT change the heartbeat value written, the store format or the readers' staleness decision.

#### Scenario: Wall clock stepped forward

- **WHEN** the previous tick saw wall time T and monotonic time M, and the next tick sees wall time T + 1000 + 7000 and monotonic time M + 1000
- **THEN** the daemon writes one `warn` line with `event = clock_jumped`, `direction = forward` and `jump_ms = 7000`

#### Scenario: Wall clock stepped backward

- **WHEN** between two ticks the wall clock moved back 4000 milliseconds while the monotonic clock advanced 1000 milliseconds
- **THEN** the daemon writes one `warn` line with `event = clock_jumped`, `direction = backward` and `jump_ms = 5000`

#### Scenario: Late tick without a clock change

- **WHEN** the writer is busy and the next tick arrives 6000 milliseconds after the previous one, and both the wall clock and the monotonic clock advanced 6000 milliseconds
- **THEN** no `clock_jumped` line is written

#### Scenario: Difference at the threshold

- **WHEN** the wall clock and the monotonic clock differ by exactly 2000 milliseconds between two ticks
- **THEN** no line is written, and with a difference of 2001 milliseconds one line is written

#### Scenario: First tick is only a baseline

- **WHEN** the daemon has just started and handles its first heartbeat tick
- **THEN** no comparison is made and no `clock_jumped` line is written, even if the host clock was stepped during start-up

#### Scenario: Repeated jumps are limited

- **WHEN** the clock jumps at tick 20 and again at tick 25 (5 seconds later)
- **THEN** one `clock_jumped` line is written for the first jump, none for the second, and the clock jump counter counts both
