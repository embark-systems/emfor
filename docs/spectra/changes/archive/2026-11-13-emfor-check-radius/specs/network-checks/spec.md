## ADDED Requirements

### Requirement: RADIUS check options

The `radius` plugin SHALL accept these `check_options` keys and SHALL report every other key, including `secret` and `password`, as `unknown key`:

- `secret_file` (required): an absolute path of a readable file whose content, after removing one trailing `\n` or `\r\n`, is the non-empty shared secret.
- `request`: `status-server` or `access-request`, default `status-server`. Otherwise the error is `must be status-server or access-request`.
- `application`: `auth` or `acct`, default `auth`. Otherwise the error is `must be auth or acct`. `acct` together with `request = "access-request"` SHALL be an error on `application`: `requires request = "status-server"`.
- `user`: a string of 1 to 253 bytes, otherwise `must be 1 to 253 bytes`; `password_file`: an absolute path of a readable file, non-empty after removing one trailing `\n` or `\r\n`, holding at most 128 bytes (the longest password User-Password can carry), otherwise `password longer than 128 bytes`. With `request = "access-request"` both SHALL be required (`missing required key`); with `request = "status-server"` each of them SHALL be an error: `requires request = "access-request"`.
- `reject_is_up`: a boolean, default `false`; set with `request = "status-server"` it SHALL be an error: `requires request = "access-request"`.
- `nas_identifier`: a string of 1 to 253 bytes, default `emfor`. Otherwise the error is `must be 1 to 253 bytes`.
- `require_message_authenticator`: a boolean, default `false`.

Path errors SHALL use the messages `not an absolute path`, `file not found` and `file is empty`; type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Status-Server with a secret

- **WHEN** a service with `check = "radius"` sets `check_options = { secret_file = "/etc/emfor/radius.secret" }` and the file holds `testing123\n`
- **THEN** the plugin reports no errors and checks with Status-Server on the authentication port

##### Example: RADIUS option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | `secret_file`: missing required key |
| `{ secret_file = "radius.secret" }` | `secret_file`: not an absolute path |
| `{ secret_file = "/etc/emfor/empty" }` (file holds only `\n`) | `secret_file`: file is empty |
| `{ secret_file = "/etc/emfor/radius.secret", secret = "testing123" }` | `secret`: unknown key |
| `{ secret_file = "/etc/emfor/radius.secret", request = "access-request" }` | `user`: missing required key; `password_file`: missing required key |
| `{ secret_file = "/etc/emfor/radius.secret", user = "probe" }` | `user`: requires request = "access-request" |
| `{ secret_file = "/etc/emfor/radius.secret", request = "access-request", application = "acct", user = "probe", password_file = "/etc/emfor/probe.pass" }` | `application`: requires request = "status-server" |
| `{ secret_file = "/etc/emfor/radius.secret", reject_is_up = true }` | `reject_is_up`: requires request = "access-request" |
| `{ secret_file = "/etc/emfor/radius.secret", request = "coa" }` | `request`: must be status-server or access-request |
| `{ secret_file = "/etc/emfor/radius.secret", nas_identifier = "" }` | `nas_identifier`: must be 1 to 253 bytes |

### Requirement: RADIUS check

On every check the `radius` plugin SHALL read `secret_file`, and `password_file` with `request = "access-request"`; when a file cannot be read the result SHALL be `error` with a reason starting `secret_file: ` or `password_file: ` that names the I/O error kind. It SHALL send one request through the UDP request and response exchange: a random Identifier, a random 16-byte Request Authenticator, and the attributes Message-Authenticator first (HMAC-MD5 keyed with the shared secret over the packet with this attribute's value set to 16 zero bytes), then, with `request = "access-request"`, User-Name set to `user` and User-Password set to the password hidden as in RFC 2865 section 5.2, then NAS-Identifier set to `nas_identifier`. The Code SHALL be 12 (Status-Server) or 1 (Access-Request). A datagram of at least 20 bytes with the request's Identifier SHALL match.

The plugin SHALL judge the matching response in this order:

1. A Length field that differs from the datagram length, or an attribute whose length is below 2 or runs past the end, SHALL be `down` with reason `query: malformed response`.
2. A Response Authenticator other than MD5 of the Code, Identifier, Length, the Request Authenticator, the attributes and the shared secret SHALL be `down` with reason `auth: response authenticator does not match (wrong secret?)`.
3. A Message-Authenticator attribute whose value differs from HMAC-MD5 over the response with the Request Authenticator in place of the Response Authenticator and this attribute's value zeroed SHALL be `down` with reason `auth: Message-Authenticator does not match`. A response without Message-Authenticator SHALL be `down` with reason `auth: response has no Message-Authenticator` when `require_message_authenticator` is `true`.
4. With `application = "auth"`: Access-Accept SHALL be `up`; Access-Reject SHALL be `up` when `reject_is_up` is `true` and otherwise `down` with reason `auth: Access-Reject`; Access-Challenge SHALL be `down` with reason `auth: Access-Challenge`. With `application = "acct"`: Accounting-Response SHALL be `up`. Any other Code SHALL be `down` with reason `query: unexpected response code <N>`.

An `auth: ` reason from step 4 SHALL be followed by `: ` and the Reply-Message attributes of the response joined by single spaces when it has any, with bytes that are not valid UTF-8 replaced by U+FFFD. No reason and no stderr line SHALL contain the shared secret or the password.

#### Scenario: Server answers Status-Server

- **WHEN** a check uses the defaults and the backend answers with an Access-Accept signed with the same secret
- **THEN** the check result is `up`

#### Scenario: Wrong shared secret

- **WHEN** the backend signs its Access-Accept with a different secret than the one in `secret_file`
- **THEN** the check result is `down` with reason `auth: response authenticator does not match (wrong secret?)`

##### Example: RADIUS outcomes

| Options | Backend answer (correctly signed unless noted) | Result |
| --- | --- | --- |
| defaults | Access-Accept | `up` |
| `application = "acct"` | Accounting-Response | `up` |
| `application = "acct"` | Access-Accept | `down`, `query: unexpected response code 2` |
| `request = "access-request"` | Access-Accept | `up` |
| `request = "access-request"` | Access-Reject with Reply-Message `Unknown user` | `down`, `auth: Access-Reject: Unknown user` |
| `request = "access-request"`, `reject_is_up = true` | Access-Reject | `up` |
| `request = "access-request"` | Access-Challenge | `down`, `auth: Access-Challenge` |
| defaults | Access-Accept with a wrong Message-Authenticator | `down`, `auth: Message-Authenticator does not match` |
| `require_message_authenticator = true` | Access-Accept without Message-Authenticator | `down`, `auth: response has no Message-Authenticator` |
| defaults | Access-Accept without Message-Authenticator | `up` |
| defaults | Length field 40 in a 20-byte datagram | `down`, `query: malformed response` |

#### Scenario: Password hidden as in RFC 2865

- **WHEN** an Access-Request is built with the shared secret, Request Authenticator and password of the example packet in RFC 2865 section 7.1
- **THEN** its User-Password attribute value equals the one in that example

#### Scenario: Secret file removed

- **WHEN** `secret_file` was readable when the config was loaded and is deleted before a check
- **THEN** the check result is `error` with a reason starting `secret_file: `, and the backend is not sent a request

#### Scenario: Server without Status-Server support

- **WHEN** the backend silently drops Status-Server requests and the check has `timeout_ms` 900
- **THEN** the check result is `down` with reason `timeout`
