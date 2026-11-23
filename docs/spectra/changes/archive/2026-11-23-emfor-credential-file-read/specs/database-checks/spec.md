## ADDED Requirements

### Requirement: Password and CA files of database checks are bounded regular files

The file named by `password_file` of a database check SHALL be a regular file, after following symbolic links, of at most 64 KiB, and the file named by `ca_file` SHALL be a regular file of at most 8 MiB. The file SHALL be opened without waiting for a writer, and the type and size SHALL be taken from the file that was opened. A path that names a FIFO, a device or a directory, or a file larger than the limit, SHALL be refused: at validation as an option error, and at every check as an `error` outcome whose reason names the option. The check SHALL return without waiting for the file. An error message SHALL NOT contain any of the file's content. A `ca_file` larger than 64 KiB and not larger than 8 MiB SHALL be accepted. The existing rules for these options (absolute path, non-empty, valid UTF-8, no NUL where the protocol needs it, a usable certificate in `ca_file`) are unchanged.

#### Scenario: password_file is a FIFO

- **WHEN** `password_file` of a database check names a FIFO that has no writer
- **THEN** the check returns an `error` outcome with a reason that starts with `password_file: ` and says the file is not a regular file, without blocking a thread

#### Scenario: Large ca_file within the limit

- **WHEN** `ca_file` of a database check names a regular file of 300 KiB that holds a usable certificate
- **THEN** validation accepts it and checks use it as before

#### Scenario: ca_file larger than its limit

- **WHEN** `ca_file` names a regular file larger than 8 MiB
- **THEN** validation reports the option as too large
