## ADDED Requirements

### Requirement: QUIC check options

The `quic` plugin SHALL accept these `check_options` keys and SHALL report every other key, including `tls`, as `unknown key`:

- `alpn`: a non-empty array of strings of 1 to 255 bytes each, default `["h3"]`. An empty array or a value that is not an array is `must be a non-empty array`; an element that is not a string of 1 to 255 bytes is `must be strings of 1 to 255 bytes`.
- `tls_verify`, `server_name` and `ca_file`: the rules they have for the `grpc` plugin, except that they never require `tls = true`, because QUIC is always encrypted.
- `http3`: a boolean, default `false`. `true` while `alpn` does not contain `h3` is `requires "h3" in alpn`.
- `path`: a string starting with `/`, default `/`; otherwise `must start with /`. It SHALL be a URL path with an optional query that parses unchanged; a path with a space, a control character or `#` is `must be a URL path without spaces, control characters or #`.
- `host_header`: a string of 1 to 256 bytes that is a host with an optional port, as a URI authority without userinfo (RFC 9114 section 4.3.1); otherwise `must be a host with an optional port, 1 to 256 bytes`.
- `expect_status`: a non-empty array whose entries are status codes (integers or strings) or inclusive `"lo-hi"` ranges with `lo` not greater than `hi`, all from 100 to 599, default `["200-399"]`. An empty array or a value that is not an array is `must be a non-empty array`; an invalid entry is `must be a status code from 100 to 599 or a range lo-hi`.
- `expect_body`: a string of 1 to 16384 bytes; otherwise `must be 1 to 16384 bytes`.

`path`, `host_header`, `expect_status` and `expect_body` SHALL be reported as `requires http3 = true` when `http3` is not `true`. Type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Defaults

- **WHEN** a service with `check = "quic"` sets no `check_options`
- **THEN** the plugin reports no errors and each check is a QUIC handshake offering ALPN `h3` with certificate verification on

#### Scenario: HTTP option without http3

- **WHEN** a service with `check = "quic"` sets `check_options = { path = "/healthz" }`
- **THEN** validation reports `path`: requires http3 = true

##### Example: QUIC option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | none |
| `{ alpn = ["h3", "hq-interop"] }` | none |
| `{ alpn = [] }` | `alpn`: must be a non-empty array |
| `{ alpn = [""] }` | `alpn`: must be strings of 1 to 255 bytes |
| `{ alpn = "h3" }` | `alpn`: must be a non-empty array |
| `{ http3 = true, path = "/healthz", expect_status = [200, "204-299"], expect_body = "ok" }` | none |
| `{ http3 = true, alpn = ["doq"] }` | `http3`: requires "h3" in alpn |
| `{ http3 = true, path = "healthz" }` | `path`: must start with / |
| `{ http3 = true, expect_status = ["299-200"] }` | `expect_status`: must be a status code from 100 to 599 or a range lo-hi |
| `{ http3 = true, expect_status = [] }` | `expect_status`: must be a non-empty array |
| `{ http3 = true, host_header = "a\nb" }` | `host_header`: must be a host with an optional port, 1 to 256 bytes |
| `{ http3 = true, host_header = "api.internal/x" }` | `host_header`: must be a host with an optional port, 1 to 256 bytes |
| `{ http3 = true, host_header = "api.internal:8443" }` | none |
| `{ http3 = true, path = "/a b" }` | `path`: must be a URL path without spaces, control characters or # |
| `{ expect_body = "ok" }` | `expect_body`: requires http3 = true |
| `{ server_name = "web.internal", tls_verify = false }` | none |
| `{ tls = true }` | `tls`: unknown key |
| `{ http3 = "yes" }` | `http3`: must be a boolean |

### Requirement: QUIC check

The `quic` plugin SHALL open a new QUIC version 1 connection per check to the backend's IP and UDP port, without 0-RTT, offering the ALPN values of `alpn` in order and verifying the certificate with TLS 1.3 against `server_name` (or the backend IP when it is not set) unless `tls_verify = false`. All checks of one plugin process SHALL share one local UDP socket per address family, opened when first needed. With `http3 = false`, a completed handshake whose negotiated ALPN is in `alpn` SHALL be `up`, and the plugin SHALL then close the connection with application error code 0. With `http3 = true`, the plugin SHALL then send one HTTP/3 `GET` request for `path` with `:authority` set to `host_header`, else to `server_name` (an IPv6 address in brackets), else to the backend address (`ip:port`, IPv6 in brackets), and `user-agent: emfor-check/<package version>`; it SHALL NOT follow redirects. When `expect_body` is set and the status matches, it SHALL read the body up to 65536 bytes, stopping as soon as `expect_body` is found as a case-sensitive byte substring; without `expect_body` it SHALL NOT read the body. It SHALL then close the connection with `H3_NO_ERROR` (0x100).

