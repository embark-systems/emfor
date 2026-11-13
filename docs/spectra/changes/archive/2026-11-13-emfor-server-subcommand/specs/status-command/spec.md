## MODIFIED Requirements

### Requirement: Status reads the store read-only

`emfor status` SHALL read the state store at the `[store] path` of the config file given with `--config` (the build-time default config path when `--config` is absent), or `/var/lib/emfor/state.mdb` when the config file has no `[store] path`. It SHALL read only the `[store]` table of the config file and SHALL NOT validate the rest of the config, start plugins, run checks, or write to the store. It SHALL NOT take the daemon's writer lock, so a daemon can start or keep running while `emfor status` runs. When the store file does not exist, `emfor status` SHALL print `cannot open store <path>: no such file` and exit 2 without creating the store file or its `-lock` file. When `meta/schema_version` is not `1`, it SHALL print `store format version <n>; this emfor reads version 1` and exit 2. The other subcommands of `emfor` and running it without a subcommand SHALL follow the service-config requirement "Command line subcommands".

#### Scenario: Status beside a running daemon

- **WHEN** a daemon is running with the store at `/tmp/t/state.mdb` and an operator runs `emfor status --config <that config>`
- **THEN** the command prints the services and exits, and the daemon keeps writing heartbeats

#### Scenario: Store missing

- **WHEN** the config sets `[store] path = "/tmp/t/none.mdb"` and that file does not exist
- **THEN** `emfor status` prints `cannot open store /tmp/t/none.mdb: no such file`, exits 2, and neither `/tmp/t/none.mdb` nor `/tmp/t/none.mdb-lock` exists afterwards

#### Scenario: Rest of the config is not validated

- **WHEN** the config file has a valid `[store]` table and a service with an unknown key `intervall`
- **THEN** `emfor status` shows the store contents instead of a validation error
