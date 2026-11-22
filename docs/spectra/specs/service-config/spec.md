# service-config Specification

## Purpose

Defines how operators configure emfor: the TOML file format, built-in defaults with per-service overrides, validation rules, the `emfor check-config` command and the `emfor` subcommands, and how a running daemon reloads its config on `SIGHUP` without ever applying an invalid one.

## Requirements

### Requirement: TOML config with defaults and per-service overrides

The daemon SHALL read its configuration from a TOML file, the default config path fixed at build time (`/etc/emfor/emfor.toml` unless built with `EMFOR_PREFIX`) or the path given with `--config`. Each service under `[services."<name>"]` SHALL use the value of `interval`, `timeout`, `rise`, `fall`, `jitter`, `on_all_down`, `balance`, and `peers` from its own table when present, otherwise from `[defaults]`, otherwise from the built-in defaults: `interval = "2s"`, `timeout = "1s"`, `rise = 2`, `fall = 3`, `jitter = "10%"`, `on_all_down = "open"`, `balance = "round-robin"`, `peers = true`. A service MAY also set `dns_ttl`, which overrides `[dns] ttl` for that service and has no `[defaults]` entry. Durations SHALL be strings with units (for example `"500ms"`, `"2s"`).

#### Scenario: Service inherits and overrides settings

- **WHEN** `[defaults]` sets `interval = "5s"` and a service sets only `interval = "1s"`
- **THEN** that service uses `interval` 1s and `timeout` 1s, `rise` 2, `fall` 3, `jitter` 10%, `on_all_down` open, `balance` round-robin, `peers` true

##### Example: effective settings resolution

| `[defaults]` | Service table | Effective `interval` | Effective `fall` | Effective `balance` | Effective `peers` |
| --- | --- | --- | --- | --- | --- |
| absent | absent | 2s | 3 | round-robin | true |
| `interval = "5s"` | absent | 5s | 3 | round-robin | true |
| `interval = "5s"`, `fall = 5` | `interval = "1s"` | 1s | 5 | round-robin | true |
| `balance = "failover"` | absent | 2s | 3 | failover | true |
| `balance = "failover"` | `balance = "sticky"` | 2s | 3 | sticky | true |
| `peers = false` | absent | 2s | 3 | round-robin | false |
| `peers = false` | `peers = true` | 2s | 3 | round-robin | true |


<!-- @trace
source: emfor-peers-service-off
updated: 2026-11-13
code:
  - crates/emfor-daemon/src/writer.rs
  - docs/manual/05-configuring.md
  - crates/emfor-daemon/src/quorum.rs
  - crates/emfor-daemon/src/peers/
  - docs/manual/12-operating.md
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/supervisor.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/config.rs
  - examples/emfor.toml
  - CHANGELOG.md
  - docs/manual/03-concepts.md
  - crates/emfor-daemon/src/schedule.rs
tests:
  - crates/emfor-daemon/tests/peers.rs
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

- `interval` from 100ms to 24h; `timeout` at least 10ms and not greater than `interval` (equal is allowed, because checks of one backend run one after another and cannot overlap); `rise` at least 1; `fall` at least 1; `jitter` from 0% to 50%; `on_all_down` is `open` or `closed`; `balance` is `round-robin`, `sticky`, `failover`, `weighted`, `locality`, or `fastest`; `dns_ttl` is a whole number of seconds from 0s to 3600s; `peers` is a boolean.
- `backends` is non-empty; every entry is a string or a table; a string, and a table's `addr`, is `IPv4:port` or `[IPv6]:port` with port 1–65535; a table has `addr` and no keys other than `addr`, `weight`, `site`, `backup`; `weight` is an integer from 1 to 100; no `addr` repeats within a service.
- The top-level `site` and every backend `site` are, after ASCII lowercasing, 1 to 63 characters of `[a-z0-9-]` that do not start or end with `-`.
- `check` names a registered checker and `check_options` passes that checker's validation.
- `[dns] listen` is a non-empty array of `IPv4:port` or `[IPv6]:port` with port 1–65535 and no repeats; `[dns] zone` follows the service name rules after the same normalization and is at most 242 characters, so that the SOA name `hostmaster.<zone>` fits in 253; `[dns] ttl` is a whole number of seconds from 0s to 3600s; `[dns] rate_limit` is an integer from 0 to 1000000.
- With `[dns]` present, for every service whose name is in the zone, the SRV target name of each backend (`<ip label>.<service>`, spec dns-server "SRV target names") is at most 253 characters; otherwise the error is on that backend, `services."<name>".backends[<i>]`. A 230-character service name fits an IPv4 backend but not an IPv6 one.
- `[metrics] listen` is a non-empty array of `IPv4:port` or `[IPv6]:port` with port 1–65535 and no repeats; `[metrics] allow`, when present, is a non-empty array of CIDR strings (an IPv4 or IPv6 address, `/`, and a prefix length of 0–32 or 0–128) with no repeated entry; `[metrics]` has no keys other than `listen` and `allow`.
- `[peers]` follows the peer group configuration rules (spec peer-group), and the top-level `site` is set when `[peers]` is present.
- No unknown keys anywhere in the file.

#### Scenario: Multiple errors reported together

- **WHEN** a service sets `timeout = "3s"` with `interval = "2s"` and also lists backend `10.0.0.1:70000`
- **THEN** validation reports both the timeout error and the port error

