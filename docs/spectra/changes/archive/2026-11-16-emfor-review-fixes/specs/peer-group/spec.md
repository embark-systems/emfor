## ADDED Requirements

### Requirement: Peer secret file must not be writable

A `secret_file` or `previous_secret_file` that is writable by group or others (mode `& 0o022` not zero) SHALL be a config error at that key, whose message says the file must not be writable by group or other users. A file with mode 0600 or 0640 SHALL be accepted.

#### Scenario: Writable secret

- **WHEN** the `secret_file` has mode 0620, 0602 or 0660
- **THEN** the config is rejected with an error at `peers.secret_file`

### Requirement: Peer failure reason in the log

A backend failure reason received from a peer SHALL be cut to 256 characters and escaped before it is written to the log, so that it cannot add log lines or fill the log.

#### Scenario: Reason with a line break

- **WHEN** an authenticated member reports a backend down with a reason that contains a line break
- **THEN** the log line shows the line break escaped, on one line