The result SHALL be decided in this order, and the first matching rule SHALL apply:

1. The check has not finished within `timeout_ms`: `down`, `timeout`.
2. The local UDP socket cannot be opened, or `ca_file` or the system CA certificates cannot be read: `error`, with a reason naming the failure.
3. Version negotiation fails: `down`, `quic: version negotiation failed`.
4. The TLS handshake fails: `down`, a reason starting `tls: `; a server that shares no ALPN value is `tls: no application protocol (ALPN)`.
5. The connection is closed by the peer, also while the HTTP/3 request or the body is in progress: `down`, `quic: closed by peer: <code>`, followed by `: ` and the peer's reason text when it is not empty; a stateless reset is `quic: stateless reset`; any other connection error that the plugin did not cause, such as an idle timeout, is `down` with a reason starting `quic: `.
6. The negotiated ALPN is not in `alpn`: `down`, `tls: server selected ALPN <value> not in alpn`.
7. `http3` is `false`: `up`.
8. Any other HTTP/3 connection or stream error: `down`, `query: HTTP/3: <error>`.
9. The status matches no entry of `expect_status`: `down`, `unexpected status <status>`.
10. A failure while reading the body: `down`, a reason starting `body: `; `expect_body` not found: `down`, `expect_body: got "<first 64 bytes of the body, escaped>"`.
11. Otherwise: `up`.

#### Scenario: Handshake only

- **WHEN** a `quic` check with default options reaches a server that completes the handshake with ALPN `h3` and a certificate valid for the backend IP
- **THEN** the check result is `up` and the plugin closes the connection with application error code 0

#### Scenario: Nothing answers on the UDP port

- **WHEN** a check request has `timeout_ms` 900 and nothing answers on the backend's UDP port
- **THEN** the plugin answers that request within about 900 ms with `result` `down` and reason `timeout`

#### Scenario: HTTP/3 health endpoint

- **WHEN** a `quic` check sets `http3 = true` and `path = "/healthz"` and the server answers status 200
- **THEN** the check result is `up`

##### Example: QUIC and HTTP/3 outcomes

| Options | Server behavior | Result |
| --- | --- | --- |
| default | handshake with ALPN `h3`, trusted certificate | up |
| `alpn = ["hq-interop"]` | supports only `h3` | down: `tls: no application protocol (ALPN)` |
| default | certificate from an untrusted CA | down: reason starts with `tls: ` |
| `tls_verify = false` | certificate from an untrusted CA | up |
| `server_name = "other.internal"` | certificate only for `web.internal` | down: reason starts with `tls: ` |
| default | refuses the connection (`CONNECTION_REFUSED`, no reason) | down: `quic: closed by peer: 2` |
| `http3 = true` | completes the handshake, then closes the connection with application code 0x10 and reason `busy` | down: `quic: closed by peer: 16: busy` |
| default | nothing listens on the UDP port | down: `timeout` |
| `http3 = true` | 200 | up |
| `http3 = true` | 302 to `/login` | up (redirect not followed) |
| `http3 = true` | 503 | down: `unexpected status 503` |
| `http3 = true, expect_body = "ok"` | 200, body `all ok` | up |
| `http3 = true, expect_body = "ok"` | 200, body `fail` | down: `expect_body: got "fail"` |
| `http3 = true, expect_body = "ok"` | 503, body `ok` | down: `unexpected status 503` |
| `http3 = true, host_header = "api.internal"` | records the request | `:authority` is `api.internal` |
| `http3 = true` | resets the request stream | down: reason starts with `query: HTTP/3: ` |
| `http3 = true, expect_body = "ok"` | 200, then resets the stream before the body | down: reason starts with `body: ` |
| `http3 = true, expect_body = "ok"` | 200 and body `partial`, then closes the connection with application code 0x10 and reason `busy` | down: `quic: closed by peer: 16: busy` |
