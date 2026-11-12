## MODIFIED Requirements

### Requirement: TCP checker

The `tcp` checker SHALL connect to the backend address. A refused or unreachable connection SHALL be `down`. A local failure to create the socket (such as too many open files) SHALL be `error`.

The `tcp` checker SHALL accept two optional `check_options` keys and no others:

- `send`: a string whose UTF-8 bytes the checker SHALL write to the connection once, right after connecting, without any further unescaping.
- `expect`: a string whose UTF-8 bytes SHALL appear, byte for byte and case-sensitively, somewhere in the bytes the backend sends.

Validation SHALL fail when `send` or `expect` is not a string, when `send` is set without `expect` (error on `expect`: `required when send is set`), when `expect` is empty, or when `expect` is longer than 16384 bytes. An empty `send` with an `expect` SHALL behave as `expect` alone.

Without `expect`, a completed connection SHALL be `up` and the connection SHALL be closed immediately. With `expect`, the checker SHALL write `send` when set, then read from the connection and keep the received bytes, up to 16384 bytes. It SHALL answer `up` and close the connection as soon as the received bytes contain `expect`, including when `expect` spans several reads. It SHALL answer `down` with reason `expect: got "<excerpt>"` when the backend closes the connection, or 16384 bytes have been received, without `expect` appearing. The excerpt SHALL be the first 64 received bytes, escaped so that printable ASCII other than `\`, `'` and `"` stays as is; tab, carriage return and line feed become `\t`, `\r` and `\n`; `\`, `'` and `"` are preceded by `\`; and every other byte becomes `\x` followed by two lowercase hex digits. A failure while writing SHALL be `down` with a reason starting `send: `; a failure while reading, other than the backend closing the connection, SHALL be `down` with a reason starting `expect: `. A check that has not finished within the service's `timeout` SHALL be `down` with reason `timeout`, as for every checker.

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
