## ADDED Requirements

### Requirement: gRPC check options

The `grpc` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `service`: a string of 0 to 256 bytes, default the empty string (the whole server). A longer string is `must be at most 256 bytes`.
- `tls`, `tls_verify`, `server_name` and `ca_file`: the rules they have for the `redis`, `mysql` and `pgsql` plugins, including the error `requires tls = true`.

Type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Defaults

- **WHEN** a service with `check = "grpc"` sets no `check_options`
- **THEN** the plugin reports no errors and checks the whole server over plaintext HTTP/2

##### Example: gRPC option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | none |
| `{ service = "orders.v1.Orders" }` | none |
| `{ service = "" }` | none |
| `{ service = "<300 bytes>" }` | `service`: must be at most 256 bytes |
| `{ tls = true, server_name = "orders.internal" }` | none |
| `{ server_name = "orders.internal" }` | `server_name`: requires tls = true |
| `{ service = 1 }` | `service`: must be a string |
| `{ authority = "orders" }` | `authority`: unknown key |

### Requirement: gRPC check

The `grpc` plugin SHALL open a new TCP connection per check (a failure is `down` with a reason starting `connect: `). With `tls = true` it SHALL perform the TLS handshake offering ALPN `h2` (a failure is `down` with a reason starting `tls: `; a server that does not select `h2` is `down` with reason `tls: server did not select h2 (ALPN)`); without TLS it SHALL start HTTP/2 with the connection preface (prior knowledge). It SHALL send one request: `POST /grpc.health.v1.Health/Check` with `:authority` set to `server_name` when it is set and otherwise to the backend address (`ip:port`, IPv6 in brackets), headers `content-type: application/grpc`, `te: trailers` and `user-agent: emfor-check/<package version>`, and a body of one uncompressed message holding `HealthCheckRequest` with field 1 set to `service`. It SHALL then read the response headers, the body and the trailers, and close the connection.

The result SHALL be decided in this order, and the first matching rule SHALL apply:

1. An HTTP/2 connection or stream error: `down`, `query: HTTP/2: <error>`.
2. A status other than 200: `down`, `query: HTTP status <status>`.
3. A `content-type` that does not start with `application/grpc`: `down`, `query: not a gRPC response (content-type <value>)`.
4. `grpc-status` (from the trailers, or from the headers when the response has no body) other than 0: `down`, `grpc: <code> <NAME>`, followed by `: ` and the percent-decoded `grpc-message` when it is present and not empty; `<NAME>` is the gRPC status code name, or `UNKNOWN_CODE` for a code above 16.
5. No `grpc-status`: `down`, `query: no grpc-status`.
6. The message: a compressed flag of 1 is `down`, `query: compressed message`; a length above 65536 is `down`, `query: reply too large`; no message is `down`, `query: no message`; protobuf that cannot be decoded is `down`, `query: malformed message`. Fields other than 1 SHALL be skipped by wire type, and a missing field 1 SHALL read as 0.
7. The health status: 1 (SERVING) is `up`; 0, 2 and 3 are `down` with reasons `status: UNKNOWN`, `status: NOT_SERVING` and `status: SERVICE_UNKNOWN`; any other value is `down` with reason `status: <value>`.

#### Scenario: Serving

- **WHEN** the server answers 200 with `content-type: application/grpc`, a message `08 01` and trailers `grpc-status: 0`
- **THEN** the check result is `up`

#### Scenario: No health service

- **WHEN** the server answers with headers only, holding `grpc-status: 12`
- **THEN** the check result is `down` with reason `grpc: 12 UNIMPLEMENTED`

##### Example: gRPC responses

| Server behavior | Result |
| --- | --- |
| 200, message `08 01`, `grpc-status: 0` | up |
| 200, message `08 02`, `grpc-status: 0` | down: `status: NOT_SERVING` |
| 200, message `08 03`, `grpc-status: 0` | down: `status: SERVICE_UNKNOWN` |
| 200, empty message (length 0), `grpc-status: 0` | down: `status: UNKNOWN` |
| 200, message `08 07`, `grpc-status: 0` | down: `status: 7` |
| 200, message `10 05 08 01` (an unknown field 2, then field 1), `grpc-status: 0` | up |
| 200, headers only, `grpc-status: 5`, `grpc-message: unknown%20service` | down: `grpc: 5 NOT_FOUND: unknown service` |
| 200, headers only, `grpc-status: 12` | down: `grpc: 12 UNIMPLEMENTED` |
| 200, headers only, `grpc-status: 99` | down: `grpc: 99 UNKNOWN_CODE` |
| 200, message `08 01`, no trailers | down: `query: no grpc-status` |
| 200, compressed flag 1 | down: `query: compressed message` |
| 200, message length 70000 | down: `query: reply too large` |
| 200, no message, `grpc-status: 0` | down: `query: no message` |
| 200, message `08`, `grpc-status: 0` | down: `query: malformed message` |
| 404, `content-type: application/grpc` | down: `query: HTTP status 404` |
| 200, `content-type: text/html` | down: `query: not a gRPC response (content-type text/html)` |
| an HTTP/1.1 server on the port | down: reason starts with `query: HTTP/2: ` |
| `tls = true`, server selects ALPN `http/1.1` | down: `tls: server did not select h2 (ALPN)` |
