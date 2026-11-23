## ADDED Requirements

### Requirement: Peer secret rotation log line

When the content of `secret_file` changes to a valid secret other than the current one and the daemon makes it the current secret, whether the file was changed in place or replaced by renaming another over it, or `secret_file` was changed by a reload to a path whose content differs from the current secret, the daemon SHALL log one line at `info` level at that moment. The line SHALL carry the field `change` with the value `peers_secret_rotated` and the field `secret_file` with the path the content was read from, and its text SHALL say that messages are signed with the previous secret for 60 seconds more and that it is accepted for 120 seconds. The daemon SHALL NOT log this line when the content is the same as the current secret (a file that was only touched or rewritten with the same content), when the file cannot be read, or when its content is not valid; those cases keep their existing behavior. The line SHALL NOT contain the content of any secret, a hash or fingerprint of it, or its length. The field name and the value of `change` are stable, like those of the configuration audit log. This line is not one of the lines of the configuration audit log: it is logged when the secret changes, not by a reload, and it does not count towards the 50 lines of a reload.

#### Scenario: Secret file replaced with a new secret

- **WHEN** `secret_file` is replaced by renaming a file with a new valid secret over it
- **THEN** the daemon logs one `info` line with `change=peers_secret_rotated` and `secret_file` set to the path of `secret_file`, and the line contains neither the old nor the new secret

#### Scenario: Same content written again

- **WHEN** `secret_file` is rewritten with the content it already has
- **THEN** no line with `change=peers_secret_rotated` is logged

#### Scenario: Invalid content is not logged as a rotation

- **WHEN** `secret_file` is changed to content that is not a valid secret
- **THEN** the `warn` line about the file is logged as before, the daemon keeps the last valid secret, and no line with `change=peers_secret_rotated` is logged

#### Scenario: Reload changes the path to a file with another secret

- **WHEN** a reload changes `secret_file` to a path whose valid content differs from the current secret
- **THEN** the daemon logs one `info` line with `change=peers_secret_rotated` and `secret_file` set to the new path
