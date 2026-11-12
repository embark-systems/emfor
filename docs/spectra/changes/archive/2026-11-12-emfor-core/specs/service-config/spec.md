## Purpose

Defines how operators configure emfor: the TOML file format, built-in defaults with per-service overrides, validation rules, the `--check-config` command, and how a running daemon reloads its config on `SIGHUP` without ever applying an invalid one.

## ADDED Requirements

### Requirement: TOML config with defaults and per-service overrides

The daemon SHALL read its configuration from a TOML file, `/etc/emfor/emfor.toml` by default or the path given with `--config`. Each service under `[services."<name>"]` SHALL use the value of `interval`, `timeout`, `rise`, `fall`, `jitter`, and `on_all_down` from its own table when present, otherwise from `[defaults]`, otherwise from the built-in defaults: `interval = "2s"`, `timeout = "1s"`, `rise = 2`, `fall = 3`, `jitter = "10%"`, `on_all_down = "open"`. Durations SHALL be strings with units (for example `"500ms"`, `"2s"`).

#### Scenario: Service inherits and overrides settings

- **WHEN** `[defaults]` sets `interval = "5s"` and a service sets only `interval = "1s"`
- **THEN** that service uses `interval` 1s and `timeout` 1s, `rise` 2, `fall` 3, `jitter` 10%, `on_all_down` open

##### Example: effective settings resolution

| `[defaults]` | Service table | Effective `interval` | Effective `fall` |
| --- | --- | --- | --- |
| absent | absent | 2s | 3 |
| `interval = "5s"` | absent | 5s | 3 |
| `interval = "5s"`, `fall = 5` | `interval = "1s"` | 1s | 5 |

### Requirement: Service names are normalized hostnames

The daemon SHALL normalize each service name to lowercase and strip one trailing dot. A normalized name SHALL be 1–253 characters made of dot-separated labels, each label 1–63 characters of `[a-z0-9-]` not starting or ending with `-`. Two service names that normalize to the same value SHALL be reported as a duplicate.

#### Scenario: Name normalization

- **WHEN** the config defines a service named `API.svc.emfor.`
- **THEN** the service is known as `api.svc.emfor`

#### Scenario: Duplicate after normalization

- **WHEN** the config defines services `api.svc.emfor` and `API.SVC.EMFOR`
- **THEN** validation fails with a duplicate service error naming `api.svc.emfor`

### Requirement: Config validation rules

The daemon SHALL reject a config that violates any of these rules and SHALL report every violation, each prefixed with its TOML path:

- `interval` at least 100ms; `timeout` greater than zero and not greater than `interval` (equal is allowed, because checks of one backend run one after another and cannot overlap); `rise` at least 1; `fall` at least 1; `jitter` from 0% to 50%; `on_all_down` is `open` or `closed`.
- `backends` is non-empty; every entry is `IPv4:port` or `[IPv6]:port` with port 1–65535; no entry repeats within a service.
- `check` names a registered checker and `check_options` passes that checker's validation.
- No unknown keys anywhere in the file.

#### Scenario: Multiple errors reported together

- **WHEN** a service sets `timeout = "3s"` with `interval = "2s"` and also lists backend `10.0.0.1:70000`
- **THEN** validation reports both the timeout error and the port error

##### Example: validation outcomes

| Setting | Value | Result |
| --- | --- | --- |
| `interval` | `"50ms"` | error: interval below 100ms |
| `interval` / `timeout` | `"1s"` / `"1s"` | ok |
| `interval` / `timeout` | `"1s"` / `"2s"` | error: timeout must not exceed interval |
| `jitter` | `"60%"` | error: jitter above 50% |
| `backends` | `["db.local:5432"]` | error: backend must be an IP literal with port |
| `backends` | `["[2001:db8::1]:8080"]` | ok |
| `chekc` (typo key) | `"tcp"` | error: unknown key |
| `check` | `"redis"` | error: unknown checker `redis` |

### Requirement: Check-config command

`emfor --check-config` SHALL validate the config file without opening the state store or starting checks. On success it SHALL print `config ok: <n> services, <m> backends` and exit with status 0. On failure it SHALL print each validation error on its own line and exit with status 2.

#### Scenario: Valid config

- **WHEN** an operator runs `emfor --check-config --config examples/emfor.toml` on a file with 2 services and 5 backends
- **THEN** the command prints `config ok: 2 services, 5 backends` and exits 0

#### Scenario: Invalid config

- **WHEN** the config contains an unknown key `services."api.svc.emfor".intervall`
- **THEN** the command prints an error containing `services."api.svc.emfor".intervall` and exits 2

### Requirement: Startup with an invalid config

When the daemon starts with a config that fails validation, it SHALL log every validation error and exit with status 2 without opening the state store.

#### Scenario: Daemon refuses an invalid config

- **WHEN** the daemon starts with a config whose `rise` is 0
- **THEN** the daemon exits with status 2 and the store file is not created

### Requirement: Reload on SIGHUP applies only a fully valid config

On `SIGHUP` the daemon SHALL re-read the config file. If the new config passes validation, the daemon SHALL apply it. If any validation error exists, the daemon SHALL log every error at `error` level and continue running the previous config unchanged. A change to any key in the `[store]` table SHALL be a validation error on reload.

#### Scenario: Valid reload adds a service

- **WHEN** a service is added to the config file and the daemon receives `SIGHUP`
- **THEN** the daemon starts checking the new service's backends

#### Scenario: Invalid reload is rejected

- **WHEN** the config file is edited so a service has `timeout = "5s"` and `interval = "2s"`, and the daemon receives `SIGHUP`
- **THEN** the daemon logs the timeout error, keeps checking with the previous settings, and the store is not changed by the reload

#### Scenario: Store settings change requires restart

- **WHEN** `[store] path` is changed in the config file and the daemon receives `SIGHUP`
- **THEN** the reload is rejected with the error "store settings require restart"
