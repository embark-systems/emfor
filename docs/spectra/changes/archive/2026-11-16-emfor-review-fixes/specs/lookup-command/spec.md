## ADDED Requirements

### Requirement: Lookup output errors

`emfor lookup` SHALL write its output with a write that reports errors. When standard output cannot be written (a full disk), it SHALL print `cannot write the output: <error>` to standard error and exit 3. A reader that closes the pipe early SHALL NOT be a failure: the exit status is the one the lookup gives. It SHALL NOT panic or exit with 101.

#### Scenario: Full disk

- **WHEN** `emfor lookup` runs with standard output on a device that is full
- **THEN** it prints `cannot write the output` to standard error and exits 3
