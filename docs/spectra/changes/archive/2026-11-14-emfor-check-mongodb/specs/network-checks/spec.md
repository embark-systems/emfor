## ADDED Requirements

### Requirement: MongoDB check options

The `mongodb` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `user`: a string of 1 to 255 bytes, otherwise `must be 1 to 255 bytes`. Without `password_file` it SHALL be an error on `user`: `requires password_file`.
- `password_file`: the rules it has for the other `emfor-check` plugins. Without `user` it SHALL be an error on `password_file`: `requires user`.
- `auth_source`: 1 to 64 ASCII letters, digits, `_` or `-`, default `admin`, otherwise `must be a database name`. Without `user` it SHALL be an error: `requires user`.
- `role`: `primary` or `secondary`, otherwise `must be one of primary, secondary`.
- `tls`, `tls_verify`, `server_name` and `ca_file`: the rules they have for the `redis`, `mysql` and `pgsql` plugins, including the error `requires tls = true`.

Type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: No credentials

- **WHEN** a service with `check = "mongodb"` sets `check_options = { role = "primary" }`
- **THEN** the plugin reports no errors and checks without logging in

##### Example: MongoDB option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | none |
| `{ user = "emfor", password_file = "/etc/emfor/mongo.pass", auth_source = "app" }` | none |
| `{ user = "emfor" }` | `user`: requires password_file |
| `{ password_file = "/etc/emfor/mongo.pass" }` | `password_file`: requires user |
| `{ auth_source = "app" }` | `auth_source`: requires user |
| `{ user = "emfor", password_file = "/etc/emfor/mongo.pass", auth_source = "a.b" }` | `auth_source`: must be a database name |
| `{ role = "replica" }` | `role`: must be one of primary, secondary |
| `{ server_name = "db.internal" }` | `server_name`: requires tls = true |
| `{ database = "app" }` | `database`: unknown key |

### Requirement: MongoDB check

The `mongodb` plugin SHALL open a new TCP connection per check (a failure is `down` with a reason starting `connect: `) and, with `tls = true`, perform the TLS handshake first (a failure is `down` with a reason starting `tls: `). It SHALL send commands as OP_MSG messages (opCode 2013, flagBits 0, one kind 0 section) and read each reply as one OP_MSG message of at most 65536 bytes, ignoring a trailing checksum when the reply's checksumPresent flag is set. A larger reply SHALL be `down` with reason `query: reply too large`; a reply that is not an OP_MSG answering the request or whose BSON cannot be decoded SHALL be `down` with reason `query: malformed reply`; the connection closing before a reply is complete SHALL be `down` with reason `query: connection closed`.

The plugin SHALL first send `{hello: 1, $db: "admin"}`. With `user` set it SHALL then authenticate with SCRAM-SHA-256 (RFC 7677) against `auth_source` using `saslStart` with `options: {skipEmptyExchange: true}` and `saslContinue`, escaping `=` and `,` in the user name as RFC 5802 requires, and SHALL verify the server signature. A password that is not printable ASCII SHALL be `error` with reason `password_file: only printable ASCII passwords are supported`. A command answered with `ok: 0` SHALL be `down` with reason `<prefix>: <codeName>: <errmsg>`, where the prefix is `auth` for `saslStart` and `saslContinue` and `query` for `hello`. A server-first-message with an iteration count below 4096, a nonce that does not start with the client nonce, or a missing field SHALL be `down` with reason `auth: malformed SCRAM message`; a server signature that does not match SHALL be `down` with reason `auth: server signature does not match`.

With `role` set, the plugin SHALL derive the actual role from the `hello` reply in this order: `arbiter` when `arbiterOnly` is true; `mongos` when `msg` is `isdbgrid`; `primary` when `isWritablePrimary` is true and `setName` is present; `standalone` when `isWritablePrimary` is true without `setName`; `secondary` when `secondary` is true; otherwise `unknown`. `role = "primary"` SHALL accept `primary`, `standalone` and `mongos`; `role = "secondary"` SHALL accept `secondary` only. Any other actual role SHALL be `down` with reason `role is <actual>, want <role>`. The result SHALL be `up` when every step succeeded and the role, if set, is accepted.

#### Scenario: Primary of a replica set

- **WHEN** `role = "primary"` and the server answers `hello` with `isWritablePrimary: true` and `setName: "rs0"`
- **THEN** the check result is `up`

#### Scenario: SCRAM example from RFC 7677

- **WHEN** the plugin authenticates user `user` with password `pencil`, client nonce `rOprNGfwEbeRWgbNEkqO`, and the server-first-message `r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096`
- **THEN** the client-final-message is `c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=` and the server signature `v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=` is accepted

##### Example: MongoDB replies

| Options | Server behavior | Result |
| --- | --- | --- |
| `{}` | `hello` ok, standalone | up |
| `{ role = "primary" }` | `hello` with `isWritablePrimary: true`, `setName: "rs0"` | up |
| `{ role = "primary" }` | `hello` with `isWritablePrimary: true`, no `setName` | up |
| `{ role = "primary" }` | `hello` with `isWritablePrimary: true`, `msg: "isdbgrid"` | up |
| `{ role = "primary" }` | `hello` with `secondary: true`, `setName: "rs0"` | down: `role is secondary, want primary` |
| `{ role = "secondary" }` | `hello` with `secondary: true`, `setName: "rs0"` | up |
| `{ role = "secondary" }` | `hello` with `isWritablePrimary: true`, no `setName` | down: `role is standalone, want secondary` |
| `{ role = "primary" }` | `hello` with `arbiterOnly: true`, `secondary: false` | down: `role is arbiter, want primary` |
| `{ role = "secondary" }` | `hello` with `isWritablePrimary: false`, `secondary: false`, `setName: "rs0"` | down: `role is unknown, want secondary` |
| `{}` | `hello` answered `ok: 0`, `codeName: "CommandNotFound"`, `errmsg: "no such command: 'hello'"` | down: `query: CommandNotFound: no such command: 'hello'` |
| user and password | `saslStart` answered `ok: 0`, `codeName: "AuthenticationFailed"`, `errmsg: "Authentication failed."` | down: `auth: AuthenticationFailed: Authentication failed.` |
| user and password | server-first-message with `i=1000` | down: `auth: malformed SCRAM message` |
| user and password | server-final-message with a wrong `v=` | down: `auth: server signature does not match` |
| user and password (`pässword`) | (no request sent after `hello`) | error: `password_file: only printable ASCII passwords are supported` |
| `{}` | a reply of 70000 bytes | down: `query: reply too large` |
| `{}` | closes the connection after the request | down: `query: connection closed` |
