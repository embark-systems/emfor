## MODIFIED Requirements

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