##### Example: validation outcomes

| Setting | Value | Result |
| --- | --- | --- |
| `interval` | `"50ms"` | error: interval below 100ms |
| `interval` | `"25h"` | error: interval above 24h |
| `timeout` | `"9ms"` | error: timeout below 10ms |
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
| `[dns] rate_limit` | `-1` | error: rate_limit must be 0 to 1000000 |
| `[dns] listen` | `["localhost:5300"]` | error: listen address must be an IP literal with port |
| `[dns] zone` | `"svc..emfor"` | error: invalid zone name |
| `[dns] zone` | a valid name of 243 characters | error: zone name longer than 242 characters |
| `[dns] zone` | `"SVC.Emfor."` | ok, normalized to `svc.emfor` |
| `backends` of a service named in the zone, 230 characters | `["[2001:db8::1]:80"]` | error: service name too long for this backend's SRV target name |
| `[metrics] listen` | `[]` | error: listen must not be empty |
| `[metrics] listen` | `["localhost:5301"]` | error: listen address must be an IP literal with port |
| `[metrics] listen` | `["127.0.0.1:5301", "127.0.0.1:5301"]` | error: duplicate listen address |
| `[metrics] allow` | `[]` | error: allow must not be empty |
| `[metrics] allow` | `["10.0.0.1"]` | error: allow entry must be an address with a prefix length, such as "10.0.0.0/8" |
| `[metrics] allow` | `["10.0.0.0/33"]` | error: allow entry must be an address with a prefix length, such as "10.0.0.0/8" |
| `[metrics] allow` | `["10.0.0.0/8", "10.0.0.0/8"]` | error: duplicate allow entry |
| `[metrics] allow` | `["10.0.0.0/8", "fd00::/8"]` | valid |
| `[metrics] path` | `"/m"` | error: unknown key |
| `[peers]` | present, no top-level `site` | error: site is required when [peers] is set |
| `[peers] members` | `["10.0.0.5:5302"]` | error: members must list 2 to 64 addresses |
| `peers` | `false` | ok |
| `peers` | `"no"` | error: peers must be true or false |
| `peers` (no `[peers]` table) | `false` | ok |


<!-- @trace
source: emfor-metrics-allow
updated: 2026-11-23
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/metrics.rs
  - docs/manual.zh_TW/05-configuring.md
  - docs/manual.zh_TW/12-operating.md
  - docs/manual.zh_TW/13-security.md
  - docs/manual/05-configuring.md
  - docs/manual/12-operating.md
  - docs/manual/13-security.md
tests:
  - crates/emfor-daemon/tests/metrics.rs
-->

---
### Requirement: Check-config command

`emfor check-config` SHALL validate the config file without opening the state store or starting checks. On success it SHALL print `config ok: <n> services, <m> backends` and exit with status 0. On failure it SHALL print each validation error on its own line and exit with status 2.

#### Scenario: Valid config

- **WHEN** an operator runs `emfor check-config --config emfor.toml` where `emfor.toml` has 2 services and 5 backends
- **THEN** the command prints `config ok: 2 services, 5 backends` and exits 0

#### Scenario: Invalid config in check-config

- **WHEN** the config contains an unknown key `services."api.svc.emfor".intervall`
- **THEN** the command prints an error containing `services."api.svc.emfor".intervall` and exits 2


<!-- @trace
source: emfor-server-subcommand
updated: 2026-11-13
code:
  - README.md
  - docs/deploy/plugins.md
  - examples/systemd/emfor.service
  - tests/spike/run.sh
  - docs/plugins/protocol-v1.md
  - examples/freebsd/Makefile
  - examples/freebsd/README.md
  - examples/freebsd/files/emfor.in
  - CHANGELOG.md
  - tests/plugins/inside.sh
  - README.zh-TW.md
  - docs/deploy/daemon.md
  - tests/resolver/resolved.sh
  - examples/emfor.toml
  - tests/resolver/inside.sh
  - crates/emfor-daemon/src/cli.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - crates/emfor-daemon/tests/metrics.rs
  - crates/emfor-daemon/tests/daemon.rs
  - crates/emfor-daemon/tests/plugins.rs
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

On `SIGHUP` the daemon SHALL re-read the config file. If the new config passes validation, the daemon SHALL apply it. If any validation error exists, the daemon SHALL log every error at `error` level and continue running the previous config unchanged. A change to any key in the `[store]` table SHALL be a validation error on reload. A change to `[dns] listen`, including adding or removing the `[dns]` table, SHALL be a validation error on reload with the message "dns listen settings require restart". A change to `[metrics] listen`, including adding or removing the `[metrics]` table, SHALL be a validation error on reload with the message "metrics listen settings require restart". A change to `[peers] listen`, including adding or removing the `[peers]` table, SHALL be a validation error on reload with the message "peers listen settings require restart". Changes to `[dns] zone`, `[dns] ttl`, `dns_ttl`, `balance`, `[peers] members`, `[peers] secret_file` and `peers` SHALL take effect on reload. A change to `peers` alone SHALL keep the state of the service's backends. Reading the config file and its fragments SHALL NOT keep the daemon from handling `SIGTERM` or `SIGINT`: when a stop request arrives while a reload is reading or validating, the daemon SHALL abandon the reload, log one line at `info` level saying so, and shut down, even if a read has not returned (a file on a hung file system). Only one reload SHALL run at a time; a `SIGHUP` that arrives while one runs SHALL cause one more reload after it, however many arrive.

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

