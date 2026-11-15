## MODIFIED Requirements

### Requirement: Diameter check options

The `diameter` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `origin_host` and `origin_realm` (both required): DiameterIdentity strings of 1 to 255 characters made of ASCII letters, digits, `-` and `.`, not starting or ending with `.`. Otherwise the error is `not a valid DiameterIdentity`.
- `auth_application_ids`, `acct_application_ids` and `supported_vendor_ids`: arrays of integers from 0 to 4294967295, default empty. Otherwise the error is `must be an array of integers from 0 to 4294967295`.
- `vendor_specific_application_ids`: an array of tables, default empty, each with exactly the keys `vendor_id` and one of `auth_application_id` or `acct_application_id`, all integers from 0 to 4294967295. Otherwise the error is `each entry needs vendor_id and one of auth_application_id, acct_application_id`.
- `host_ip_address`: an IPv4 or IPv6 address. Otherwise the error is `not an IP address`.
- `tls`, `tls_verify`, `server_name` and `ca_file` with the rules they have for the other `emfor-check` plugins, including `requires tls = true`.
- `tls_cert_file` and `tls_key_file`: absolute paths of a PEM client certificate chain and of its PEM private key, set together and only with `tls = true` (otherwise `requires tls = true`). When only one is set, the error is on that key: `requires tls_key_file` or `requires tls_cert_file`. A path that is not absolute is `not an absolute path`; when options are validated, a file that cannot be read is `file not found` or `cannot read: <kind>`, a certificate file without a PEM certificate is `no certificate found`, and a key file without a PEM private key is `no private key found`. When both files pass, validation SHALL build the TLS client configuration a check builds with them; a key that cannot be used or does not belong to the certificate SHALL be an error on `tls_key_file`. During checks, both files SHALL be handled like `ca_file` (spec health-checking): a replaced or rewritten file takes effect at the next check without a reload, an unchanged one is kept parsed and costs one `stat` per check, and one no check used for 10 minutes is released.

When the other options are valid, validation SHALL build the Capabilities-Exchange-Request a check sends, with `host_ip_address` or, when it is not set, an IPv6 Host-IP-Address (the longer form). When that request is longer than 16777215 bytes, the largest value the 24-bit Message Length holds, the plugin SHALL report an error on the longest of `auth_application_ids`, `acct_application_ids`, `vendor_specific_application_ids` and `supported_vendor_ids`: `too many entries: the Capabilities-Exchange-Request would be <n> bytes, above Diameter's 16777215-byte message limit`, where `<n>` is the length.

When `auth_application_ids`, `acct_application_ids` and `vendor_specific_application_ids` are all empty or absent, the plugin SHALL report an error on `auth_application_ids`: `at least one application is required`. A missing `origin_host` or `origin_realm` SHALL be `missing required key`; type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Gx peer options

- **WHEN** a service with `check = "diameter"` sets `check_options = { origin_host = "emfor-app01.example.org", origin_realm = "example.org", vendor_specific_application_ids = [{ vendor_id = 10415, auth_application_id = 16777238 }], supported_vendor_ids = [10415] }`
- **THEN** the plugin reports no errors

#### Scenario: Request longer than the Message Length

- **WHEN** a service with `check = "diameter"` sets `origin_host = "emfor.example.org"`, `origin_realm = "example.org"`, `auth_application_ids = [4]` and the most `supported_vendor_ids` for which the Capabilities-Exchange-Request, counted with an IPv6 Host-IP-Address, is at most 16777215 bytes
- **THEN** the plugin reports no errors, and with one entry more it reports `supported_vendor_ids`: `too many entries: the Capabilities-Exchange-Request would be <n> bytes, above Diameter's 16777215-byte message limit`, where `<n>` is the length with that entry

##### Example: Diameter option validation

