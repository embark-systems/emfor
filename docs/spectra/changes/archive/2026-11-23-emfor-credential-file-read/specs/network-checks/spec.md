## ADDED Requirements

### Requirement: Credential files of network checks are bounded regular files

The files named by the RADIUS options `secret_file` and `password_file` and the Diameter options `tls_cert_file` and `tls_key_file` SHALL each be a regular file, after following symbolic links, of at most the size limit of the option: 64 KiB for `secret_file` and `password_file`, and 1 MiB for `tls_cert_file` and `tls_key_file`. The file SHALL be opened without waiting for a writer and the type and size SHALL be taken from the file that was opened. A path that names a FIFO, a device or a directory, or a file larger than the limit, SHALL be refused: at validation as an option error that says the file is not a regular file or is too large, and at every check as an `error` outcome whose reason starts with the option name. The check SHALL return without waiting for the file. An error message SHALL NOT contain any of the file's content. The existing rules for these options (absolute path, non-empty, at most 128 bytes for the RADIUS password) are unchanged.

#### Scenario: RADIUS secret_file is a FIFO

- **WHEN** `secret_file` of a RADIUS check names a FIFO that has no writer
- **THEN** the check returns an `error` outcome with a reason that starts with `secret_file: ` and says the file is not a regular file, within the check timeout and without blocking a thread

#### Scenario: Credential file larger than its limit

- **WHEN** `password_file` of a RADIUS check names a regular file of 65537 bytes
- **THEN** validation reports the option as too large, and a check that reads it returns an `error` outcome with a reason that starts with `password_file: `

#### Scenario: File of exactly the limit or a link to a regular file

- **WHEN** `secret_file` names a regular file of exactly 65536 bytes, or a symbolic link to such a file
- **THEN** the file is read and used as before

#### Scenario: Diameter key file is a directory

- **WHEN** `tls_key_file` of a Diameter check names a directory
- **THEN** validation reports `tls_key_file` as not a regular file