#### Scenario: Metrics listen change requires restart

- **WHEN** the daemon runs with `[metrics] listen = ["127.0.0.1:5301"]`, the config file is changed to `["127.0.0.1:5302"]`, and the daemon receives `SIGHUP`
- **THEN** the reload is rejected with the error "metrics listen settings require restart" and `GET /metrics` keeps answering on 127.0.0.1:5301

#### Scenario: Peers listen change requires restart

- **WHEN** the daemon runs with `[peers] listen = "127.0.0.1:5302"`, the config file is changed to `"127.0.0.1:5305"`, and the daemon receives `SIGHUP`
- **THEN** the reload is rejected with the error "peers listen settings require restart"

#### Scenario: Leaving the peer group on reload keeps backend state

- **WHEN** a service with a `healthy` backend is changed from `peers = true` to `peers = false` and the daemon receives `SIGHUP`
- **THEN** the backend stays `healthy` and the daemon no longer sends `down` or `result` messages for that service

#### Scenario: Stop request during a reload that cannot read the config

- **WHEN** the config path is replaced by a named pipe that no process has open for writing, the daemon receives `SIGHUP`, and then `SIGTERM`
- **THEN** the daemon logs that a stop was requested while reloading and the reload is abandoned, and the process exits within 5 seconds of `SIGTERM`

#### Scenario: SIGHUP during a reload

- **WHEN** a reload is running and the daemon receives `SIGHUP` three times
- **THEN** the running reload ends and exactly one more reload runs after it


<!-- @trace
source: emfor-reload-and-plugin-lifetime
updated: 2026-11-21
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/plugin/host.rs
tests:
  - crates/emfor-daemon/tests/daemon.rs
  - crates/emfor-daemon/tests/plugin_host.rs
  - crates/emfor-daemon/tests/support/test_plugin.rs
-->

---
### Requirement: Host site and backend weight and site

The config SHALL accept an optional top-level `site`, the label of the data centre this host is in. Each `backends` entry SHALL be either a backend string or a table with the key `addr` (a backend string) and the optional keys `weight`, `site` and `backup`. `weight` SHALL default to 1, `site` to none and `backup` to `false`; `backup` SHALL be a boolean. A service whose every backend has `backup = true` SHALL be a config error at the service's `backends` path with the message `at least one backend must not be a backup`. A string entry SHALL be equivalent to a table with only `addr`. `weight` and `site` SHALL be accepted under every `balance` value; only `weighted` uses `weight` and only `locality` uses `site`. Site labels SHALL be ASCII-lowercased when read. `backup` SHALL be accepted under every `balance` value. A change of the top-level `site`, a backend's `weight`, a backend's `site`, or a backend's `backup` SHALL take effect on reload, and the backend SHALL keep its health state.

#### Scenario: Mixed backend forms

- **WHEN** a service lists `backends = ["10.0.0.10:8080", { addr = "10.0.0.11:8080", weight = 3, site = "DC1" }]`
- **THEN** the service has backend `10.0.0.10:8080` with weight 1 and no site, and backend `10.0.0.11:8080` with weight 3 and site `dc1`

#### Scenario: Weight change applies on reload

- **WHEN** a `weighted` service changes a backend from `weight = 1` to `weight = 4` and the daemon receives `SIGHUP`
- **THEN** the store record for that backend shows `weight` 4 and the DNS answers follow the new weight

#### Scenario: Backup backends

- **WHEN** a service lists `backends = ["10.0.0.10:8080", { addr = "10.0.1.10:8080", backup = true }]`
- **THEN** the service has backend `10.0.0.10:8080` with `backup` false and backend `10.0.1.10:8080` with `backup` true

#### Scenario: Only backup backends

- **WHEN** service `api.svc.emfor` lists only `{ addr = "10.0.1.10:8080", backup = true }`
- **THEN** `emfor check-config` reports `services."api.svc.emfor".backends: at least one backend must not be a backup` and exits 2

##### Example: backup values

| `backup` value in a backend table | Result |
| --- | --- |
| absent | not a backup |
| `false` | not a backup |
| `true` | backup |
| `"yes"` | config error: `backup must be true or false` |


<!-- @trace
source: emfor-backup-backends
updated: 2026-11-14
code:
  - docs/manual.zh_TW/05-backend-selection.md
  - clients/go/emfor.go
  - crates/emfor-daemon/src/status.rs
  - tests/conformance/v1/README.md
  - clients/python/emfor/__init__.py
  - crates/emfor-daemon/src/writer.rs
  - docs/manual.zh_TW/07-dns.md
  - examples/emfor.toml
  - tests/conformance/v1/cases.json
  - README.md
  - clients/rust/src/lib.rs
  - clients/perl/README.md
  - clients/rust/src/client.rs
  - crates/emfor-format/src/lib.rs
  - docs/manual.zh_TW/03-configuration.md
  - crates/emfor-daemon/src/state.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/select.rs
  - crates/emfor-daemon/src/config.rs
  - clients/python/README.md
  - clients/rust/README.md
  - docs/manual/09-clients.md
  - docs/manual/10-dns.md
  - CHANGELOG.md
  - docs/manual/05-configuring.md
  - crates/emfor-daemon/src/dns.rs
  - docs/manual.zh_TW/06-clients.md
  - docs/manual/08-choosing-a-backend.md
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/metrics.rs
  - crates/emfor-format/src/rules.rs
  - clients/go/README.md
  - clients/php/README.md
