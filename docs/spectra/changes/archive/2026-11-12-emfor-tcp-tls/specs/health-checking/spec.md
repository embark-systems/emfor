## MODIFIED Requirements

### Requirement: TCP checker

The `tcp` checker SHALL connect to the backend address. A refused or unreachable connection SHALL be `down`. A local failure to create the socket (such as too many open files) SHALL be `error`.

The `tcp` checker SHALL accept these optional `check_options` keys and no others:

- `send`: a string whose UTF-8 bytes the checker SHALL write to the connection once, right after connecting (after the TLS handshake when `tls = true`), without any further unescaping.
- `expect`: a string whose UTF-8 bytes SHALL appear, byte for byte and case-sensitively, somewhere in the bytes the backend sends.
- `tls`: a boolean, default `false`. With `true` the checker SHALL perform a TLS handshake right after connecting and SHALL NOT fall back to plaintext.
- `tls_verify`: a boolean, default `true`. With `false` the checker SHALL accept any server certificate and still encrypt.
- `server_name`: a host name or IP address, default the backend IP. The server certificate SHALL match it, and it SHALL be sent as SNI when it is a host name.
- `ca_file`: an absolute path of a PEM file. The server certificate SHALL be verified against the certificates in it instead of the system's CA certificates.

Validation SHALL fail when `send` or `expect` is not a string, when `send` is set without `expect` (error on `expect`: `required when send is set`), when `expect` is empty, or when `expect` is longer than 16384 bytes; when `tls` or `tls_verify` is not a boolean (`must be a boolean`); when `server_name` is not a valid host name or IP address (`not a valid host name or IP address`); when `ca_file` is not an absolute path (`not an absolute path`), does not exist (`file not found`) or holds no PEM certificate (`no certificate found`); and when `tls_verify`, `server_name` or `ca_file` is set without `tls = true` (`requires tls = true`). An empty `send` with an `expect` SHALL behave as `expect` alone.

With `tls = true`, a failed TLS handshake or certificate verification SHALL be `down` with a reason starting `tls: `. The handshake SHALL get at most 90% of the time left before the service's `timeout`; when the backend has not completed it by then, the result SHALL be `down` with reason `tls: no answer to the TLS handshake (does the port speak TLS?)`. When less than 100 ms would be left for the handshake, the service's `timeout` SHALL apply instead. When `ca_file` cannot be read during a check, or the host has no system CA certificates and `ca_file` is not set, the result SHALL be `error` with a reason starting `ca_file: ` or `tls: ` respectively.

Without `expect`, a completed connection (with `tls = true`, a completed handshake) SHALL be `up` and the connection SHALL be closed immediately (with `tls = true`, after sending a TLS close_notify). With `expect`, the checker SHALL write `send` when set, then read from the connection and keep the received bytes, up to 16384 bytes. It SHALL answer `up` and close the connection as soon as the received bytes contain `expect`, including when `expect` spans several reads. It SHALL answer `down` with reason `expect: got "<excerpt>"` when the backend closes the connection, or 16384 bytes have been received, without `expect` appearing. The excerpt SHALL be the first 64 received bytes, escaped so that printable ASCII other than `\`, `'` and `"` stays as is; tab, carriage return and line feed become `\t`, `\r` and `\n`; `\`, `'` and `"` are preceded by `\`; and every other byte becomes `\x` followed by two lowercase hex digits. A failure while writing SHALL be `down` with a reason starting `send: `; a failure while reading, other than the backend closing the connection, SHALL be `down` with a reason starting `expect: `. A check that has not finished within the service's `timeout` SHALL be `down` with reason `timeout`, as for every checker.

#### Scenario: Listening backend

- **WHEN** a `tcp` check without `check_options` runs against a backend with a listening socket
- **THEN** the result is `up`

#### Scenario: Closed port

- **WHEN** a `tcp` check runs against a port with no listener
- **THEN** the result is `down` with a reason describing the refused connection

#### Scenario: Unexpected option

- **WHEN** a `tcp` service sets `check_options.path = "/"`
- **THEN** validation fails with an unknown key error for `check_options.path`

#### Scenario: Banner matches

- **WHEN** a `tcp` service sets only `expect` and the backend sends a greeting containing it
- **THEN** the checker sends nothing and the result is `up`

##### Example: SMTP greeting

- **GIVEN** `check_options = { expect = "220" }`
- **WHEN** the backend sends `220 mail.example ESMTP\r\n` after accepting the connection
- **THEN** the result is `up`

#### Scenario: Request and response match

- **WHEN** a `tcp` service sets `send` and `expect` and the backend answers the request with a response containing `expect`
- **THEN** the backend received exactly the bytes of `send` and the result is `up`

##### Example: memcached stats

