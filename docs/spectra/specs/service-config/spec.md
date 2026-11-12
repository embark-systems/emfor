# service-config Specification

## Purpose

Defines how operators configure emfor: the TOML file format, built-in defaults with per-service overrides, validation rules, the `--check-config` command, and how a running daemon reloads its config on `SIGHUP` without ever applying an invalid one.

## Requirements

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


<!-- @trace
source: emfor-status
updated: 2026-11-13
code:
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/build/prefix.rs
  - crates/emfor-daemon/Cargo.toml
  - docs/deploy/daemon.md
  - crates/emfor-daemon/build.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/config.rs
  - examples/rc.d/emfor
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Service names are normalized hostnames

The daemon SHALL normalize each service name to lowercase and strip one trailing dot. A normalized name SHALL be 1–253 characters made of dot-separated labels, each label 1–63 characters of `[a-z0-9-]` not starting or ending with `-`. Two service names that normalize to the same value SHALL be reported as a duplicate.

#### Scenario: Name normalization

- **WHEN** the config defines a service named `API.svc.emfor.`
- **THEN** the service is known as `api.svc.emfor`

#### Scenario: Duplicate after normalization

- **WHEN** the config defines services `api.svc.emfor` and `API.SVC.EMFOR`
- **THEN** validation fails with a duplicate service error naming `api.svc.emfor`


<!-- @trace
source: emfor-core
updated: 2026-11-12
code:
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/writer.rs
  - Cargo.toml
  - crates/emfor-daemon/src/cli.rs
  - CLAUDE.md
  - crates/emfor-daemon/src/main.rs
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/Cargo.toml
  - .spectra.yaml
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-daemon/src/checker/mod.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - examples/emfor.toml
  - crates/emfor-format/Cargo.toml
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Config validation rules

The daemon SHALL reject a config that violates any of these rules and SHALL report every violation, each prefixed with its TOML path:

- `interval` at least 100ms; `timeout` greater than zero and not greater than `interval` (equal is allowed, because checks of one backend run one after another and cannot overlap); `rise` at least 1; `fall` at least 1; `jitter` from 0% to 50%; `on_all_down` is `open` or `closed`; `balance` is `round-robin`, `sticky`, `failover`, `weighted`, `locality`, or `fastest`; `dns_ttl` is a whole number of seconds from 0s to 3600s.
- `backends` is non-empty; every entry is a string or a table; a string, and a table's `addr`, is `IPv4:port` or `[IPv6]:port` with port 1–65535; a table has `addr` and no keys other than `addr`, `weight`, `site`; `weight` is an integer from 1 to 100; no `addr` repeats within a service.
- The top-level `site` and every backend `site` are, after ASCII lowercasing, 1 to 63 characters of `[a-z0-9-]` that do not start or end with `-`.
- `check` names a registered checker and `check_options` passes that checker's validation.
- `[dns] listen` is a non-empty array of `IPv4:port` or `[IPv6]:port` with port 1–65535 and no repeats; `[dns] zone` follows the service name rules after the same normalization; `[dns] ttl` is a whole number of seconds from 0s to 3600s.
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
| `backends` | `[{ addr = "10.0.0.1:80", weight = 3, site = "dc1" }]` | ok |
| `backends` | `[{ weight = 3 }]` | error: backend table needs addr |
| `backends` | `[{ addr = "10.0.0.1:80", wieght = 3 }]` | error: unknown key |
| `backends` | `[{ addr = "10.0.0.1:80", weight = 0 }]` | error: weight must be from 1 to 100 |
| `backends` | `[{ addr = "10.0.0.1:80", weight = 101 }]` | error: weight must be from 1 to 100 |
| `backends` | `["10.0.0.1:80", { addr = "10.0.0.1:80" }]` | error: duplicate backend |
| `backends` | `[{ addr = "10.0.0.1:80", site = "dc_1" }]` | error: invalid site label |
| `site` (top level) | `"DC-1"` | ok, normalized to `dc-1` |
| `site` (top level) | `"-dc1"` | error: invalid site label |
| `chekc` (typo key) | `"tcp"` | error: unknown key |
| `check` | `"redis"` | error: unknown checker `redis` |
| `balance` | `"weighted"` | ok |
| `balance` | `"fastest"` | ok |
| `balance` | `"least-conn"` | error: balance must be round-robin, sticky, failover, weighted, locality, or fastest |
| `dns_ttl` | `"1500ms"` | error: dns_ttl must be whole seconds |
| `[dns] ttl` | `"2h"` | error: ttl above 3600s |
| `[dns] listen` | `[]` | error: listen must not be empty |
| `[dns] listen` | `["localhost:5300"]` | error: listen address must be an IP literal with port |
| `[dns] zone` | `"svc..emfor"` | error: invalid zone name |
| `[dns] zone` | `"SVC.Emfor."` | ok, normalized to `svc.emfor` |


<!-- @trace
source: emfor-balance-fastest
updated: 2026-11-12
code:
  - crates/emfor-format/src/lib.rs
  - crates/emfor-daemon/src/select.rs
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/dns.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/config.rs
  - examples/emfor.toml
  - docs/deploy/dns-resolver.md
  - crates/emfor-daemon/src/checker/mod.rs
  - clients/rust/src/client.rs
  - .spectra.yaml
  - tests/conformance/v1/cases.json
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/schedule.rs
  - clients/rust/README.md
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Check-config command

