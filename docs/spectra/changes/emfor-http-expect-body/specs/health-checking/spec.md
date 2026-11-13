## MODIFIED Requirements

### Requirement: HTTP checker

The `http` checker SHALL send one `GET` request per check to the backend IP and port using `scheme` (`http` or `https`, default `http`) and `path` (default `/`), with the `Host` header set to `host_header` when configured. It SHALL NOT follow redirects. The result SHALL be `up` when the response status matches any entry of `expect_status` (default `["200-399"]`; each entry is a status code or an inclusive `lo-hi` range) and `down` otherwise, with the status code in the reason. For `https`, certificate verification SHALL be on unless `tls_verify = false`. When `expect_body` is set (a string of 1 to 16384 bytes), a response whose status matches SHALL also be read, up to 65536 bytes and stopping as soon as `expect_body` is found as a case-sensitive byte substring; the result SHALL be `up` only when it is found, and otherwise `down` with reason `expect_body: got "<first 64 bytes of the body, escaped>"`. A failure while reading the body SHALL be `down` with a reason starting `body: `. Without `expect_body`, the checker SHALL NOT read the body.

#### Scenario: Status inside expected range

- **WHEN** an `http` check with default options receives status 204
- **THEN** the result is `up`

##### Example: status matching

| `expect_status` | Response status | Result |
| --- | --- | --- |
| default `["200-399"]` | 200 | up |
| default `["200-399"]` | 302 | up (redirect not followed) |
| default `["200-399"]` | 503 | down, reason contains 503 |
| `["200", "204"]` | 204 | up |
| `["200-299"]` | 301 | down, reason contains 301 |

#### Scenario: Invalid expect_status

- **WHEN** a service sets `expect_status = ["299-200"]`
- **THEN** validation fails because the range lower bound exceeds the upper bound

#### Scenario: Host header

- **WHEN** a service sets `host_header = "api.internal"`
- **THEN** each check request carries `Host: api.internal`

#### Scenario: Body must contain expect_body

- **WHEN** a service sets `expect_body = "\"status\":\"UP\""` and the backend answers 200 with body `{"status":"DOWN"}`
- **THEN** the result is `down` with reason `expect_body: got "{\"status\":\"DOWN\"}"`

##### Example: body matching

| `expect_body` | Response | Result |
| --- | --- | --- |
| (not set) | 200, body `{"status":"DOWN"}` | up |
| `"status":"UP"` | 200, body `{"status":"UP"}` | up |
| `"status":"UP"` | 200, body `{"status":"up"}` | down: `expect_body: got "{\"status\":\"up\"}"` |
| `"status":"UP"` | 503, body `{"status":"UP"}` | down: `unexpected status 503` |
| `ok` | 200, body sent in two parts `o` and `k` | up |
| `ok` | 200, 70000 bytes of `a` followed by `ok` | down: `expect_body: got "aaaa…"` (first 64 bytes) |
| `ok` | 200, empty body | down: `expect_body: got ""` |

#### Scenario: Invalid expect_body

- **WHEN** a service sets `expect_body = ""`
- **THEN** validation fails on `expect_body` with `must not be empty`

##### Example: expect_body validation

| `expect_body` | Error |
| --- | --- |
| `""` | `expect_body`: must not be empty |
| 16385 bytes | `expect_body`: must be at most 16384 bytes |
| `200` (integer) | `expect_body`: must be a string |