- **GIVEN** `check_options = { send = "stats\r\n", expect = "STAT pid" }`
- **WHEN** the backend receives `stats\r\n` (the 7 bytes `73 74 61 74 73 0d 0a`) and answers `STAT pid 4242\r\n`
- **THEN** the result is `up`

#### Scenario: Expected text split across reads

- **WHEN** the backend sends the expected text in two separate writes that arrive in separate reads
- **THEN** the result is `up`

##### Example: split greeting

- **GIVEN** `check_options = { expect = "+PONG" }`
- **WHEN** the backend writes `+PO`, waits 50 ms, then writes `NG\r\n`
- **THEN** the result is `up`

#### Scenario: Backend closes without the expected text

- **WHEN** the backend closes the connection before `expect` appears in what it sent
- **THEN** the result is `down` with reason `expect: got "<excerpt>"`

##### Example: excerpts

| Backend sends, then closes | Reason |
| --- | --- |
| `ERROR\r\n` | `expect: got "ERROR\r\n"` |
| nothing | `expect: got ""` |
| `say "hi"\x00` | `expect: got "say \"hi\"\x00"` |
| 100 bytes of `a` | `expect: got "` followed by 64 `a` and `"` |

#### Scenario: Too much data without the expected text

- **WHEN** the backend sends 16384 bytes without `expect` among them and keeps the connection open
- **THEN** the result is `down` with a reason starting `expect: got "` without waiting for `timeout`

#### Scenario: Silent backend

- **WHEN** a `tcp` service sets `expect`, and the backend accepts the connection but sends nothing until the service's `timeout` passes
- **THEN** the result is `down` with reason `timeout`

#### Scenario: Invalid send and expect options

- **WHEN** a `tcp` service sets `send` or `expect` to a value that breaks the rules above
- **THEN** validation fails with an error on the named key

##### Example: validation errors

| `check_options` | Error key | Message |
| --- | --- | --- |
| `{ send = "PING\r\n" }` | `expect` | `required when send is set` |
| `{ expect = "" }` | `expect` | `must not be empty` |
| `{ expect = 220 }` | `expect` | `must be a string` |
| `{ send = ["a"], expect = "b" }` | `send` | `must be a string` |
| `expect` of 16385 bytes | `expect` | `must be at most 16384 bytes` |

#### Scenario: TLS handshake succeeds

- **WHEN** a `tcp` service sets `tls = true`, `ca_file` holds the CA that signed the backend certificate and `server_name` matches it
- **THEN** the result is `up`

#### Scenario: TLS with expected text

- **WHEN** a `tcp` service sets `tls = true` and `expect`, and the backend sends a greeting containing `expect` over TLS after the handshake
- **THEN** the result is `up`

##### Example: TLS greeting

- **GIVEN** `check_options = { tls = true, server_name = "db.test", ca_file = "/etc/emfor/ca.pem", expect = "+OK" }` and a certificate for `db.test` signed by that CA
- **WHEN** the backend completes the handshake and sends `+OK ready\r\n`
- **THEN** the result is `up`

#### Scenario: TLS name mismatch

- **WHEN** `tls = true`, `ca_file` holds the CA that signed the backend certificate, and `server_name` does not match the certificate
- **THEN** the result is `down` with a reason starting `tls: `

#### Scenario: TLS verification turned off

- **WHEN** `tls = true`, `tls_verify = false` and the backend certificate names another host and is signed by an unknown CA
- **THEN** the result is `up`

#### Scenario: TLS to a plaintext port

- **WHEN** `tls = true`, the service's `timeout` is 1 s, and the backend accepts the connection but never answers the TLS handshake
- **THEN** the result is `down` with reason `tls: no answer to the TLS handshake (does the port speak TLS?)` before the timeout passes

#### Scenario: CA file removed after validation

- **WHEN** `ca_file` is deleted after the config was applied
- **THEN** checks of that service are `error` with a reason starting `ca_file: `

#### Scenario: Invalid TLS options

- **WHEN** a `tcp` service sets a TLS option that breaks the rules above
- **THEN** validation fails with an error on the named key

##### Example: TLS validation errors

| `check_options` | Error key | Message |
| --- | --- | --- |
| `{ tls = "yes" }` | `tls` | `must be a boolean` |
| `{ server_name = "db.test" }` | `server_name` | `requires tls = true` |
| `{ tls = false, tls_verify = false }` | `tls_verify` | `requires tls = true` |
| `{ tls = true, server_name = "not a name" }` | `server_name` | `not a valid host name or IP address` |
| `{ tls = true, ca_file = "ca.pem" }` | `ca_file` | `not an absolute path` |
| `{ tls = true, ca_file = "/etc/emfor/missing.pem" }` | `ca_file` | `file not found` |
| `{ tls = true, ca_file = "/etc/emfor/not-pem.txt" }` | `ca_file` | `no certificate found` |