tests:
  - crates/emfor-daemon/tests/daemon.rs
  - clients/rust/tests/conformance_capi.rs
-->

---
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


<!-- @trace
source: emfor-server-subcommand
updated: 2026-11-13
code:
  - README.md
  - docs/deploy/plugins.md
  - examples/systemd/emfor.service
  - tests/spike/run.sh
  - docs/plugins/protocol-v1.md
  - examples/freebsd/Makefile
  - examples/freebsd/README.md
  - examples/freebsd/files/emfor.in
  - CHANGELOG.md
  - tests/plugins/inside.sh
  - README.zh-TW.md
  - docs/deploy/daemon.md
  - tests/resolver/resolved.sh
  - examples/emfor.toml
  - tests/resolver/inside.sh
  - crates/emfor-daemon/src/cli.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - crates/emfor-daemon/tests/metrics.rs
  - crates/emfor-daemon/tests/daemon.rs
  - crates/emfor-daemon/tests/plugins.rs
-->

---
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

<!-- @trace
source: emfor-server-subcommand
updated: 2026-11-13
code:
  - README.md
  - docs/deploy/plugins.md
  - examples/systemd/emfor.service
  - tests/spike/run.sh
  - docs/plugins/protocol-v1.md
  - examples/freebsd/Makefile
  - examples/freebsd/README.md
  - examples/freebsd/files/emfor.in
  - CHANGELOG.md
  - tests/plugins/inside.sh
  - README.zh-TW.md
  - docs/deploy/daemon.md
  - tests/resolver/resolved.sh
  - examples/emfor.toml
  - tests/resolver/inside.sh
  - crates/emfor-daemon/src/cli.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - crates/emfor-daemon/tests/metrics.rs
  - crates/emfor-daemon/tests/daemon.rs
  - crates/emfor-daemon/tests/plugins.rs
-->

---
### Requirement: Slow start setting

A service SHALL accept `slow_start`, a duration, in its own table or in `[defaults]`, with the service's value taking precedence and a built-in default of `"0s"`. `slow_start` SHALL be `0s` (slow start off) or from 1s to 1h; any other value SHALL be a validation error prefixed with its TOML path, reported together with every other violation as in "Config validation rules". `slow_start` SHALL be accepted under every `balance` value. A change of `slow_start` SHALL take effect on reload, including for backends whose slow start is in progress.

#### Scenario: Default and override

- **WHEN** `[defaults]` sets `slow_start = "30s"`, service `a.svc.emfor` sets nothing and service `b.svc.emfor` sets `slow_start = "0s"`
- **THEN** `a.svc.emfor` has slow start 30s and `b.svc.emfor` has slow start off

#### Scenario: Out of range

- **WHEN** service `api.svc.emfor` sets `slow_start = "2h"`
- **THEN** check-config fails with an error starting `services."api.svc.emfor".slow_start:`

##### Example: slow_start validation

| `slow_start` | Result |
| --- | --- |
| absent | ok, off |
| `"0s"` | ok, off |
| `"500ms"` | error |
| `"1s"` | ok |
| `"30s"` | ok |
| `"1h"` | ok |
| `"2h"` | error |
| `30` | error: not a duration string |

<!-- @trace
source: emfor-slow-start
updated: 2026-11-14
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/state.rs
  - tools/emfor-fixture/src/lib.rs
  - docs/manual.zh_TW/07-dns.md
  - tests/conformance/v1/README.md
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-format/src/rules.rs
  - tests/conformance/v1/cases.json
  - crates/emfor-format/src/lib.rs
  - docs/manual/10-dns.md
  - crates/emfor-daemon/src/select.rs
  - crates/emfor-daemon/src/writer.rs
  - docs/manual/05-configuring.md
  - crates/emfor-daemon/src/supervisor.rs
  - docs/manual.zh_TW/05-backend-selection.md
  - crates/emfor-daemon/src/metrics.rs
  - CHANGELOG.md
  - docs/manual/08-choosing-a-backend.md
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/dns.rs
  - docs/manual.zh_TW/03-configuration.md
  - clients/rust/src/client.rs
-->

---
### Requirement: Config directory with ordered fragments

The main config file MAY set a top-level `include_dir` string. A relative value SHALL be resolved against the directory of the main config file; an absolute value SHALL be used as is. The daemon SHALL read the main config file first and then every selected file of that one directory, and merge them into a single config before validation, so that the main file is the lowest layer and a later file overrides an earlier one. A file SHALL be selected when its name ends in `.toml`, does not start with `.`, and it is a regular file or a symbolic link to a regular file. Selected files SHALL be read in byte-wise filename order, not locale order. A name that is not valid UTF-8 SHALL be skipped and reported as a warning. Every other entry, including subdirectories and names such as `foo.toml~`, `.x.toml.swp` and `x.toml.disabled`, SHALL be ignored. An empty directory SHALL be valid. A selected file whose type or content cannot be read, for any reason other than that it does not exist (a dangling link), SHALL be a config error that names it. An `include_dir` value that is the empty string SHALL be a config error. When `include_dir` is not set, loading and the output of `emfor check-config` SHALL be unchanged.

