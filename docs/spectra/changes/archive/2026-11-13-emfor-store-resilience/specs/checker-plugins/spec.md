## ADDED Requirements

### Requirement: Plugin output line limit

The daemon SHALL read at most 65536 bytes of any single line a plugin writes, on stdout and on stderr, and SHALL NOT buffer more than that for one line. When a stdout line exceeds 65536 bytes before its line feed, the daemon SHALL log at `warn` that plugin `<name>` wrote a line longer than 65536 bytes and stop reading its stdout; the plugin SHALL then be handled as a plugin that closed its stdout (it is stopped, pending requests become `error` with reason `plugin exited`, and it is restarted after the backoff delay). When a stderr line exceeds 65536 bytes, the daemon SHALL log the line's first 65536 bytes as it logs any stderr line and discard the rest of that line up to its line feed; the plugin keeps running.

#### Scenario: Oversized answer line

- **WHEN** plugin `p` writes 70000 bytes to stdout without a line feed
- **THEN** the daemon logs that plugin `p` wrote a line longer than 65536 bytes, its pending checks are `error` with reason `plugin exited`, and the plugin is started again after the restart delay

#### Scenario: Oversized stderr line

- **WHEN** plugin `p` writes 70000 bytes to stderr followed by a line feed, then answers a check `up`
- **THEN** the daemon logs one stderr line of plugin `p`, the plugin keeps running, and the check result is `up`
