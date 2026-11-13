## MODIFIED Requirements

### Requirement: Plugin selection by argument

`emfor-check` SHALL take exactly one argument naming the plugin: `redis`, `mysql`, `pgsql`, `ldap` or `dns`. With a missing, extra or unknown argument it SHALL print a usage line to stderr and exit with status 2 without answering the handshake. With a valid argument it SHALL speak checker-plugins protocol 1 and answer `hello` with `name` equal to the plugin name and `version` equal to the `emfor-check` package version. Each check of the `redis`, `mysql`, `pgsql` and `ldap` plugins SHALL open a new connection to the backend and close it before answering. A check that has not finished within the request's `timeout_ms` SHALL be answered `down` with reason `timeout`.

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

#### Scenario: Slow backend

- **WHEN** a check request has `timeout_ms` 900 and the backend accepts the TCP connection but never answers
- **THEN** the plugin answers that request within about 900 ms with `result` `down` and reason `timeout`


### Requirement: Check options validation

The `redis`, `mysql`, `pgsql` and `ldap` plugins SHALL each validate `check_options` with the keys below and SHALL report every other key as `unknown key`. `user` SHALL be required for `mysql` and `pgsql`; `database` SHALL be required for `pgsql` and SHALL be an unknown key for `redis`, `mysql` and `ldap`. `role` SHALL accept `master` or `replica` for `redis` and `primary` for `mysql` and `pgsql`, and SHALL be an unknown key for `ldap`. `starttls` SHALL be a boolean accepted by `ldap` only and SHALL be an unknown key for the other plugins. `password_file` and `ca_file` SHALL be absolute paths of readable files; `password_file` SHALL be non-empty after removing one trailing `\n` or `\r\n`; `ca_file` SHALL contain at least one PEM certificate. For `redis`, `mysql` and `pgsql`, `tls_verify`, `server_name` and `ca_file` SHALL be errors (`requires tls = true`) unless `tls = true`. For `ldap`, they SHALL be errors (`requires tls = true or starttls = true`) unless `tls = true` or `starttls = true`, and `tls = true` together with `starttls = true` SHALL be an error on `starttls` (`conflicts with tls`). For `redis`, `user` without `password_file` SHALL be an error on `user` (`requires password_file`). For `ldap`, `user` without `password_file` SHALL be an error on `user` (`requires password_file`), and `password_file` without `user` SHALL be an error on `password_file` (`requires user`).

#### Scenario: Valid options

- **WHEN** a service with `check = "pgsql"` sets `check_options = { user = "emfor", database = "app", role = "primary" }`
- **THEN** the plugin reports no errors and the config is accepted

##### Example: option validation

| Plugin | `check_options` | Errors |
| --- | --- | --- |
| mysql | `{}` | `user`: missing required key |
| pgsql | `{ user = "emfor" }` | `database`: missing required key |
| redis | `{ database = "0" }` | `database`: unknown key |
| redis | `{ role = "primary" }` | `role`: must be one of master, replica |
| mysql | `{ user = "emfor", role = "master" }` | `role`: must be primary |
| redis | `{ user = "emfor" }` | `user`: requires password_file |
| redis | `{ password_file = "redis.pass" }` | `password_file`: not an absolute path |
| redis | `{ password_file = "/etc/emfor/missing" }` | `password_file`: file not found |
| redis | `{ password_file = "/etc/emfor/empty" }` (file holds only `\n`) | `password_file`: file is empty |
| redis | `{ server_name = "redis.internal" }` | `server_name`: requires tls = true |
| redis | `{ tls = "yes" }` | `tls`: must be a boolean |
| redis | `{ tls = true, ca_file = "/etc/emfor/not-pem.txt" }` | `ca_file`: no certificate found |
| redis | `{ timeout = 1 }` | `timeout`: unknown key |
| redis | `{ starttls = true }` | `starttls`: unknown key |
| ldap | `{}` | none (anonymous bind) |
| ldap | `{ user = "cn=monitor,dc=example,dc=org" }` | `user`: requires password_file |
| ldap | `{ password_file = "/etc/emfor/ldap.pass" }` | `password_file`: requires user |
| ldap | `{ tls = true, starttls = true }` | `starttls`: conflicts with tls |
| ldap | `{ server_name = "ldap.internal" }` | `server_name`: requires tls = true or starttls = true |
| ldap | `{ starttls = "yes" }` | `starttls`: must be a boolean |
| ldap | `{ role = "primary" }` | `role`: unknown key |
| ldap | `{ database = "dc=example" }` | `database`: unknown key |

#### Scenario: Config check reports plugin errors

- **WHEN** `emfor check-config` runs on a config whose `mysql` service has `check_options = {}`
- **THEN** it fails with `services."<name>".check_options.user: missing required key`


### Requirement: Check outcome classification

The `redis`, `mysql`, `pgsql` and `ldap` plugins SHALL answer `up` only when every step succeeded and the role, if set, matches. It SHALL answer `down` when the backend refuses or drops the connection (reason starting `connect: `), times out (`timeout`), fails TLS (`tls: `), rejects the login (`auth: `), fails or answers a query unexpectedly (`query: `), or has another role (`role is <actual>, want <wanted>`). It SHALL answer `error` only for failures on the plugin's side: `password_file` or `ca_file` unreadable at check time, or a local resource limit (too many open files, out of memory).

#### Scenario: Backend stopped

- **WHEN** nothing listens on the backend port
- **THEN** the check result is `down` with a reason starting `connect: `

#### Scenario: Failover follows the primary

- **WHEN** a service has `balance = "failover"`, `check = "pgsql"`, `role = "primary"`, its first backend is a standby and its second is the primary
- **THEN** after the checks reach `rise`, a lookup of the service returns the second backend