#### Scenario: Files are read in filename order

- **WHEN** `include_dir = "conf.d"` and the directory holds `10-b.toml`, `01-a.toml` and `9-c.toml`
- **THEN** the files are read in the order `01-a.toml`, `10-b.toml`, `9-c.toml`, after the main file

#### Scenario: Unselected entries are ignored

- **WHEN** the directory holds `a.toml`, `.hidden.toml`, `b.toml~`, `c.toml.disabled` and a subdirectory `d.toml`
- **THEN** only `a.toml` is read

#### Scenario: Symbolic link to a regular file is followed

- **WHEN** `10-svc.toml` is a symbolic link to a regular file elsewhere
- **THEN** that file is read as `10-svc.toml`

#### Scenario: Relative include_dir

- **WHEN** the main file is `/etc/emfor/emfor.toml` and `include_dir = "conf.d"`
- **THEN** the directory read is `/etc/emfor/conf.d`

#### Scenario: Non-UTF-8 file name

- **WHEN** a file in the directory has a name that is not valid UTF-8
- **THEN** it is skipped, a warning names it, and loading continues

#### Scenario: Dangling link is skipped, unreadable link is an error

- **WHEN** the directory holds `a.toml` as a link to a path that does not exist, and `loop.toml` as a link to itself
- **THEN** `a.toml` is skipped without error, and loading fails with an error that names `loop.toml`

#### Scenario: Empty include_dir

- **WHEN** the main file sets `include_dir = ""`
- **THEN** loading fails with the error `include_dir: must not be empty`

---
### Requirement: Fragment merge rules

When files are merged, a table in a later file and a table at the same key in an earlier layer SHALL be merged key by key, recursively. Any other combination SHALL be replaced as a whole by the later value: a scalar, a list (including `backends` and `command`), or two values of different types. A later file SHALL NOT delete a service, plugin or backend defined in an earlier layer. A service whose merged table lacks `check` or `backends` SHALL be a validation error, as in a single file. The merged config SHALL be validated, and SHALL behave in plugin startup, reload restart checks and peer group digest exactly as the same content written in one file.

#### Scenario: Table merges key by key

- **WHEN** the main file defines `[services."api.svc.emfor"]` with `check`, `backends` and `interval = "2s"`, and `99-local.toml` defines the same service with only `interval = "500ms"`
- **THEN** the service keeps `check` and `backends` and uses `interval` 500ms

#### Scenario: List is replaced

- **WHEN** the main file sets `backends = ["10.0.0.1:80", "10.0.0.2:80"]` and a later file sets `backends = ["10.0.0.3:80"]` for the same service
- **THEN** the service has the single backend `10.0.0.3:80`

#### Scenario: Plugin declared in a fragment

- **WHEN** a fragment declares `[plugins."probe"]` with a `command` and a service uses `check = "probe"`
- **THEN** the plugin is started as if declared in the main file

#### Scenario: Fragment changes a listen address

- **WHEN** a reload finds that a fragment changed `[dns] listen`
- **THEN** the reload reports that the change needs a restart, as for the same change in the main file

#### Scenario: Peer digest does not depend on file splitting

- **WHEN** a service is defined in one file, and the same service with the same values is split across two files
- **THEN** both give the same service definition digest

---
### Requirement: Keys allowed only in the main file

`[store]` and `include_dir` SHALL be accepted only in the main config file. A fragment that contains either SHALL be a config error naming that file. All other tables and the top-level `site` key MAY appear in fragments and SHALL be merged. `emfor status` and `emfor lookup` SHALL read only the main config file and SHALL NOT need to read the directory.

#### Scenario: Store in a fragment

- **WHEN** `10-store.toml` contains a `[store]` table
- **THEN** loading fails with an error that names `10-store.toml`

#### Scenario: Nested include

- **WHEN** a fragment sets `include_dir`
- **THEN** loading fails with an error that names that fragment

#### Scenario: status and lookup need only the main file

- **WHEN** a fragment in the directory is unreadable by the user running `emfor status`
- **THEN** `emfor status` and `emfor lookup` still work

---
### Requirement: Config directory permissions

The daemon SHALL refuse to load the config when the main config file, the include directory, or any file read from the include directory (for a symbolic link, the file it points to), is writable by group or others (mode `& 0o022` not zero), or is owned by a user other than root and the user the daemon runs as (the effective user). A file SHALL be checked after it has been opened, on the opened file, and read from that same open file. The error SHALL name the path and the reason, the mode or the owner, and the command that fixes it (`chmod g-w,o-w <path>` for a mode, `chown root <path>` for an owner). The main config file SHALL be checked by `emfor server`, at start and at every reload, and by `emfor check-config`. `emfor status` and `emfor lookup`, which read only `[store]` of the main file, SHALL NOT check it. A refusal at start SHALL exit with the config error status without opening the store; a refusal at a reload SHALL keep the previous config.

#### Scenario: Group-writable fragment

- **WHEN** `10-a.toml` has mode 0664
- **THEN** loading fails with an error naming `10-a.toml` and its mode

#### Scenario: World-writable directory

