## MODIFIED Requirements

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

### Requirement: Config validation rules

The daemon SHALL reject a config that violates any of these rules and SHALL report every violation, each prefixed with its TOML path:

- `interval` at least 100ms; `timeout` greater than zero and not greater than `interval` (equal is allowed, because checks of one backend run one after another and cannot overlap); `rise` at least 1; `fall` at least 1; `jitter` from 0% to 50%; `on_all_down` is `open` or `closed`; `balance` is `round-robin`, `sticky`, `failover`, `weighted`, `locality`, or `fastest`; `dns_ttl` is a whole number of seconds from 0s to 3600s; `peers` is a boolean.
- `backends` is non-empty; every entry is a string or a table; a string, and a table's `addr`, is `IPv4:port` or `[IPv6]:port` with port 1–65535; a table has `addr` and no keys other than `addr`, `weight`, `site`; `weight` is an integer from 1 to 100; no `addr` repeats within a service.
- The top-level `site` and every backend `site` are, after ASCII lowercasing, 1 to 63 characters of `[a-z0-9-]` that do not start or end with `-`.
- `check` names a registered checker and `check_options` passes that checker's validation.
- `[dns] listen` is a non-empty array of `IPv4:port` or `[IPv6]:port` with port 1–65535 and no repeats; `[dns] zone` follows the service name rules after the same normalization and is at most 242 characters, so that the SOA name `hostmaster.<zone>` fits in 253; `[dns] ttl` is a whole number of seconds from 0s to 3600s.
- `[metrics] listen` is a non-empty array of `IPv4:port` or `[IPv6]:port` with port 1–65535 and no repeats; `[metrics]` has no keys other than `listen`.
- `[peers]` follows the peer group configuration rules (spec peer-group), and the top-level `site` is set when `[peers]` is present.
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
| `[dns] zone` | a valid name of 243 characters | error: zone name longer than 242 characters |
| `[dns] zone` | `"SVC.Emfor."` | ok, normalized to `svc.emfor` |
| `[metrics] listen` | `[]` | error: listen must not be empty |
| `[metrics] listen` | `["localhost:5301"]` | error: listen address must be an IP literal with port |
| `[metrics] listen` | `["127.0.0.1:5301", "127.0.0.1:5301"]` | error: duplicate listen address |
| `[metrics] path` | `"/m"` | error: unknown key |
| `[peers]` | present, no top-level `site` | error: site is required when [peers] is set |
| `[peers] members` | `["10.0.0.5:5302"]` | error: members must list 2 to 64 addresses |
| `peers` | `false` | ok |
| `peers` | `"no"` | error: peers must be true or false |
| `peers` (no `[peers]` table) | `false` | ok |


### Requirement: Reload on SIGHUP applies only a fully valid config

On `SIGHUP` the daemon SHALL re-read the config file. If the new config passes validation, the daemon SHALL apply it. If any validation error exists, the daemon SHALL log every error at `error` level and continue running the previous config unchanged. A change to any key in the `[store]` table SHALL be a validation error on reload. A change to `[dns] listen`, including adding or removing the `[dns]` table, SHALL be a validation error on reload with the message "dns listen settings require restart". A change to `[metrics] listen`, including adding or removing the `[metrics]` table, SHALL be a validation error on reload with the message "metrics listen settings require restart". A change to `[peers] listen`, including adding or removing the `[peers]` table, SHALL be a validation error on reload with the message "peers listen settings require restart". Changes to `[dns] zone`, `[dns] ttl`, `dns_ttl`, `balance`, `[peers] members`, `[peers] secret_file` and `peers` SHALL take effect on reload. A change to `peers` alone SHALL keep the state of the service's backends.

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

