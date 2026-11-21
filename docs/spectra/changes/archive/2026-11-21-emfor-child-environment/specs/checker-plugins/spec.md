## MODIFIED Requirements

### Requirement: Plugin registration in config

A plugin SHALL be declared as `[plugins."<name>"]` with a required `command`: a non-empty array of strings whose first element is an absolute path to an existing executable file and no element of which contains a NUL character (`must not contain NUL`). `[plugins."<name>"]` SHALL accept the optional key `env_pass`, an array of strings naming the variables of the daemon's environment that the plugin process also gets (see "Plugin environment"), each a variable name of `[A-Za-z_][A-Za-z0-9_]*` (`invalid variable name "<name>"`) and none twice (`duplicate variable name "<name>"`); an `env_pass` that is not an array of strings SHALL be an error `must be an array of strings`. `[plugins."<name>"]` SHALL accept no other keys. Plugin names SHALL be 1 to 63 characters of `a-z`, `0-9`, `_`, `-` and SHALL NOT be `tcp`, `http`, or `exec`. A service SHALL use a plugin by setting `check` to the plugin's name. Every declared plugin SHALL be started when a config is applied, whether or not a service uses it.

#### Scenario: Service uses a plugin

- **WHEN** the config declares `[plugins."redis"]` with a valid `command` and a service sets `check = "redis"`
- **THEN** that service's backends are checked by the `redis` plugin

##### Example: plugin declaration validation

| Declaration | Result |
| --- | --- |
| `[plugins."redis"]` `command = ["/usr/local/libexec/emfor-redis"]` (file exists, executable) | ok |
| `[plugins."redis"]` without `command` | error at `plugins.redis.command`: missing required key |
| `[plugins."redis"]` `command = []` | error at `plugins.redis.command`: must not be empty |
| `[plugins."redis"]` `command = ["emfor-redis"]` | error at `plugins.redis.command`: not an absolute path |
| `[plugins."http"]` with a valid `command` | error at `plugins.http`: name is reserved for a built-in checker |
| `[plugins."Redis"]` with a valid `command` | error at `plugins.Redis`: invalid plugin name |
| `[plugins."redis"]` with `args = ["-v"]` | error at `plugins.redis.args`: unknown key |
| `[plugins."redis"]` with a valid `command` and `env_pass = ["http_proxy"]` | ok |
| `[plugins."redis"]` with a valid `command` and `env_pass = ["A B"]` | error at `plugins.redis.env_pass`: invalid variable name "A B" |
| `[plugins."redis"]` with a valid `command` and `env_pass = ["HOME", "HOME"]` | error at `plugins.redis.env_pass`: duplicate variable name "HOME" |
| `[plugins."redis"]` with a valid `command` and `env_pass = "HOME"` | error at `plugins.redis.env_pass`: must be an array of strings |

### Requirement: Plugins on reload

On `SIGHUP` reload, a plugin whose `command` and `env_pass` are unchanged SHALL keep its running process. A plugin that is new or whose `command` or `env_pass` changed SHALL be started before the new config is validated. If the reload is applied, plugins that were removed or replaced SHALL be stopped, and services whose plugin `command` or `env_pass` changed SHALL restart their backends from `unknown`. If the reload is rejected, only the plugin processes started for that reload SHALL be stopped, and the previous plugins and checks SHALL keep running unchanged. If a kept plugin is not running when the reload is validated (it is waiting to restart), a service whose plugin and `check_options` are the same as in the applied config SHALL be accepted without a `validate` request, and any other `check_options` for that plugin SHALL be a config error at `services."<service>".check_options` with message `plugin <name> unavailable`. Stopping a plugin SHALL close its stdin and SHALL kill it if it has not exited within 2 seconds. On daemon shutdown every plugin SHALL be stopped. Each plugin SHALL be started in a process group of its own, and whenever a plugin process exits, is stopped or is killed (including when it stopped answering), the daemon SHALL kill every process left in that group. The daemon SHALL also end the tasks that read that process's stdout and stderr and write to its stdin: after the group was killed, it SHALL give the stderr and stdout readers 100 milliseconds to read what is left in the pipes (so the last lines a plugin wrote before it exited are still logged), and SHALL then end all three, even when a process that left the group still holds a pipe open.

#### Scenario: Unchanged plugin survives reload

- **WHEN** a reload changes only a service's `interval` and the service uses plugin `redis`
- **THEN** the `redis` plugin process keeps the same process id

#### Scenario: Changed env_pass restarts the plugin

- **WHEN** a reload changes only `[plugins."redis"] env_pass` from `[]` to `["HOME"]`
- **THEN** the `redis` plugin is started again with the new environment, and services using it restart their backends from `unknown`

#### Scenario: Processes a plugin started are stopped with it

- **WHEN** plugin `p` started a background process and is then stopped, crashes, or is killed because it stopped answering
- **THEN** that background process is killed too

#### Scenario: Reload while a plugin is restarting

- **WHEN** plugin `redis` has exited and is waiting to restart, and a reload changes only the `interval` of a service using `redis`
- **THEN** the reload is applied without a `validate` request to `redis`

#### Scenario: Rejected reload with a new plugin

- **WHEN** a reload adds plugin `mysql` and also contains an invalid `timeout`
- **THEN** the reload is rejected, the `mysql` process started for it is stopped, and the previous config keeps running

#### Scenario: Last stderr lines of an exiting plugin are logged

- **WHEN** plugin `p` writes `fatal: out of memory` to stderr and exits at once
- **THEN** the daemon logs `plugin p: fatal: out of memory` before it restarts the plugin

#### Scenario: Pipe held by a process outside the group

- **WHEN** plugin `p` started a background process that left the plugin's process group and keeps the plugin's stderr open, and `p` is restarted after it exited
- **THEN** the tasks that served the old process end within 1 second of the exit, and the number of live tasks does not grow with each restart

## ADDED Requirements

### Requirement: Plugin environment

A plugin process SHALL be started with an environment that contains only: the variables `PATH`, `LANG`, `TZ` and every variable whose name starts with `LC_`, with the values they have in the daemon's environment (a variable the daemon does not have is not set); and the variables named in the plugin's `env_pass`, with the values they have in the daemon's environment (a name the daemon does not have is not set and is not an error). No other variable of the daemon's environment SHALL reach the plugin, and emfor SHALL NOT set `EMFOR_IP`, `EMFOR_PORT` or `EMFOR_SERVICE` for it. The same environment SHALL be used each time the plugin is restarted after it exited.

#### Scenario: Variable of the daemon is not passed to a plugin

- **GIVEN** the daemon's environment has `SECRET_TOKEN=abc` and `HOME=/root`
- **WHEN** a plugin without `env_pass` starts and prints its environment to stderr
- **THEN** the output has neither `SECRET_TOKEN` nor `HOME`

#### Scenario: Locale, time zone and PATH are passed to a plugin

- **GIVEN** the daemon's environment has `PATH=/usr/bin:/bin`, `LANG=C.UTF-8`, `LC_ALL=C` and `TZ=UTC`
- **WHEN** a plugin starts and prints its environment to stderr
- **THEN** the output has `PATH=/usr/bin:/bin`, `LANG=C.UTF-8`, `LC_ALL=C` and `TZ=UTC`

#### Scenario: Variable named in env_pass reaches a plugin, also after a restart

- **GIVEN** the daemon's environment has `http_proxy=http://proxy:3128`, and `[plugins."redis"] env_pass = ["http_proxy"]`
- **WHEN** the plugin starts, exits and is restarted
- **THEN** both processes see `http_proxy=http://proxy:3128`
