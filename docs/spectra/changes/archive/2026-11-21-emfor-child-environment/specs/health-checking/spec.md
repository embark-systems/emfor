## MODIFIED Requirements

### Requirement: Exec checker

The `exec` checker SHALL run the command given in `check_options.command` once per check, without a shell, with the backend passed in the environment variables `EMFOR_IP`, `EMFOR_PORT`, and `EMFOR_SERVICE`. The command's environment SHALL contain only: those three variables; the variables `PATH`, `LANG`, `TZ` and every variable whose name starts with `LC_`, with the values they have in the daemon's environment (a variable the daemon does not have is not set); and the variables named in `check_options.env_pass`, with the values they have in the daemon's environment (a name the daemon does not have is not set and is not an error). No other variable of the daemon's environment SHALL reach the command. `check_options.env_pass` SHALL be optional; when present it SHALL be an array of strings (`must be an array of strings`), each a variable name of `[A-Za-z_][A-Za-z0-9_]*` (`invalid variable name "<name>"`), without a name twice (`duplicate variable name "<name>"`) and without the name `EMFOR_IP`, `EMFOR_PORT` or `EMFOR_SERVICE` (`<name> is set by emfor`; any other name, also one that starts with `EMFOR_`, is allowed). The result SHALL follow the exit status: 0 is `up`; 1 or 2 is `down` with the first line of standard output as the reason (or `exit code N` when there is none); any other exit code is `error`; death by a signal is `error`; failure to start the command is `error`. When the check exceeds `timeout`, the command's whole process group SHALL be killed and the result SHALL be `down` with reason `timeout`. `check_options.command` SHALL be required, SHALL be a non-empty array of strings whose first element is an absolute path to an existing executable file, SHALL NOT contain a NUL character in any element (`must not contain NUL`), and `command` and `env_pass` SHALL be the only accepted keys.

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

#### Scenario: Variable of the daemon is not passed

- **GIVEN** the daemon's environment has `SECRET_TOKEN=abc` and `HOME=/root`
- **WHEN** an `exec` check without `env_pass` runs a command that prints its environment
- **THEN** the output has neither `SECRET_TOKEN` nor `HOME`

#### Scenario: Locale, time zone and PATH are passed

- **GIVEN** the daemon's environment has `PATH=/usr/bin:/bin`, `LANG=C.UTF-8`, `LC_ALL=C` and `TZ=UTC`
- **WHEN** an `exec` check runs a command that prints its environment
- **THEN** the output has `PATH=/usr/bin:/bin`, `LANG=C.UTF-8`, `LC_ALL=C` and `TZ=UTC`

#### Scenario: Variable named in env_pass is passed

- **GIVEN** the daemon's environment has `HOME=/root`, and a service sets `check_options.env_pass = ["HOME", "NOT_SET_ANYWHERE"]`
- **WHEN** the `exec` check runs a command that prints its environment
- **THEN** the output has `HOME=/root` and no `NOT_SET_ANYWHERE`, and the config is valid

##### Example: env_pass validation

| `check_options.env_pass` | Result |
| --- | --- |
| `["HOME"]` | ok |
| `[]` | ok |
| `["http_proxy", "HTTPS_PROXY"]` | ok |
| `"HOME"` | error at `check_options.env_pass`: must be an array of strings |
| `["HOME=/root"]` | error at `check_options.env_pass`: invalid variable name "HOME=/root" |
| `["1ABC"]` | error at `check_options.env_pass`: invalid variable name "1ABC" |
| `["HOME", "HOME"]` | error at `check_options.env_pass`: duplicate variable name "HOME" |
| `["EMFOR_IP"]` | error at `check_options.env_pass`: EMFOR_IP is set by emfor |
| `["EMFOR_SERVICE"]` | error at `check_options.env_pass`: EMFOR_SERVICE is set by emfor |
| `["EMFOR_PREFIX"]` | ok |
