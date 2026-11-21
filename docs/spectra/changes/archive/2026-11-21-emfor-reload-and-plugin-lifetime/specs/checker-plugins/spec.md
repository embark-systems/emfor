## MODIFIED Requirements

### Requirement: Plugins on reload

On `SIGHUP` reload, a plugin whose `command` is unchanged SHALL keep its running process. A plugin that is new or whose `command` changed SHALL be started before the new config is validated. If the reload is applied, plugins that were removed or replaced SHALL be stopped, and services whose plugin `command` changed SHALL restart their backends from `unknown`. If the reload is rejected, only the plugin processes started for that reload SHALL be stopped, and the previous plugins and checks SHALL keep running unchanged. If a kept plugin is not running when the reload is validated (it is waiting to restart), a service whose plugin and `check_options` are the same as in the applied config SHALL be accepted without a `validate` request, and any other `check_options` for that plugin SHALL be a config error at `services."<service>".check_options` with message `plugin <name> unavailable`. Stopping a plugin SHALL close its stdin and SHALL kill it if it has not exited within 2 seconds. On daemon shutdown every plugin SHALL be stopped. Each plugin SHALL be started in a process group of its own, and whenever a plugin process exits, is stopped or is killed (including when it stopped answering), the daemon SHALL kill every process left in that group. The daemon SHALL also end the tasks that read that process's stdout and stderr and write to its stdin: after the group was killed, it SHALL give the stderr and stdout readers 100 milliseconds to read what is left in the pipes (so the last lines a plugin wrote before it exited are still logged), and SHALL then end all three, even when a process that left the group still holds a pipe open.

#### Scenario: Unchanged plugin survives reload

- **WHEN** a reload changes only a service's `interval` and the service uses plugin `redis`
- **THEN** the `redis` plugin process keeps the same process id

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
