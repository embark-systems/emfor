## MODIFIED Requirements

### Requirement: Check options validation

The `redis`, `mysql`, `pgsql` and `ldap` plugins SHALL each validate `check_options` with the keys below and SHALL report every other key as `unknown key`. `user` SHALL be required for `mysql` and `pgsql`; `database` SHALL be required for `pgsql` and SHALL be an unknown key for `redis`, `mysql` and `ldap`. `role` SHALL accept `master` or `replica` for `redis` and `primary` or `replica` for `mysql` and `pgsql`, and SHALL be an unknown key for `ldap`. `max_lag` SHALL be an integer from 1 to 86400 (seconds), SHALL be an error (`requires role = "replica"`) unless `role = "replica"`, and SHALL be an unknown key for `ldap`. `starttls` SHALL be a boolean accepted by `ldap` only and SHALL be an unknown key for the other plugins. `password_file` and `ca_file` SHALL be absolute paths of readable files; `password_file` SHALL be non-empty after removing one trailing `\n` or `\r\n`; `ca_file` SHALL contain at least one PEM certificate (`no certificate found`) and at least one that can be used as a trust anchor (`no usable certificate`), as a check reads it. For `mysql` and `pgsql`, `user` and `database` SHALL NOT contain NUL (`must not contain NUL`): both are sent as NUL-terminated strings. For `pgsql`, when options are validated, a `password_file` whose password contains NUL SHALL be an error on `password_file` (`password must not contain NUL`): PostgreSQL cannot store such a password and the driver refuses to send it. For `redis`, `mysql` and `pgsql`, `tls_verify`, `server_name` and `ca_file` SHALL be errors (`requires tls = true`) unless `tls = true`. For `ldap`, they SHALL be errors (`requires tls = true or starttls = true`) unless `tls = true` or `starttls = true`, and `tls = true` together with `starttls = true` SHALL be an error on `starttls` (`conflicts with tls`). For `redis`, `user` without `password_file` SHALL be an error on `user` (`requires password_file`). For `ldap`, `user` without `password_file` SHALL be an error on `user` (`requires password_file`), and `password_file` without `user` SHALL be an error on `password_file` (`requires user`).

#### Scenario: Valid options

- **WHEN** a service with `check = "pgsql"` sets `check_options = { user = "emfor", database = "app", role = "primary" }`
- **THEN** the plugin reports no errors and the config is accepted

#### Scenario: Valid replica options

- **WHEN** a service with `check = "mysql"` sets `check_options = { user = "emfor", role = "replica", max_lag = 30 }`
- **THEN** the plugin reports no errors and the config is accepted

##### Example: option validation

| Plugin | `check_options` | Errors |
| --- | --- | --- |
| mysql | `{}` | `user`: missing required key |
| pgsql | `{ user = "emfor" }` | `database`: missing required key |
| redis | `{ database = "0" }` | `database`: unknown key |
| redis | `{ role = "primary" }` | `role`: must be one of master, replica |
| mysql | `{ user = "emfor", role = "master" }` | `role`: must be one of primary, replica |
| pgsql | `{ user = "emfor", database = "app", role = "standby" }` | `role`: must be one of primary, replica |
| redis | `{ role = "replica", max_lag = 15 }` | none |
| pgsql | `{ user = "emfor", database = "app", role = "replica", max_lag = 86400 }` | none |
| mysql | `{ user = "emfor", max_lag = 30 }` | `max_lag`: requires role = "replica" |
| mysql | `{ user = "emfor", role = "primary", max_lag = 30 }` | `max_lag`: requires role = "replica" |
| redis | `{ role = "replica", max_lag = 0 }` | `max_lag`: must be an integer from 1 to 86400 |
| redis | `{ role = "replica", max_lag = 86401 }` | `max_lag`: must be an integer from 1 to 86400 |
| redis | `{ role = "replica", max_lag = "30s" }` | `max_lag`: must be an integer from 1 to 86400 |
| ldap | `{ max_lag = 30 }` | `max_lag`: unknown key |
| redis | `{ user = "emfor" }` | `user`: requires password_file |
| redis | `{ password_file = "redis.pass" }` | `password_file`: not an absolute path |
| redis | `{ password_file = "/etc/emfor/missing" }` | `password_file`: file not found |
| redis | `{ password_file = "/etc/emfor/empty" }` (file holds only `\n`) | `password_file`: file is empty |
| redis | `{ server_name = "redis.internal" }` | `server_name`: requires tls = true |
| redis | `{ tls = "yes" }` | `tls`: must be a boolean |
| redis | `{ tls = true, ca_file = "/etc/emfor/not-pem.txt" }` | `ca_file`: no certificate found |
| redis | `{ tls = true, ca_file = "/etc/emfor/broken.pem" }` (a certificate block that is not a valid certificate) | `ca_file`: no usable certificate |
| pgsql | `{ user = "em\u0000for", database = "app" }` | `user`: must not contain NUL |
| pgsql | `{ user = "emfor", database = "app", password_file = "/etc/emfor/nul.pass" }` (file holds `s3\0cret\n`) | `password_file`: password must not contain NUL |
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

#### Scenario: PostgreSQL password with NUL

- **WHEN** a service with `check = "pgsql"` sets `password_file` to a file whose password contains a NUL byte and its options are validated
- **THEN** the plugin reports `password_file`: `password must not contain NUL` and the config is refused

#### Scenario: Config check reports plugin errors

- **WHEN** `emfor check-config` runs on a config whose `mysql` service has `check_options = {}`
- **THEN** it fails with `services."<name>".check_options.user: missing required key`

### Requirement: Passwords from password_file only

A plugin SHALL accept a password only through `password_file`; a `password` key SHALL be reported as `unknown key`. The plugin SHALL read the file on every check, removing one trailing `\n` or `\r\n`, so that a changed password takes effect without a reload. When the file cannot be read during a check, the result SHALL be `error` with a reason starting `password_file: ` that names the I/O error kind. For `pgsql`, a password read during a check that contains NUL SHALL make the result `error` with reason `password_file: password must not contain NUL`, before the plugin connects. No check reason and no line the plugin writes to stderr SHALL contain the password.

#### Scenario: Password rotated

- **WHEN** the password in `password_file` and on the backend are both changed while emfor runs
- **THEN** the next check uses the new password and is `up` without a reload

#### Scenario: Password file removed after validation

- **WHEN** `password_file` is deleted after the config was applied
- **THEN** checks of that service are `error` with a reason starting `password_file: `, and the backends' health state does not count them as failures

#### Scenario: PostgreSQL password file gains a NUL after validation

- **WHEN** the `password_file` of a `pgsql` service is rewritten after the config was applied so that its password contains a NUL byte
- **THEN** checks of that service are `error` with reason `password_file: password must not contain NUL`, no connection is opened, and the backends' health state does not count them as failures
