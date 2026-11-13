## MODIFIED Requirements

### Requirement: Plugin selection by argument

`emfor-check` SHALL take exactly one argument naming the plugin: `redis`, `mysql`, `pgsql`, `ldap`, `dns`, `radius` or `sip`. With a missing, extra or unknown argument it SHALL print a usage line to stderr and exit with status 2 without answering the handshake. With a valid argument it SHALL speak checker-plugins protocol 1 and answer `hello` with `name` equal to the plugin name and `version` equal to the `emfor-check` package version. Each check of the `redis`, `mysql`, `pgsql` and `ldap` plugins SHALL open a new connection to the backend and close it before answering. A check that has not finished within the request's `timeout_ms` SHALL be answered `down` with reason `timeout`.

#### Scenario: Unknown plugin argument

- **WHEN** a config declares `[plugins."kv"]` with `command = ["/usr/local/libexec/emfor-check", "memcached"]`
- **THEN** the plugin fails to start and the config is rejected with an error naming plugin `kv`

#### Scenario: Handshake names the plugin

- **WHEN** `emfor-check redis` receives `{"type":"hello","protocol":1}`
- **THEN** it answers `{"type":"hello","protocol":1,"name":"redis","version":"<package version>"}`

#### Scenario: LDAP plugin handshake

- **WHEN** `emfor-check ldap` receives `{"type":"hello","protocol":1}`
- **THEN** it answers `{"type":"hello","protocol":1,"name":"ldap","version":"<package version>"}`

#### Scenario: DNS plugin handshake

- **WHEN** `emfor-check dns` receives `{"type":"hello","protocol":1}`
- **THEN** it answers `{"type":"hello","protocol":1,"name":"dns","version":"<package version>"}`

#### Scenario: RADIUS plugin handshake

- **WHEN** `emfor-check radius` receives `{"type":"hello","protocol":1}`
- **THEN** it answers `{"type":"hello","protocol":1,"name":"radius","version":"<package version>"}`

#### Scenario: SIP plugin handshake

- **WHEN** `emfor-check sip` receives `{"type":"hello","protocol":1}`
- **THEN** it answers `{"type":"hello","protocol":1,"name":"sip","version":"<package version>"}`

#### Scenario: Slow backend

- **WHEN** a check request has `timeout_ms` 900 and the backend accepts the TCP connection but never answers
- **THEN** the plugin answers that request within about 900 ms with `result` `down` and reason `timeout`
