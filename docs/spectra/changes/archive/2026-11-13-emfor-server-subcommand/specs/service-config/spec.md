## ADDED Requirements

### Requirement: Command line subcommands

The `emfor` executable SHALL take one of the subcommands `server`, `check-config` and `status`. `emfor server` SHALL run the daemon. `emfor check-config` SHALL check the config file (see "Check-config command"). `emfor status` SHALL show the published state (see the status-command capability). `--config <path>` SHALL be a global option accepted before or after any of the three subcommands with the same meaning. Without a subcommand, `emfor` SHALL print its usage, listing the three subcommands, to standard error and exit with status 2 without reading the config file, opening or creating the state store, or starting plugins; this SHALL hold also when `--config` is given. `emfor --help` SHALL print the usage to standard output and exit 0. The option `--check-config` SHALL NOT check anything: wherever it appears, `emfor` SHALL print `--check-config was replaced by "emfor check-config"` to standard error and exit with status 2, and `--help` SHALL NOT list it.

#### Scenario: No subcommand

- **WHEN** an operator runs `emfor` on a host where an emfor daemon is already running
- **THEN** it prints a usage listing `server`, `check-config` and `status` to standard error, exits 2, and the running daemon is not affected

##### Example: invocations without a usable subcommand

| Command | Standard error contains | Exit status |
| --- | --- | --- |
| `emfor` | `server`, `check-config`, `status` | 2 |
| `emfor --config /tmp/t/emfor.toml` | `server`, `check-config`, `status` | 2 |
| `emfor --check-config` | `--check-config was replaced by "emfor check-config"` | 2 |
| `emfor --check-config --config /tmp/t/emfor.toml` | `--check-config was replaced by "emfor check-config"` | 2 |
| `emfor server --check-config` | `--check-config was replaced by "emfor check-config"` | 2 |

#### Scenario: No subcommand leaves the store alone

- **WHEN** an operator runs `emfor --config <config>` whose `[store] path` is `/tmp/t/state.mdb` and that file does not exist
- **THEN** it exits 2 and neither `/tmp/t/state.mdb` nor `/tmp/t/state.mdb-lock` nor `/tmp/t/state.mdb.writer` exists afterwards

#### Scenario: Running the daemon

- **WHEN** an operator runs `emfor server --config <config>` with a valid config
- **THEN** the daemon starts, publishes its services to the state store, and on `SIGTERM` exits 0

#### Scenario: Global config option position

- **WHEN** an operator runs `emfor --config examples/emfor.toml check-config` and, separately, `emfor check-config --config examples/emfor.toml`
- **THEN** both print the same `config ok: ...` line and exit 0

## MODIFIED Requirements

### Requirement: Check-config command

`emfor check-config` SHALL validate the config file without opening the state store or starting checks. On success it SHALL print `config ok: <n> services, <m> backends` and exit with status 0. On failure it SHALL print each validation error on its own line and exit with status 2.

#### Scenario: Valid config

- **WHEN** an operator runs `emfor check-config --config emfor.toml` where `emfor.toml` has 2 services and 5 backends
- **THEN** the command prints `config ok: 2 services, 5 backends` and exits 0

#### Scenario: Invalid config

- **WHEN** the config contains an unknown key `services."api.svc.emfor".intervall`
- **THEN** the command prints an error containing `services."api.svc.emfor".intervall` and exits 2

### Requirement: Default config path set at build time

The default config path used by `emfor server`, `emfor check-config` and `emfor status` when `--config` is absent SHALL be decided when the daemon is built, from the environment variable `EMFOR_PREFIX`: `/etc/emfor/emfor.toml` when `EMFOR_PREFIX` is unset, empty or `/usr`, and `<EMFOR_PREFIX>/etc/emfor/emfor.toml` otherwise, with one trailing `/` of the prefix removed. A value that is not an absolute path SHALL fail the build with an error naming `EMFOR_PREFIX`. Changing `EMFOR_PREFIX` SHALL cause the next build to use the new path. `emfor --help` SHALL show the default config path built in. The target operating system SHALL NOT change the default.

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
