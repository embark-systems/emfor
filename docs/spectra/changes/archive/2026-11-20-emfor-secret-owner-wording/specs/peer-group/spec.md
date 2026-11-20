## MODIFIED Requirements

### Requirement: Peer secret file owner

A `secret_file` or `previous_secret_file` SHALL be owned by root (uid 0) or by the effective user of the process that checks it; otherwise it SHALL be a config error at that key, whose message says the file is owned by uid N, not root or the user running emfor. The check SHALL apply at start, in `emfor check-config` and at reload, like the mode checks, and the process that checks is the daemon at start and at reload and the process of whoever runs `emfor check-config`, so `emfor check-config` compares the owner with the user that runs it, not with the user the daemon runs as. The owner SHALL be that of the file that was opened, so for a link it is the owner of the file the link points to, and the check SHALL come after the file has been opened: a user that cannot read the file gets a cannot-read error, not an owner error. The group of the file SHALL NOT matter for the owner check (the group can still decide whether the file can be read).

#### Scenario: Secret owned by another user

- **WHEN** the daemon runs as uid 998 and the `secret_file` has mode 0640 and is owned by uid 1000, and the daemon user can read it (for example through the group)
- **THEN** the config is rejected with an error at `peers.secret_file` that names uid 1000

#### Scenario: Secret owned by root or by the checking user

- **WHEN** the `secret_file` has mode 0640 and is owned by root, or by the user that does the check (the daemon user at start and at reload), whatever its group, provided the checking user can read it
- **THEN** the config is accepted

#### Scenario: Previous secret owned by another user

- **WHEN** the `previous_secret_file` has mode 0640, the checking user can read it, and it is owned by a user other than root and the user that does the check
- **THEN** the config is rejected with an error at `peers.previous_secret_file` that names the owner's uid

#### Scenario: Link to a file of another owner

- **WHEN** the `secret_file` is a link, owned by root, to a file owned by uid 1000 that is not the user that does the check, and the file has mode 0640 and the checking user can read it
- **THEN** the config is rejected with an error at `peers.secret_file` that names uid 1000

#### Scenario: Check-config run by an account that is not the daemon user

- **WHEN** a user with uid 1001, who is not root, runs `emfor check-config` and the `secret_file` has mode 0640 and is owned by uid 998, the user the daemon runs as, and uid 1001 can read the file (for example it is in the group of the file)
- **THEN** the config is rejected with an error at `peers.secret_file` that names uid 998

#### Scenario: Check-config accepts a secret owned by root for every account

- **WHEN** a user with uid 1001, who is not root, runs `emfor check-config` and the `secret_file` has mode 0640 and is owned by root, and uid 1001 can read the file (for example it is in the group of the file)
- **THEN** the config is accepted

##### Example: Owner and checking user

Every row assumes that the file is a regular file that the checking user can read, has no mode bit for others to read and none for group or others to write (for example 0640), and has at least 32 bytes after one trailing line feed is removed.

| File owner | Checking user | Result |
| ---------- | ------------- | ------ |
| 0 | 998 | accepted |
| 998 | 998 | accepted |
| 1000 | 998 | error naming uid 1000 |
| 0 | 0 | accepted |
| 1000 | 0 | error naming uid 1000 |
