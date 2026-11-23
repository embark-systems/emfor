## ADDED Requirements

### Requirement: Log format

The `[log]` table SHALL accept the key `format`, a string that is `"text"` or `"json"`, with `"text"` when it is not set. A `format` that is not a string, or is any other string, SHALL be a config error at `log.format` with the message `must be one of text, json`. With `"text"` the daemon SHALL log as it did before the key existed, except that the state change line of "Logging of state changes" has the extra field `event`. With `"json"` the daemon SHALL write each log line from the moment it has read the config as one JSON object on one line to standard error, with the keys `timestamp` (RFC 3339), `level`, `target` and `message`, and every field of the event as a key of the same object, not nested; lines written before the config is read, such as plugin start lines and config errors at start, remain text. `RUST_LOG` SHALL override only the level, never `format`. A reload whose config has a `format` other than the running one SHALL be rejected as one that changes a setting that requires restart, with an error at `log.format` and the message `log format settings require restart`, keeping the previous config; a reload that changes only `level` SHALL still be applied. No log line SHALL use `timestamp`, `level` or `target` as the name of a field.

#### Scenario: Format not set

- **WHEN** the config has no `format` in `[log]`
- **THEN** the daemon logs text lines, and `emfor check-config` exits 0

#### Scenario: JSON lines

- **WHEN** the daemon runs with `format = "json"` and a backend changes from `healthy` to `unhealthy` with reason `connection refused`
- **THEN** the line is one JSON object with `level` `INFO`, a `timestamp`, `event` `backend_state_changed`, `service`, `backend`, `from` `healthy`, `to` `unhealthy` and `reason` `connection refused` as top-level keys

#### Scenario: Reload changes the format

- **WHEN** the daemon runs with `format = "text"` and a reload has `format = "json"`
- **THEN** the reload is rejected with an error at `log.format` saying `log format settings require restart`, the daemon keeps running with the previous config, and its lines stay text

#### Scenario: Reload changes only the level

- **WHEN** a reload changes `level` from `info` to `debug` and leaves `format` as it is
- **THEN** the reload is applied and the new level is in effect

##### Example: format validation

| `[log]` | Result |
| --- | --- |
| no `format` | valid, text |
| `format = "text"` | valid, text |
| `format = "json"` | valid, JSON |
| `format = "yaml"` | error at `log.format`: `must be one of text, json` |
| `format = true` | error at `log.format`: `must be one of text, json` |