`emfor --check-config` SHALL validate the config file without opening the state store or starting checks. On success it SHALL print `config ok: <n> services, <m> backends` and exit with status 0. On failure it SHALL print each validation error on its own line and exit with status 2.

#### Scenario: Valid config

- **WHEN** an operator runs `emfor --check-config --config examples/emfor.toml` on a file with 2 services and 5 backends
- **THEN** the command prints `config ok: 2 services, 5 backends` and exits 0

#### Scenario: Invalid config

- **WHEN** the config contains an unknown key `services."api.svc.emfor".intervall`
- **THEN** the command prints an error containing `services."api.svc.emfor".intervall` and exits 2


<!-- @trace
source: emfor-core
updated: 2026-11-12
code:
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/writer.rs
  - Cargo.toml
  - crates/emfor-daemon/src/cli.rs
  - CLAUDE.md
  - crates/emfor-daemon/src/main.rs
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/Cargo.toml
  - .spectra.yaml
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-daemon/src/checker/mod.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - examples/emfor.toml
  - crates/emfor-format/Cargo.toml
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Startup with an invalid config

When the daemon starts with a config that fails validation, it SHALL log every validation error and exit with status 2 without opening the state store.

#### Scenario: Daemon refuses an invalid config

- **WHEN** the daemon starts with a config whose `rise` is 0
- **THEN** the daemon exits with status 2 and the store file is not created


<!-- @trace
source: emfor-core
updated: 2026-11-12
code:
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/writer.rs
  - Cargo.toml
  - crates/emfor-daemon/src/cli.rs
  - CLAUDE.md
  - crates/emfor-daemon/src/main.rs
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/Cargo.toml
  - .spectra.yaml
  - crates/emfor-daemon/src/checker/tcp.rs
  - crates/emfor-daemon/src/checker/mod.rs
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/store.rs
  - crates/emfor-format/src/lib.rs
  - examples/emfor.toml
  - crates/emfor-format/Cargo.toml
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Reload on SIGHUP applies only a fully valid config

On `SIGHUP` the daemon SHALL re-read the config file. If the new config passes validation, the daemon SHALL apply it. If any validation error exists, the daemon SHALL log every error at `error` level and continue running the previous config unchanged. A change to any key in the `[store]` table SHALL be a validation error on reload. A change to `[dns] listen`, including adding or removing the `[dns]` table, SHALL be a validation error on reload with the message "dns listen settings require restart". Changes to `[dns] zone`, `[dns] ttl`, `dns_ttl`, and `balance` SHALL take effect on reload.

#### Scenario: Valid reload adds a service

- **WHEN** a service is added to the config file and the daemon receives `SIGHUP`
- **THEN** the daemon starts checking the new service's backends

#### Scenario: Invalid reload is rejected

- **WHEN** the config file is edited so a service has `timeout = "5s"` and `interval = "2s"`, and the daemon receives `SIGHUP`
- **THEN** the daemon logs the timeout error, keeps checking with the previous settings, and the store is not changed by the reload

#### Scenario: Store settings change requires restart

- **WHEN** `[store] path` is changed in the config file and the daemon receives `SIGHUP`
- **THEN** the reload is rejected with the error "store settings require restart"

#### Scenario: DNS listen change requires restart

- **WHEN** `[dns] listen` is changed from `["127.0.0.1:5300"]` to `["127.0.0.1:5301"]` and the daemon receives `SIGHUP`
- **THEN** the reload is rejected with the error "dns listen settings require restart" and the DNS server keeps answering on 127.0.0.1:5300

#### Scenario: Balance change applies on reload

- **WHEN** a service with healthy backends 10.0.0.1 and 10.0.0.2 changes from `balance = "round-robin"` to `balance = "failover"` and the daemon receives `SIGHUP`
- **THEN** every following A query for that service returns 10.0.0.1

<!-- @trace
source: emfor-dns
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: Host site and backend weight and site

The config SHALL accept an optional top-level `site`, the label of the data centre this host is in. Each `backends` entry SHALL be either a backend string or a table with the key `addr` (a backend string) and the optional keys `weight` and `site`. `weight` SHALL default to 1 and `site` to none. A string entry SHALL be equivalent to a table with only `addr`. `weight` and `site` SHALL be accepted under every `balance` value; only `weighted` uses `weight` and only `locality` uses `site`. Site labels SHALL be ASCII-lowercased when read. A change of the top-level `site`, a backend's `weight`, or a backend's `site` SHALL take effect on reload.

#### Scenario: Mixed backend forms

- **WHEN** a service lists `backends = ["10.0.0.10:8080", { addr = "10.0.0.11:8080", weight = 3, site = "DC1" }]`
- **THEN** the service has backend `10.0.0.10:8080` with weight 1 and no site, and backend `10.0.0.11:8080` with weight 3 and site `dc1`

#### Scenario: Weight change applies on reload

- **WHEN** a `weighted` service changes a backend from `weight = 1` to `weight = 4` and the daemon receives `SIGHUP`
- **THEN** the store record for that backend shows `weight` 4 and the DNS answers follow the new weight

<!-- @trace
source: emfor-balance-2
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
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

<!-- @trace
source: emfor-status
updated: 2026-11-13
code:
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/build/prefix.rs
  - crates/emfor-daemon/Cargo.toml
  - docs/deploy/daemon.md
  - crates/emfor-daemon/build.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/config.rs
  - examples/rc.d/emfor
tests:
  - crates/emfor-daemon/tests/daemon.rs
-->