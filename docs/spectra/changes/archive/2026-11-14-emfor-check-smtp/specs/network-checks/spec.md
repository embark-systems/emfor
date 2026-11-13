## ADDED Requirements

### Requirement: SMTP check options

The `smtp` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `helo`: a host name of 1 to 255 bytes made of ASCII letters, digits, `-` and `.`. Otherwise the error is `must be a host name`. When it is not set, the EHLO argument SHALL be the local address of the connection as an address literal: `[<IPv4>]` or `[IPv6:<IPv6>]`.
- `tls` and `starttls`: booleans, default `false`. Both `true` SHALL be an error on `starttls`: `conflicts with tls`.
- `tls_verify`, `server_name` and `ca_file`: the rules they have for the other `emfor-check` plugins, except that without `tls = true` or `starttls = true` each SHALL be an error: `requires tls = true or starttls = true`.

Type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Defaults

- **WHEN** a service with `check = "smtp"` sets no `check_options` and the plugin connects from `10.0.0.5`
- **THEN** the plugin reports no errors and sends `EHLO [10.0.0.5]` in plaintext

##### Example: SMTP option validation

| `check_options` | Errors |
| --- | --- |
| `{}` | none |
| `{ helo = "app1.example.org" }` | none |
| `{ helo = "bad name" }` | `helo`: must be a host name |
| `{ helo = "" }` | `helo`: must be a host name |
| `{ starttls = true, ca_file = "/etc/emfor/ca.pem" }` | none |
| `{ tls = true, starttls = true }` | `starttls`: conflicts with tls |
| `{ server_name = "mx.example.org" }` | `server_name`: requires tls = true or starttls = true |
| `{ starttls = "yes" }` | `starttls`: must be a boolean |
| `{ user = "emfor" }` | `user`: unknown key |

### Requirement: SMTP check

The `smtp` plugin SHALL open a new TCP connection per check (a failure is `down` with a reason starting `connect: `) and, with `tls = true`, perform the TLS handshake first. It SHALL then read the greeting, send `EHLO <helo>`, and with `starttls = true` send `STARTTLS`, perform the TLS handshake on the same connection and send `EHLO <helo>` again; finally it SHALL send `QUIT` and close the connection without waiting for the answer. Commands SHALL end in CRLF. A reply SHALL be read line by line, each line ending in LF with an optional CR before it, a line starting with three digits and `-` continuing the reply and a line starting with the same three digits and a space or nothing else ending it. A line longer than 2048 bytes or a reply longer than 65536 bytes SHALL be `down` with reason `query: reply too long`; a line that does not start with three digits, or a continuation with another code, SHALL be `down` with reason `query: malformed reply`; the connection closing before a reply ends SHALL be `down` with reason `query: connection closed`. The result SHALL be `up` only when the greeting is 220 and each EHLO is answered 250, and with `starttls = true` the first EHLO reply has a line whose text after the code is `STARTTLS` (case-insensitive), `STARTTLS` is answered 220 and the TLS handshake succeeds. Otherwise it SHALL be `down` with these reasons, where `<code> <text>` is the code and text of the reply's first line:

- greeting other than 220: `greeting: <code> <text>`;
- EHLO other than 250: `ehlo: <code> <text>`;
- STARTTLS not in the EHLO reply: `tls: STARTTLS not offered`, without sending `STARTTLS`;
- STARTTLS answered with another code: `tls: STARTTLS refused: <code> <text>`;
- a TLS handshake or certificate failure: a reason starting `tls: `.

The plugin SHALL NOT continue in plaintext after `STARTTLS` fails.

#### Scenario: Healthy server with STARTTLS

- **WHEN** `starttls = true`, the server greets with `220 mx ESMTP`, answers EHLO with `250-mx`, `250-STARTTLS` and `250 8BITMIME`, answers STARTTLS with `220 ready`, completes the TLS handshake and answers the second EHLO with 250
- **THEN** the check result is `up`

#### Scenario: STARTTLS not offered

- **WHEN** `starttls = true` and the EHLO reply has no `STARTTLS` line
- **THEN** the check result is `down` with reason `tls: STARTTLS not offered` and the plugin sends no `STARTTLS` command

##### Example: SMTP replies

| Options | Server behavior | Result |
| --- | --- | --- |
| `{}` | `220 mx ESMTP`, EHLO `250 mx` | up |
| `{}` | `554 5.3.2 no service` | down: `greeting: 554 5.3.2 no service` |
| `{}` | `421 4.3.2 busy` | down: `greeting: 421 4.3.2 busy` |
| `{}` | `220-mx`, `220 ESMTP` (two lines), EHLO `250 mx` | up |
| `{}` | `220 mx`, EHLO `550 5.7.1 go away` | down: `ehlo: 550 5.7.1 go away` |
| `{}` | `220-mx`, `250 x` | down: `query: malformed reply` |
| `{}` | `hello` | down: `query: malformed reply` |
| `{}` | `220 mx`, then closes | down: `query: connection closed` |
| `{}` | a 3000-byte greeting line | down: `query: reply too long` |
| `{ starttls = true }` | EHLO `250-mx`, `250 starttls` (lower case), STARTTLS `220`, TLS, EHLO `250 mx` | up |
| `{ starttls = true }` | EHLO `250 mx` | down: `tls: STARTTLS not offered` |
| `{ starttls = true }` | EHLO `250-mx`, `250 STARTTLS`, STARTTLS `454 4.7.0 TLS not available` | down: `tls: STARTTLS refused: 454 4.7.0 TLS not available` |
| `{ tls = true }` | plaintext `220 mx` instead of a TLS handshake | down: reason starts with `tls: ` |
