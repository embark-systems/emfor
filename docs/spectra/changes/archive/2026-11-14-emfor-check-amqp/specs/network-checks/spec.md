## ADDED Requirements

### Requirement: AMQP check options

The `amqp` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `user`: required, a string of 1 to 255 bytes. A missing key is `missing required key`; an empty or longer string is `must be 1 to 255 bytes`.
- `password_file`: required, with the rules it has for the other `emfor-check` plugins.
- `vhost`: a string of 1 to 255 bytes, default `/`. Otherwise the error is `must be 1 to 255 bytes`.
- `tls`, `tls_verify`, `server_name` and `ca_file`: the rules they have for the `redis`, `mysql` and `pgsql` plugins, including the error `requires tls = true`.

Type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Minimal options

- **WHEN** a service with `check = "amqp"` sets `check_options = { user = "emfor", password_file = "/etc/emfor/amqp.pass" }`
- **THEN** the plugin reports no errors and opens vhost `/`

##### Example: AMQP option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | `user`: missing required key; `password_file`: missing required key |
| `{ user = "emfor" }` | `password_file`: missing required key |
| `{ user = "emfor", password_file = "/etc/emfor/amqp.pass", vhost = "orders" }` | none |
| `{ user = "emfor", password_file = "/etc/emfor/amqp.pass", vhost = "" }` | `vhost`: must be 1 to 255 bytes |
| `{ user = "", password_file = "/etc/emfor/amqp.pass" }` | `user`: must be 1 to 255 bytes |
| `{ user = "emfor", password_file = "/etc/emfor/amqp.pass", server_name = "mq.internal" }` | `server_name`: requires tls = true |
| `{ user = "emfor", password_file = "/etc/emfor/amqp.pass", password = "x" }` | `password`: unknown key |

### Requirement: AMQP check

The `amqp` plugin SHALL open a new TCP connection per check (a failure is `down` with a reason starting `connect: `), perform the TLS handshake first when `tls = true` (a failure is `down` with a reason starting `tls: `), and speak AMQP 0-9-1 on it:

1. Send the protocol header `AMQP` followed by the bytes 0, 0, 9, 1.
2. Receive Connection.Start. When its mechanisms do not include `PLAIN`, the result SHALL be `down` with reason `auth: server does not offer PLAIN`.
3. Send Connection.Start-Ok with mechanism `PLAIN`, response `\0<user>\0<password>`, locale `en_US`, and client properties containing `product` = `emfor` and a `capabilities` table with `authentication_failure_close` = true.
4. Receive Connection.Tune and send Connection.Tune-Ok with the proposed channel-max, the proposed frame-max (131072 when it is 0) and heartbeat 0.
5. Send Connection.Open with `vhost` and receive Connection.Open-Ok; the result is then `up`.
6. Send Connection.Close with reply code 200, wait up to 200 ms for Connection.Close-Ok, and close the connection.

Each frame SHALL be read as type, channel, size, payload and the end byte `0xCE`. Heartbeat frames SHALL be skipped. A size above 131072 SHALL be `down` with reason `query: frame too large`; a wrong end byte or a payload that cannot be decoded SHALL be `down` with reason `query: malformed frame`; the connection closing before the expected frame SHALL be `down` with reason `query: connection closed`, except after Start-Ok, where it SHALL be `auth: connection closed after login`. When the server answers the protocol header with its own protocol header, the result SHALL be `down` with reason `query: server speaks AMQP <a>-<b>-<c>`, the last three bytes of that header. Another method than the expected one SHALL be `down` with reason `query: unexpected method <class-id>.<method-id>`.

When the server sends Connection.Close, the plugin SHALL answer Connection.Close-Ok, close the connection, and the result SHALL be `down` with reason `<prefix>: <reply-code> <reply-text>`, where the prefix is `auth` when the Close arrives after Start-Ok and before Tune, `vhost` when it arrives after Open, and `query` otherwise.

#### Scenario: Successful handshake

- **WHEN** a broker offers `PLAIN AMQPLAIN`, accepts the login, proposes channel-max 2047, frame-max 131072 and heartbeat 60, and answers Open for vhost `/` with Open-Ok
- **THEN** the check result is `up`, and the plugin sent Tune-Ok with channel-max 2047, frame-max 131072 and heartbeat 0

#### Scenario: Wrong password

- **WHEN** the broker answers Start-Ok with Connection.Close 403 `ACCESS_REFUSED - Login was refused using authentication mechanism PLAIN`
- **THEN** the check result is `down` with reason `auth: 403 ACCESS_REFUSED - Login was refused using authentication mechanism PLAIN`, and the reason does not contain the password

##### Example: AMQP broker answers

| Broker behavior | Result |
| --- | --- |
| Start, Tune, Open-Ok | up |
| Start with mechanisms `AMQPLAIN` | down: `auth: server does not offer PLAIN` |
| Start, then Close 403 `ACCESS_REFUSED - Login was refused` after Start-Ok | down: `auth: 403 ACCESS_REFUSED - Login was refused` |
| Start, then closes the connection after Start-Ok | down: `auth: connection closed after login` |
| Start, Tune, then Close 530 `NOT_ALLOWED - vhost orders not found` after Open | down: `vhost: 530 NOT_ALLOWED - vhost orders not found` |
| a heartbeat frame between Tune and Open-Ok | up |
| `AMQP` 0 1 0 0 instead of Start | down: `query: server speaks AMQP 1-0-0` |
| a frame with size 200000 | down: `query: frame too large` |
| a frame ending in `0x00` | down: `query: malformed frame` |
| Connection.Secure (10.20) instead of Tune | down: `query: unexpected method 10.20` |
| closes the connection before Start | down: `query: connection closed` |
| Start, Tune, Open-Ok, no Close-Ok within 200 ms | up |
