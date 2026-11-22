## MODIFIED Requirements

### Requirement: Plugin check results

A plugin answer SHALL become the check result given by its `result` and `reason`; a `down` or `error` answer without `reason` SHALL use reason `no reason given`. An answer with any other `result` value SHALL make the check `error` with reason `invalid plugin response`. A stdout line that is not valid JSON or has an unknown `type` SHALL be logged at `warn` and ignored. An answer whose `id` matches no pending request SHALL be ignored. Each line the plugin writes to stderr SHALL be logged at `info` with the plugin name. Stderr lines and invalid stdout lines SHALL each be rate limited per plugin process: up to 100 lines in a burst, refilled at 10 lines a second. A line over the limit SHALL NOT be logged; the number of such lines SHALL be logged at `warn` as `plugin <name>: suppressed <n> stderr lines` (or `invalid stdout lines`) right before the next line of that stream that is logged, and when the stream ends. (100 lines leave room for a startup message or a stack trace; 10 a second keeps an occasional line per check, while a plugin writing in a loop cannot fill the log.) A plugin built on the `emfor-plugin` crate SHALL send the first 512 characters of a reason, followed by `…` when the reason was longer (so at most 513 characters), so the answer line stays within the 65536-byte line limit whatever text a backend returns; the daemon's reason normalization (spec health-checking) then keeps at most 512 characters, the first 511 followed by `…`. Such a plugin SHALL answer a check request whose target `ip` is not an IP address `error` with reason `invalid target ip "<ip>"` (the text of `ip` in double quotes), and a check whose task ended without an outcome and without a panic (it was cancelled) `error` with reason `check was cancelled`. Such a plugin SHALL answer a check whose code panics `error` with reason `check panicked: <message>` (or `check panicked` when the panic carries no text), and SHALL keep serving other requests.

#### Scenario: Plugin reports down

- **WHEN** a plugin answers a check with `{"type":"check","id":3,"result":"down","reason":"PONG not received"}`
- **THEN** the check result is `down` with reason `PONG not received`

#### Scenario: Plugin stderr is logged

- **WHEN** a plugin writes `connection pool exhausted` to stderr
- **THEN** the daemon log has an `info` line containing the plugin name and `connection pool exhausted`

#### Scenario: Reason from a plugin crate is cut

- **WHEN** a plugin built on the `emfor-plugin` crate answers a check `down` with a reason of 70000 characters
- **THEN** the answer line is shorter than 65536 bytes and its reason is the first 512 characters followed by `…`, 513 characters in all

#### Scenario: Target ip is not an address

- **WHEN** a plugin built on the `emfor-plugin` crate receives a check request whose target `ip` is `nope`
- **THEN** it answers `error` with reason `invalid target ip "nope"`

#### Scenario: Plugin log lines are rate limited

- **WHEN** plugin `p` writes 1000 lines to stderr at once and then exits
- **THEN** the daemon logs the first 100 of them and one `warn` line `plugin p: suppressed 900 stderr lines`

### Requirement: Plugin output line limit

The daemon SHALL read at most 65536 bytes of any single line a plugin writes, on stdout and on stderr, and SHALL NOT buffer more than that for one line. When a stdout line exceeds 65536 bytes before its line feed, the daemon SHALL log at `warn` that plugin `<name>` wrote a line longer than 65536 bytes and stop reading its stdout; the plugin SHALL then be handled as a plugin that closed its stdout (it is stopped, pending requests become `error` with reason `plugin exited`, and it is restarted after the backoff delay). When a stderr line exceeds 65536 bytes, the daemon SHALL read its first 65536 bytes, discard the rest of that line up to its line feed and log the line as it logs any stderr line; the plugin keeps running. The daemon SHALL log at most the first 1000 characters of any stderr line, whatever its length. A stdout line that is not a valid message SHALL be logged (at `warn`, see Plugin check results) with at most its first 200 characters, and a first stdout line that is not a valid `hello` SHALL be quoted in the handshake failure with at most its first 200 characters.

#### Scenario: Oversized answer line

- **WHEN** plugin `p` writes 70000 bytes to stdout without a line feed
- **THEN** the daemon logs that plugin `p` wrote a line longer than 65536 bytes, its pending checks are `error` with reason `plugin exited`, and the plugin is started again after the restart delay

#### Scenario: Oversized stderr line

- **WHEN** plugin `p` writes 70000 bytes to stderr followed by a line feed, then answers a check `up`
- **THEN** the daemon logs one stderr line of plugin `p` holding its first 1000 characters and no more, the plugin keeps running, and the check result is `up`

#### Scenario: Long invalid stdout line

- **WHEN** plugin `p` writes a line of 300 characters that is not JSON to stdout
- **THEN** the daemon logs at `warn` that it is ignoring an invalid line of plugin `p`, quoting its first 200 characters and no more

#### Scenario: Long handshake answer

- **WHEN** the first stdout line of plugin `p` is 300 characters long and is not a valid `hello`
- **THEN** the plugin is refused with an error that quotes the first 200 characters of that line and no more