- **WHEN** the include directory has mode 0777
- **THEN** loading fails with an error naming the directory

#### Scenario: Main file that others can write

- **WHEN** the main file has mode 0664 and `emfor check-config` reads it
- **THEN** it fails with an error naming the main file, its mode 0664 and the command `chmod g-w,o-w` with the path, and exits with status 2

##### Example: Main file modes

| Mode | Loading |
| ---- | ------- |
| 0600 | accepted |
| 0640 | accepted |
| 0644 | accepted |
| 0604 | accepted |
| 0620 | refused: group can write |
| 0602 | refused: others can write |
| 0664 | refused |
| 0666 | refused |

#### Scenario: Main file owned by another user

- **WHEN** the main file has mode 0644 and is owned by uid 12345, which is neither root nor the user running emfor
- **THEN** loading fails with an error naming the main file and `owned by uid 12345`, and the command `chown root` with the path

#### Scenario: Link to a main file that is not trusted

- **WHEN** the main file given to the daemon is a symbolic link to a file with mode 0664
- **THEN** loading fails with an error naming the path and the mode 0664, because a link is judged by the file it points to

#### Scenario: Start with a main file that others can write

- **WHEN** the daemon starts and the main file has mode 0664
- **THEN** the error is logged, the process exits with status 2, and the store is not opened

#### Scenario: Reload with a main file that others can write

- **WHEN** the daemon is running and the main file is changed to mode 0664 and the daemon receives SIGHUP
- **THEN** the error is logged, the reload is rejected, and the daemon keeps running with the previous config

#### Scenario: status and lookup do not check the main file

- **WHEN** the main file has mode 0664 and `emfor status` or `emfor lookup` is run
- **THEN** it reads the `[store]` table and works as it did before

#### Scenario: Fragment owned by another user

- **WHEN** `10-a.toml` has mode 0644 and is owned by uid 12345, which is neither root nor the user running emfor
- **THEN** loading fails with an error naming `10-a.toml` and `owned by uid 12345`

#### Scenario: Link to a file that is not trusted

- **WHEN** `10-svc.toml` is a symbolic link to a file with mode 0664, or to a file owned by another user
- **THEN** loading fails with an error naming `10-svc.toml`, because a link is judged by the file it points to

#### Scenario: Directory owned by another user

- **WHEN** the include directory is owned by another user
- **THEN** loading fails with an error naming the directory


<!-- @trace
source: emfor-main-file-trust
updated: 2026-11-21
code:
  - CHANGELOG.md
  - SECURITY.md
  - crates/emfor-daemon/src/config.rs
  - docs/deploy/daemon.md
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual.zh_TW/08-operations.md
  - docs/manual/05-configuring.md
  - docs/manual/12-operating.md
tests:
  - crates/emfor-daemon/tests/common/daemon.rs
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Missing config directory

An `include_dir` that does not exist or is not a directory SHALL be a config error. At daemon start the error SHALL be logged and the process SHALL exit with the config error status without opening the store. On a reload the error SHALL reject the reload, the daemon SHALL keep running with the previous config, and the error SHALL be logged.

#### Scenario: Missing directory at start

- **WHEN** the daemon starts and `include_dir` names a directory that does not exist
- **THEN** it logs the error and exits with status 2

#### Scenario: Directory removed before a reload

- **WHEN** the directory is removed and the daemon receives `SIGHUP`
- **THEN** the reload is rejected, the previous config stays in effect, and the daemon keeps running

---
### Requirement: Errors name their file

A syntax error in the main file or a fragment SHALL be reported with the file name, line and column. A validation error SHALL be reported with its key path and, when known, the file that last set that key or its nearest recorded parent key. Attribution SHALL be best effort when an error arises from values set in different files. An error found after validation, namely a plugin that fails to start or a change that needs a restart on reload, SHALL name the file in the same way when its key path is known. Any error in the main file or any fragment SHALL reject the whole load, and on reload the previous config SHALL be kept.

#### Scenario: Syntax error in a fragment

- **WHEN** `20-x.toml` has a TOML syntax error on line 3, column 7
- **THEN** the error reads `20-x.toml:3:7` followed by the TOML message

#### Scenario: Invalid value in a fragment

- **WHEN** `30-y.toml` sets `interval = "fast"` for a service
- **THEN** the error shows the key path `services."api.svc.emfor".interval` and the file `30-y.toml`

#### Scenario: Plugin start failure and restart-required change name their file

- **WHEN** a plugin declared in `20-probe.toml` fails to start, or a reload finds `[dns] listen` changed by `20-dns.toml`
- **THEN** the error for `plugins.probe.command` or `dns.listen` is followed by `(in <path of that file>)`

#### Scenario: One bad fragment rejects a reload

- **WHEN** one of three fragments is invalid at reload
- **THEN** none of the three is applied and the previous config stays in effect

---
### Requirement: Listing the files read

