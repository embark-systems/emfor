## MODIFIED Requirements

### Requirement: Config directory permissions

The daemon SHALL refuse to load the config when the main config file, the include directory, or any file read from the include directory (for a symbolic link, the file it points to), is writable by group or others (mode `& 0o022` not zero), or is owned by a user other than root and the user the daemon runs as (the effective user). A file SHALL be checked after it has been opened, on the opened file, and read from that same open file. The error SHALL name the path and the reason, the mode or the owner, and the command that fixes it (`chmod g-w,o-w <path>` for a mode, `chown root <path>` for an owner). The main config file SHALL be checked by `emfor server`, at start and at every reload, and by `emfor check-config`. `emfor status` and `emfor lookup`, which read only `[store]` of the main file, SHALL NOT check it. A refusal at start SHALL exit with the config error status without opening the store; a refusal at a reload SHALL keep the previous config.

#### Scenario: Group-writable fragment

- **WHEN** `10-a.toml` has mode 0664
- **THEN** loading fails with an error naming `10-a.toml` and its mode

#### Scenario: World-writable directory

- **WHEN** the include directory has mode 0777
- **THEN** loading fails with an error naming the directory

#### Scenario: Main file that others can write

- **WHEN** the main file has mode 0664 and `emfor check-config` reads it
- **THEN** it fails with an error naming the main file, its mode 0664 and the command `chmod g-w,o-w` with the path, and exits with status 2

##### Example: Main file modes

| Mode | Loading |
| ---- | ------- |
| 0600 | accepted |
| 0640 | accepted |
| 0644 | accepted |
| 0604 | accepted |
| 0620 | refused: group can write |
| 0602 | refused: others can write |
| 0664 | refused |
| 0666 | refused |

#### Scenario: Main file owned by another user

- **WHEN** the main file has mode 0644 and is owned by uid 12345, which is neither root nor the user running emfor
- **THEN** loading fails with an error naming the main file and `owned by uid 12345`, and the command `chown root` with the path

#### Scenario: Link to a main file that is not trusted

- **WHEN** the main file given to the daemon is a symbolic link to a file with mode 0664
- **THEN** loading fails with an error naming the path and the mode 0664, because a link is judged by the file it points to

#### Scenario: Start with a main file that others can write

- **WHEN** the daemon starts and the main file has mode 0664
- **THEN** the error is logged, the process exits with status 2, and the store is not opened

#### Scenario: Reload with a main file that others can write

- **WHEN** the daemon is running and the main file is changed to mode 0664 and the daemon receives SIGHUP
- **THEN** the error is logged, the reload is rejected, and the daemon keeps running with the previous config

#### Scenario: status and lookup do not check the main file

- **WHEN** the main file has mode 0664 and `emfor status` or `emfor lookup` is run
- **THEN** it reads the `[store]` table and works as it did before

#### Scenario: Fragment owned by another user

- **WHEN** `10-a.toml` has mode 0644 and is owned by uid 12345, which is neither root nor the user running emfor
- **THEN** loading fails with an error naming `10-a.toml` and `owned by uid 12345`

#### Scenario: Link to a file that is not trusted

- **WHEN** `10-svc.toml` is a symbolic link to a file with mode 0664, or to a file owned by another user
- **THEN** loading fails with an error naming `10-svc.toml`, because a link is judged by the file it points to

#### Scenario: Directory owned by another user

- **WHEN** the include directory is owned by another user
- **THEN** loading fails with an error naming the directory
