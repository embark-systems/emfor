## MODIFIED Requirements

### Requirement: Check options validation

Each plugin SHALL validate `check_options` with the keys below and SHALL report every other key as `unknown key`. `user` SHALL be required for `mysql` and `pgsql`; `database` SHALL be required for `pgsql` and SHALL be an unknown key for `redis`, `mysql` and `ldap`. `role` SHALL accept `master` or `replica` for `redis` and `primary` for `mysql` and `pgsql`, and SHALL be an unknown key for `ldap`. `starttls` SHALL be a boolean accepted by `ldap` only and SHALL be an unknown key for the other plugins. `password_file` and `ca_file` SHALL be absolute paths of readable files; `password_file` SHALL be non-empty after removing one trailing `\n` or `\r\n`; `ca_file` SHALL contain at least one PEM certificate. For `redis`, `mysql` and `pgsql`, `tls_verify`, `server_name` and `ca_file` SHALL be errors (`requires tls = true`) unless `tls = true`. For `ldap`, they SHALL be errors (`requires tls = true or starttls = true`) unless `tls = true` or `starttls = true`, and `tls = true` together with `starttls = true` SHALL be an error on `starttls` (`conflicts with tls`). For `redis`, `user` without `password_file` SHALL be an error on `user` (`requires password_file`). For `ldap`, `user` without `password_file` SHALL be an error on `user` (`requires password_file`), and `password_file` without `user` SHALL be an error on `password_file` (`requires user`).

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
