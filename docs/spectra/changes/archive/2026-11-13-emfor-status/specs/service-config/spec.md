## MODIFIED Requirements

### Requirement: TOML config with defaults and per-service overrides

The daemon SHALL read its configuration from a TOML file, the default config path fixed at build time (`/etc/emfor/emfor.toml` unless built with `EMFOR_PREFIX`) or the path given with `--config`. Each service under `[services."<name>"]` SHALL use the value of `interval`, `timeout`, `rise`, `fall`, `jitter`, `on_all_down`, and `balance` from its own table when present, otherwise from `[defaults]`, otherwise from the built-in defaults: `interval = "2s"`, `timeout = "1s"`, `rise = 2`, `fall = 3`, `jitter = "10%"`, `on_all_down = "open"`, `balance = "round-robin"`. A service MAY also set `dns_ttl`, which overrides `[dns] ttl` for that service and has no `[defaults]` entry. Durations SHALL be strings with units (for example `"500ms"`, `"2s"`).

#### Scenario: Service inherits and overrides settings

- **WHEN** `[defaults]` sets `interval = "5s"` and a service sets only `interval = "1s"`
- **THEN** that service uses `interval` 1s and `timeout` 1s, `rise` 2, `fall` 3, `jitter` 10%, `on_all_down` open, `balance` round-robin

##### Example: effective settings resolution

| `[defaults]` | Service table | Effective `interval` | Effective `fall` | Effective `balance` |
| --- | --- | --- | --- | --- |
| absent | absent | 2s | 3 | round-robin |
| `interval = "5s"` | absent | 5s | 3 | round-robin |
| `interval = "5s"`, `fall = 5` | `interval = "1s"` | 1s | 5 | round-robin |
| `balance = "failover"` | absent | 2s | 3 | failover |
| `balance = "failover"` | `balance = "sticky"` | 2s | 3 | sticky |

## ADDED Requirements

### Requirement: Default config path set at build time

The default config path used by `emfor`, `emfor --check-config` and `emfor status` when `--config` is absent SHALL be decided when the daemon is built, from the environment variable `EMFOR_PREFIX`: `/etc/emfor/emfor.toml` when `EMFOR_PREFIX` is unset, empty or `/usr`, and `<EMFOR_PREFIX>/etc/emfor/emfor.toml` otherwise, with one trailing `/` of the prefix removed. A value that is not an absolute path SHALL fail the build with an error naming `EMFOR_PREFIX`. Changing `EMFOR_PREFIX` SHALL cause the next build to use the new path. `emfor --help` SHALL show the default config path built in. The target operating system SHALL NOT change the default.

#### Scenario: Built with a prefix

- **WHEN** emfor is built with `EMFOR_PREFIX=/usr/local` and an operator runs `emfor status` without `--config`
- **THEN** it reads `/usr/local/etc/emfor/emfor.toml`

##### Example: prefix to default path

| `EMFOR_PREFIX` at build | Default config path |
| --- | --- |
| unset | `/etc/emfor/emfor.toml` |
| `/usr` | `/etc/emfor/emfor.toml` |
| `/usr/local` | `/usr/local/etc/emfor/emfor.toml` |
| `/usr/local/` | `/usr/local/etc/emfor/emfor.toml` |
| `/opt/emfor` | `/opt/emfor/etc/emfor/emfor.toml` |
| `usr/local` | build fails, error names `EMFOR_PREFIX` |

#### Scenario: Help shows the built-in default

- **WHEN** emfor is built without `EMFOR_PREFIX` and an operator runs `emfor --help`
- **THEN** the `--config` line shows the default `/etc/emfor/emfor.toml`
