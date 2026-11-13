## MODIFIED Requirements

### Requirement: Plugin validation of check options

When a config is loaded at startup, on `SIGHUP` reload, or by `emfor check-config`, the daemon SHALL start the declared plugins and send each service's `check_options` to its plugin in a `validate` request before applying the config. Every error the plugin returns SHALL be a config error at `services."<service>".check_options.<key>`. A plugin that fails to start or to complete the handshake SHALL be a config error at `plugins."<name>".command`. A plugin that does not answer a `validate` request within 5 seconds SHALL be a config error at `services."<service>".check_options`, and every later `validate` request to that plugin in the same load SHALL fail with the same error without waiting. These errors SHALL be reported together with all other validation errors. `emfor check-config` SHALL stop every plugin it started before exiting.

#### Scenario: Plugin rejects an option

- **WHEN** a plugin answers `validate` for service `cache.svc.emfor` with `{"errors":[{"key":"password_file","message":"file not found"}]}`
- **THEN** config validation fails with `services."cache.svc.emfor".check_options.password_file: file not found`

#### Scenario: Check config with a broken plugin

- **WHEN** `emfor check-config` runs with a config whose plugin exits before answering the handshake
- **THEN** it prints an error at `plugins."<name>".command`, exits with status 2, and leaves no plugin process running
