## ADDED Requirements

### Requirement: ca_file of tcp and http checks is a bounded regular file

The file named by `ca_file` of a `tcp` or `http` check, and by the client certificate options a check uses, SHALL be a regular file, after following symbolic links, of at most 8 MiB for `ca_file` and 1 MiB for `tls_cert_file` and `tls_key_file`. The daemon SHALL open it without waiting for a writer, and SHALL take the cache stamp and the content from the file that was opened, not from two lookups of the path. A path that names a FIFO, a device or a directory, or a file larger than the limit, SHALL make the check return `error` with a reason that starts with the option name and says the file is not a regular file or is too large, and SHALL NOT block a thread of the daemon. Such a failure is not cached: when the file is replaced by a valid one, the next check uses it. The error reason SHALL NOT contain any of the file's content. The existing rules for `ca_file` (an absolute path, a usable certificate, re-reading when the file changes) are unchanged.

#### Scenario: ca_file is a FIFO

- **WHEN** the `ca_file` of an `http` check names a FIFO that has no writer
- **THEN** the check returns `error` with a reason that starts with `ca_file: ` and says the file is not a regular file, and the daemon keeps running its other checks

#### Scenario: ca_file replaced after an error

- **WHEN** the `ca_file` of a check was refused as too large and the file is then replaced by a valid CA file within the limit
- **THEN** the next check reads the new file and runs normally

#### Scenario: Large CA bundle is accepted

- **WHEN** `ca_file` names a regular file of 300 KiB that holds usable certificates
- **THEN** the check uses it as before
