## MODIFIED Requirements

### Requirement: Config directory permissions

The daemon SHALL refuse to load the config when the include directory, or any file read from it (for a symbolic link, the file it points to), is writable by group or others (mode `& 0o022` not zero), or is owned by a user other than root and the user the daemon runs as (the effective user). A file SHALL be checked after it has been opened, on the opened file, and read from that same open file. The error SHALL name the path and the reason: the mode, or the owner. The main config file SHALL NOT get a new permission check.

#### Scenario: Group-writable fragment

- **WHEN** `10-a.toml` has mode 0664
- **THEN** loading fails with an error naming `10-a.toml` and its mode

#### Scenario: World-writable directory

- **WHEN** the include directory has mode 0777
- **THEN** loading fails with an error naming the directory

#### Scenario: Main file mode unchanged

- **WHEN** the main file has mode 0664 and `include_dir` is not set
- **THEN** loading is not refused for that reason

#### Scenario: Fragment owned by another user

- **WHEN** `10-a.toml` has mode 0644 and is owned by uid 12345, which is neither root nor the user running emfor
- **THEN** loading fails with an error naming `10-a.toml` and `owned by uid 12345`

#### Scenario: Link to a file that is not trusted

- **WHEN** `10-svc.toml` is a symbolic link to a file with mode 0664, or to a file owned by another user
- **THEN** loading fails with an error naming `10-svc.toml`, because a link is judged by the file it points to

#### Scenario: Directory owned by another user

- **WHEN** the include directory is owned by another user
- **THEN** loading fails with an error naming the directory

## ADDED Requirements

### Requirement: Check-config output errors

`emfor check-config` SHALL write its output with a write that reports errors. When standard output cannot be written (a full disk), it SHALL print `cannot write the output: <error>` to standard error and exit 1. A reader that closes the pipe early (`| head`) SHALL NOT be a failure. It SHALL NOT panic or exit with 101.

#### Scenario: Full disk

- **WHEN** `emfor check-config` runs with standard output on a device that is full
- **THEN** it prints `cannot write the output` to standard error and exits 1

#### Scenario: Closed pipe

- **WHEN** the reader of standard output has gone away
- **THEN** the exit status is the one the check gives (0 for a valid config)
