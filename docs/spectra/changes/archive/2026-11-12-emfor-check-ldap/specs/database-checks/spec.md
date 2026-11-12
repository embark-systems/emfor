## MODIFIED Requirements

### Requirement: Plugin selection by argument

`emfor-check` SHALL take exactly one argument naming the plugin: `redis`, `mysql`, `pgsql` or `ldap`. With a missing, extra or unknown argument it SHALL print a usage line to stderr and exit with status 2 without answering the handshake. With a valid argument it SHALL speak checker-plugins protocol 1 and answer `hello` with `name` equal to the plugin name and `version` equal to the `emfor-check` package version. Each check SHALL open a new connection to the backend and close it before answering. A check that has not finished within the request's `timeout_ms` SHALL be answered `down` with reason `timeout`.

#### Scenario: Unknown plugin argument

- **WHEN** a config declares `[plugins."kv"]` with `command = ["/usr/local/libexec/emfor-check", "memcached"]`
- **THEN** the plugin fails to start and the config is rejected with an error naming plugin `kv`

#### Scenario: Handshake names the plugin

- **WHEN** `emfor-check redis` receives `{"type":"hello","protocol":1}`
- **THEN** it answers `{"type":"hello","protocol":1,"name":"redis","version":"<package version>"}`

#### Scenario: LDAP plugin handshake

- **WHEN** `emfor-check ldap` receives `{"type":"hello","protocol":1}`
- **THEN** it answers `{"type":"hello","protocol":1,"name":"ldap","version":"<package version>"}`

#### Scenario: Slow backend

- **WHEN** a check request has `timeout_ms` 900 and the backend accepts the TCP connection but never answers
- **THEN** the plugin answers that request within about 900 ms with `result` `down` and reason `timeout`

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

- **WHEN** `emfor --check-config` runs on a config whose `mysql` service has `check_options = {}`
- **THEN** it fails with `services."<name>".check_options.user: missing required key`

### Requirement: TLS connections

With `tls = true` a plugin SHALL connect with TLS and SHALL NOT fall back to plaintext. With `starttls = true` the `ldap` plugin SHALL connect in plaintext, send the StartTLS extended request, and on success perform the TLS handshake on the same connection before binding; when the server rejects StartTLS or answers it with anything other than success, the result SHALL be `down` with a reason starting `tls: ` and the plugin SHALL NOT bind in plaintext. In both cases the plugin SHALL verify the server certificate against the certificates in `ca_file`, or the system trust store when `ca_file` is not set, and against `server_name`, or the backend IP when `server_name` is not set; `server_name` SHALL also be sent as SNI. With `tls_verify = false` it SHALL accept any server certificate and still encrypt. A TLS handshake or certificate verification failure SHALL be `down` with a reason starting `tls: `.

#### Scenario: Name mismatch

- **WHEN** `tls = true`, `ca_file` holds the CA that signed the server certificate, and `server_name` does not match the certificate
- **THEN** the check result is `down` with a reason starting `tls: `

#### Scenario: Verification turned off

- **WHEN** `tls = true` and `tls_verify = false` and the server certificate names another host
- **THEN** the check result is `up` when the service itself is healthy

#### Scenario: Server without TLS

- **WHEN** `tls = true` and the backend port speaks plaintext
- **THEN** the check result is `down` with a reason starting `tls: `

#### Scenario: StartTLS upgrade

- **WHEN** an `ldap` service sets `starttls = true`, `server_name` and `ca_file` matching the server certificate, and the server supports StartTLS on its plaintext port
- **THEN** the bind happens over TLS and the check result is `up`

#### Scenario: StartTLS refused

- **WHEN** an `ldap` service sets `starttls = true` and the server answers the StartTLS request with a non-zero resultCode
- **THEN** the check result is `down` with a reason starting `tls: StartTLS refused: ` and no bind request is sent

##### Example: refused StartTLS

- **GIVEN** `check_options = { starttls = true, tls_verify = false }`
- **WHEN** the server answers the StartTLS extended request with resultCode 2 and an empty diagnosticMessage
- **THEN** the reason is `tls: StartTLS refused: 2 protocolError`

## ADDED Requirements

### Requirement: LDAP check

The `ldap` plugin SHALL perform an LDAPv3 simple bind and SHALL answer `up` when the BindResponse resultCode is 0. Without `user` it SHALL bind anonymously (empty name, empty password); with `user` it SHALL bind with `user` as the DN and the content of `password_file` as the password. After a successful bind it SHALL send an UnbindRequest before closing the connection. A BindResponse with a non-zero resultCode SHALL be `down` with reason `auth: <code> <name>`, followed by `: <diagnosticMessage>` when the server sent a non-empty diagnosticMessage; `<name>` SHALL be the resultCode name defined in RFC 4511 section 4.1.9 (for example `invalidCredentials` for 49), and SHALL be left out (keeping only the code) for codes RFC 4511 does not define. A response that is not a valid LDAPMessage, has another messageID, carries another protocol operation, or a Notice of Disconnection, SHALL be `down` with a reason starting `query: `. The plugin SHALL read at most 65536 bytes for one response and SHALL answer `down` with a reason starting `query: ` when a response is longer.

#### Scenario: Anonymous bind

- **WHEN** an `ldap` service sets no `user` and the server allows anonymous bind
- **THEN** the plugin sends a BindRequest with version 3, an empty name and an empty simple password, and the result is `up`

##### Example: anonymous bind bytes

- **GIVEN** `check_options = {}`
- **WHEN** the plugin sends its first message
- **THEN** the bytes are `30 0c 02 01 01 60 07 02 01 03 04 00 80 00` (LDAPMessage, messageID 1, BindRequest version 3, empty name, empty simple password)

#### Scenario: Bind with a password

- **WHEN** an `ldap` service sets `user` and `password_file` matching an entry on the server
- **THEN** the result is `up`

#### Scenario: Wrong LDAP password

- **WHEN** the password in `password_file` does not match the entry named by `user`
- **THEN** the result is `down` with reason starting `auth: 49 invalidCredentials` and the reason does not contain the password

##### Example: result codes in reasons

| resultCode | diagnosticMessage | Reason |
| --- | --- | --- |
| 49 | empty | `auth: 49 invalidCredentials` |
| 53 | `unauthenticated bind (DN with no password) disallowed` | `auth: 53 unwillingToPerform: unauthenticated bind (DN with no password) disallowed` |
| 8 | empty | `auth: 8 strongerAuthRequired` |
| 4096 | empty | `auth: 4096` |

#### Scenario: Malformed response

- **WHEN** the server answers the BindRequest with bytes that are not a BindResponse for messageID 1
- **THEN** the result is `down` with a reason starting `query: `
