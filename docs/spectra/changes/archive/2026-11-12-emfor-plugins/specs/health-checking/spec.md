## MODIFIED Requirements

### Requirement: Check results are up, down, or error

Every check SHALL produce exactly one result: `up` (the backend passed), `down` with a reason (the backend failed), or `error` with a reason (emfor could not perform the check because of a local problem). A check that does not finish within the service's `timeout` SHALL be recorded as `down` with reason `timeout`, except for checks performed by an external plugin, which SHALL be recorded as `error` with reason `plugin did not answer within timeout`.

#### Scenario: Timeout is a backend failure

- **WHEN** a backend accepts the TCP connection for an `http` check but sends no response within `timeout`
- **THEN** the check result is `down` with reason `timeout`

#### Scenario: Plugin timeout is not a backend failure

- **WHEN** a service uses a plugin that receives the `check` request but never answers, and the service's `timeout` passes
- **THEN** the check result is `error` with reason `plugin did not answer within timeout`

## ADDED Requirements

### Requirement: Exec checker

The `exec` checker SHALL run the command given in `check_options.command` once per check, without a shell, with the backend passed in the environment variables `EMFOR_IP`, `EMFOR_PORT`, and `EMFOR_SERVICE`. The result SHALL follow the exit status: 0 is `up`; 1 or 2 is `down` with the first line of standard output as the reason (or `exit code N` when there is none); any other exit code is `error`; death by a signal is `error`; failure to start the command is `error`. When the check exceeds `timeout`, the command's whole process group SHALL be killed and the result SHALL be `down` with reason `timeout`. `check_options.command` SHALL be required, SHALL be a non-empty array of strings whose first element is an absolute path to an existing executable file, and SHALL be the only accepted key.

#### Scenario: Script reports critical

- **WHEN** an `exec` check runs a script that prints `replication lag 120s` and exits 2
- **THEN** the result is `down` with reason `replication lag 120s`

##### Example: exit status mapping

| Command behavior | Result | Reason |
| --- | --- | --- |
| exits 0 | up | none |
| prints `slow`, exits 1 | down | `slow` |
| prints nothing, exits 2 | down | `exit code 2` |
| prints `unknown state`, exits 3 | error | `exit code 3: unknown state` |
| killed by signal 9 | error | `killed by signal 9` |
| sleeps longer than `timeout` | down | `timeout` |

#### Scenario: Backend passed in the environment

- **WHEN** an `exec` check for service `db.svc.emfor` runs against backend `10.0.1.5:5432`
- **THEN** the command sees `EMFOR_IP=10.0.1.5`, `EMFOR_PORT=5432`, and `EMFOR_SERVICE=db.svc.emfor`

#### Scenario: Timeout kills child processes

- **WHEN** an `exec` command starts a background child and both sleep longer than `timeout`
- **THEN** the result is `down` with reason `timeout` and neither process is still running afterwards

#### Scenario: Invalid command option

- **WHEN** a service sets `check = "exec"` with `check_options.command = ["check_pg"]`
- **THEN** validation fails at `check_options.command` because the first element is not an absolute path to an existing executable
