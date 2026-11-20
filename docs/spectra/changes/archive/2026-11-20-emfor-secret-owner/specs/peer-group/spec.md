## ADDED Requirements

### Requirement: Peer secret file owner

A `secret_file` or `previous_secret_file` SHALL be owned by root (uid 0) or by the effective user the daemon runs as; otherwise it SHALL be a config error at that key, whose message says the file is owned by uid N, not root or the user running emfor. The owner SHALL be that of the file that was opened, so for a link it is the owner of the file the link points to. The group of the file SHALL NOT matter. The check SHALL apply at start, in `emfor check-config` and at reload, like the mode checks.

#### Scenario: Secret owned by another user

- **WHEN** the daemon runs as uid 998 and the `secret_file` has mode 0640 and is owned by uid 1000
- **THEN** the config is rejected with an error at `peers.secret_file` that names uid 1000

#### Scenario: Secret owned by root or by the daemon user

- **WHEN** the `secret_file` has mode 0640 and is owned by root, or by the user the daemon runs as, whatever its group
- **THEN** the config is accepted

#### Scenario: Previous secret owned by another user

- **WHEN** the `previous_secret_file` is owned by a user other than root and the daemon user
- **THEN** the config is rejected with an error at `peers.previous_secret_file`

#### Scenario: Link to a file of another owner

- **WHEN** the `secret_file` is a link, owned by root, to a file owned by uid 1000 that the daemon does not run as
- **THEN** the config is rejected with an error at `peers.secret_file`

##### Example: Owner and daemon user

| File owner | Daemon user | Result |
| ---------- | ----------- | ------ |
| 0 | 998 | accepted |
| 998 | 998 | accepted |
| 1000 | 998 | error: owned by uid 1000, not root or the user running emfor |
| 0 | 0 | accepted |
| 1000 | 0 | error: owned by uid 1000, not root or the user running emfor |