When `include_dir` is set, `emfor check-config` SHALL print its usual `config ok: <n> services, <m> backends` line and then one `read <path>` line per file read, the main file first, in read order. The daemon SHALL log, at `info` level, one `config file read` line per file read, with the fields `file` and `sha256` (the SHA-256 of the file's bytes as read, lowercase hexadecimal, as `sha256sum` prints it), the main file first, in read order, whether or not `include_dir` is set: at start after the config is accepted, and at every reload whose files were read, whether the reload is then accepted or rejected. A load that cannot read a file SHALL log no hash for it. Warnings about skipped file names SHALL be printed on standard error by `emfor check-config` and logged by the daemon at start and at every reload, whether or not the load succeeds. When `include_dir` is not set, the output of `emfor check-config` SHALL be exactly as before.

#### Scenario: check-config with fragments

- **WHEN** `include_dir` is set and two fragments are read
- **THEN** the output is the `config ok:` line followed by three `read` lines: the main file and the two fragments in order

#### Scenario: Skipped name reported by check-config and by a failed load

- **WHEN** a fragment name is not valid UTF-8 and the rest of the config is valid, or the load fails for another reason
- **THEN** `emfor check-config` writes a warning naming the skipped file to standard error, and the daemon logs it at start and reload

#### Scenario: check-config without include_dir

- **WHEN** `include_dir` is not set
- **THEN** the output is the single `config ok:` line, as before

#### Scenario: Daemon logs the hash of every file read

- **WHEN** the daemon starts with a main file and no `include_dir`, and later reloads after the file is edited
- **THEN** the log has a `config file read` line with `file` and the `sha256` that `sha256sum` prints for the file, at start and again after the reload with the new value

#### Scenario: Rejected reload still logs the hashes

- **WHEN** the file is edited so that validation fails and the daemon receives `SIGHUP`
- **THEN** the log has the `config file read` line with the hash of the edited file and the `reload rejected` line


<!-- @trace
source: emfor-reload-audit-log
updated: 2026-11-23
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/audit.rs
  - crates/emfor-daemon/src/audit/tests.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/lib.rs
  - docs/manual.zh_TW/12-operating.md
  - docs/manual.zh_TW/13-security.md
  - docs/manual.zh_TW/15-upgrading.md
  - docs/manual/12-operating.md
  - docs/manual/13-security.md
  - docs/manual/15-upgrading.md
tests:
  - crates/emfor-daemon/tests/audit_log.rs
-->

---
### Requirement: Showing the merged config

`emfor check-config --show` SHALL load and validate the config exactly as `emfor check-config` does, with the same errors and the same exit status. When the config is invalid it SHALL write nothing to standard output. When it is valid it SHALL write the merged config as TOML to standard output and nothing else, and SHALL write the usual `config ok: <n> services, <m> backends` line, and the `read <path>` lines when `include_dir` is set, to standard error. The TOML with its comments removed SHALL parse to the merged config without `include_dir`, which SHALL appear only as a comment, because its files are already merged, so that the output saved as a file checks without a `conf.d` next to it. Built-in defaults SHALL NOT be added, and no value SHALL be redacted. When `include_dir` is set, every key's value SHALL be followed, on its last line, by a comment `# <file>` naming the file that last set that key (a list or an inline table is set as a whole, so its comment names the file that set it; a multi-line string is followed by the comment after its closing quotes); when it is not set, the output SHALL begin with one comment line `# merged from <file>` and have no other comments. Without `--show`, the output of `emfor check-config` SHALL be unchanged.

#### Scenario: Valid config with fragments

- **WHEN** the main file sets `[services."api.svc.emfor"]` with `interval = "2s"` and `99-local.toml` sets `interval = "5s"` for the same service
- **THEN** standard output has the line `interval = "5s" # <path of 99-local.toml>` and the other keys of the service with the main file as their source

#### Scenario: Round trip

- **WHEN** the output of `--show` is saved as a file in another directory, without any `conf.d`, and given to `emfor check-config --config`
- **THEN** it is valid and `--show` of that file gives the same TOML without the comments

#### Scenario: Without include_dir

- **WHEN** `include_dir` is not set
- **THEN** the first line is `# merged from <main file>` and no other line has a comment

#### Scenario: Invalid config when showing the merged config

- **WHEN** the config has an error
- **THEN** standard output is empty, standard error has the same errors as `emfor check-config`, and the exit status is 2

#### Scenario: Defaults are not shown

- **WHEN** a service sets no `interval`
- **THEN** the output has no `interval` line for it

#### Scenario: Without the flag

- **WHEN** `emfor check-config` runs without `--show`
- **THEN** its standard output and exit status are as before

---
### Requirement: Check-config output errors

`emfor check-config` SHALL write its output with a write that reports errors. When standard output cannot be written (a full disk), it SHALL print `cannot write the output: <error>` to standard error and exit 1. A reader that closes the pipe early (`| head`) SHALL NOT be a failure. It SHALL NOT panic or exit with 101.

#### Scenario: Full disk

- **WHEN** `emfor check-config` runs with standard output on a device that is full
- **THEN** it prints `cannot write the output` to standard error and exits 1

#### Scenario: Closed pipe

- **WHEN** the reader of standard output has gone away
- **THEN** the exit status is the one the check gives (0 for a valid config)

---
### Requirement: Draining a backend

A backend given as a table in `backends` MAY set `drain`, a boolean that defaults to `false`: `{ addr = "10.0.0.11:8080", drain = true }`. A `drain` value that is not a boolean SHALL be a validation error whose path is that backend's `drain` key and whose message is `drain must be true or false`. A reload that sets, changes or removes `drain` SHALL be applied and SHALL NOT restart the backend's check or reset its state, latency or recovery time. A backend with `drain = true` SHALL keep being checked. `drain` SHALL NOT be part of the service definition digest, so hosts of one peer group MAY drain different backends. A service in which every backend is drained SHALL be valid.

#### Scenario: Drain value is validated

- **WHEN** a backend table sets `drain = "yes"`
- **THEN** the config is rejected with the error `services."api.svc.emfor".backends[1].drain: drain must be true or false`

#### Scenario: Reload applies drain without resetting state

- **WHEN** a healthy backend is changed to `drain = true` and the daemon is reloaded, and later changed back to `drain = false` and reloaded
- **THEN** its check keeps running, and after the second reload it is selected again at once with its recovery time unchanged and without slow start

#### Scenario: Drain does not change the digest

- **WHEN** two hosts have the same service and one host sets `drain = true` on a backend
- **THEN** both hosts compute the same service definition digest

---
### Requirement: Configuration audit log

After each accepted reload the daemon SHALL log, at `info` level, one line for each difference between the running config and the new one, before the line saying the config was reloaded, in this order: services by name; within a service, the service-level lines, then its backend lines by backend id; then the peer group lines. Each line SHALL carry the field `change` and the fields listed for that change; the field names and the values of `change` are stable, the wording of the line is not. The changes are: `service_added` and `service_removed` (`service`, `backends`); `backend_added` and `backend_removed` (`service`, `backend`); `backend_changed` (`service`, `backend`, `field` one of `weight`, `site`, `backup`, `drain`, and `from`, `to`); `service_changed` (`service`, `field` one of `check`, `interval`, `timeout`, `rise`, `fall`, `jitter`, `on_all_down`, `balance`, `peers`, `slow_start`, `dns_ttl`, and `from`, `to`; for `check` the values are the check names only); `check_definition_changed` (`service`, `digest_from`, `digest_to`, and `resets_state`, `true` when the checker or its `check_options` differ, which restarts the state of the service's backends from `unknown`, `false` when only the digest differs, as for `timeout`); `peer_member_added` and `peer_member_removed` (`peer`); `peers_changed` (`field` one of `encrypt`, `accept_plaintext`, `secret_file`, `previous_secret_file`, and `from`, `to`, and `applies`, `now`, or `next_start` for `previous_secret_file`, which is read only at start). A `site` that is not set SHALL be written as `-`. The daemon SHALL log at most 50 such lines per reload; when there are more differences it SHALL replace the rest by one line with `change=more` and `omitted` set to their number. No line SHALL contain a `check_options` value, the content of a secret file, or a hash of it. A reload with no difference SHALL log no such line.

#### Scenario: Backend added and another drained

- **WHEN** a reload adds backend `10.0.0.13:8080` to service `api.svc.emfor` and sets `drain = true` on `10.0.0.11:8080`
- **THEN** the log has a line with `change=backend_added`, `service=api.svc.emfor`, `backend=10.0.0.13:8080`, and a line with `change=backend_changed`, `field=drain`, `from=false`, `to=true` for `10.0.0.11:8080`, both before the `config reloaded` line

#### Scenario: Check options changed without exposing them

- **WHEN** a reload changes `check_options` of a service, for example `expect = "ready"` to `expect = "ok"`
- **THEN** the log has one `change=check_definition_changed` line with `digest_from`, `digest_to` and `resets_state=true`, and no log line contains `ready` or `ok` as an option value

#### Scenario: Timeout change keeps the state

- **WHEN** a reload changes only a service's `timeout`
- **THEN** the log has a `change=service_changed` line with `field=timeout` and a `change=check_definition_changed` line with `resets_state=false`

#### Scenario: Peer group changes

- **WHEN** a reload adds a member, sets `encrypt = true`, and changes `previous_secret_file`
- **THEN** the log has `peer_member_added`, a `peers_changed` line with `field=encrypt` and `applies=now`, and a `peers_changed` line with `field=previous_secret_file` and `applies=next_start`, and no line holds the content of a secret file or a hash of it

#### Scenario: More than fifty differences

- **WHEN** a reload adds 80 services
- **THEN** the log has 50 `service_added` lines and one line with `change=more` and `omitted=30`

##### Example: change values

| Reload changes | Lines with `change` |
| --- | --- |
| nothing | none |
| service `a` removed | `service_removed` |
| backend `weight` 1 to 3 | `backend_changed` with `field=weight` |
| `[defaults] interval` 2s to 5s, three services use it | three `service_changed` lines with `field=interval` |
| `check = "tcp"` to `check = "http"` | `service_changed` with `field=check`, `check_definition_changed` with `resets_state=true` |
| `[peers] members` loses one address | `peer_member_removed` |

#### Scenario: No difference

- **WHEN** a reload applies a file with the same effective config
- **THEN** the log has the `config reloaded` line and no `change=` line

<!-- @trace
source: emfor-reload-audit-log
updated: 2026-11-23
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/audit.rs
  - crates/emfor-daemon/src/audit/tests.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/lib.rs
  - docs/manual.zh_TW/12-operating.md
  - docs/manual.zh_TW/13-security.md
  - docs/manual.zh_TW/15-upgrading.md
  - docs/manual/12-operating.md
  - docs/manual/13-security.md
  - docs/manual/15-upgrading.md
tests:
  - crates/emfor-daemon/tests/audit_log.rs
-->