| `check_options` | Errors |
| --- | --- |
| `{ origin_realm = "example.org", auth_application_ids = [4] }` | `origin_host`: missing required key |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org" }` | `auth_application_ids`: at least one application is required |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", auth_application_ids = [4] }` | none |
| `{ origin_host = "emfor example", origin_realm = "example.org", auth_application_ids = [4] }` | `origin_host`: not a valid DiameterIdentity |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", auth_application_ids = [-1] }` | `auth_application_ids`: must be an array of integers from 0 to 4294967295 |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", vendor_specific_application_ids = [{ vendor_id = 10415 }] }` | `vendor_specific_application_ids`: each entry needs vendor_id and one of auth_application_id, acct_application_id |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", acct_application_ids = [3], host_ip_address = "emfor01" }` | `host_ip_address`: not an IP address |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", acct_application_ids = [3], ca_file = "/etc/emfor/ca.pem" }` | `ca_file`: requires tls = true |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", acct_application_ids = [3], tls = true, tls_cert_file = "/etc/emfor/diameter.crt" }` | `tls_cert_file`: requires tls_key_file |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", acct_application_ids = [3], tls_cert_file = "/etc/emfor/diameter.crt", tls_key_file = "/etc/emfor/diameter.key" }` | `tls_cert_file`: requires tls = true; `tls_key_file`: requires tls = true |
| `{ origin_host = "emfor.example.org", origin_realm = "example.org", acct_application_ids = [3], tls = true, tls_cert_file = "/etc/emfor/diameter.crt", tls_key_file = "/etc/emfor/other.key" }`, the key of another certificate | `tls_key_file`: the TLS library's message |

### Requirement: AMQP check options

The `amqp` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `user`: required, a string of 1 to 255 bytes without NUL. A missing key is `missing required key`; an empty or longer string is `must be 1 to 255 bytes`; one with NUL is `must not contain NUL`, since PLAIN separates the user from the password with NUL.
- `password_file`: required, with the rules it has for the other `emfor-check` plugins; when options are validated, a password with NUL is `password must not contain NUL`. Start-Ok carries the password and is sent before Connection.Tune sets the frame size, so it SHALL fit in AMQP 0-9-1's frame-min-size of 4096 bytes: when options are validated, a password for which the Start-Ok frame (header, method, arguments and frame end) with this `user` is longer than 4096 bytes is `password too long: Start-Ok must fit in AMQP's 4096-byte first frame, at most <n> bytes with this user`, where `<n>` is the longest password that fits. A password read during a check that does not fit SHALL make the result `error` with that message after `password_file: `, before the plugin connects.
- `vhost`: a string of 1 to 255 bytes, default `/`. Otherwise the error is `must be 1 to 255 bytes`.
- `tls`, `tls_verify`, `server_name` and `ca_file`: the rules they have for the `redis`, `mysql` and `pgsql` plugins, including the error `requires tls = true`.

Type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Minimal options

- **WHEN** a service with `check = "amqp"` sets `check_options = { user = "emfor", password_file = "/etc/emfor/amqp.pass" }`
- **THEN** the plugin reports no errors and opens vhost `/`

#### Scenario: Password too long for the first frame

- **WHEN** a service with `check = "amqp"` sets a `user` and a `password_file` whose password is one byte longer than the longest one whose Start-Ok frame with that user is 4096 bytes
- **THEN** the plugin reports `password_file`: `password too long: Start-Ok must fit in AMQP's 4096-byte first frame, at most <n> bytes with this user`, and a password of `<n>` bytes is accepted, and a check with it against RabbitMQ is `up` when the broker knows that user and password

#### Scenario: Password file grows after validation

- **WHEN** the `password_file` of an `amqp` service is rewritten after the config was applied with a 5000-byte password
- **THEN** checks of that service are `error` with a reason starting `password_file: password too long`, and no connection is opened

##### Example: AMQP option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | `user`: missing required key; `password_file`: missing required key |
| `{ user = "emfor" }` | `password_file`: missing required key |
| `{ user = "emfor", password_file = "/etc/emfor/amqp.pass", vhost = "orders" }` | none |
| `{ user = "emfor", password_file = "/etc/emfor/amqp.pass", vhost = "" }` | `vhost`: must be 1 to 255 bytes |
| `{ user = "", password_file = "/etc/emfor/amqp.pass" }` | `user`: must be 1 to 255 bytes |
| `{ user = "em\u0000for", password_file = "/etc/emfor/amqp.pass" }` | `user`: must not contain NUL |
| `{ user = "emfor", password_file = "/etc/emfor/amqp.pass", server_name = "mq.internal" }` | `server_name`: requires tls = true |
| `{ user = "emfor", password_file = "/etc/emfor/amqp.pass", password = "x" }` | `password`: unknown key |
