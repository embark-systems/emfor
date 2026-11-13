## ADDED Requirements

### Requirement: Diameter check options

The `diameter` plugin SHALL accept these `check_options` keys and SHALL report every other key as `unknown key`:

- `origin_host` and `origin_realm` (both required): DiameterIdentity strings of 1 to 255 characters made of ASCII letters, digits, `-` and `.`, not starting or ending with `.`. Otherwise the error is `not a valid DiameterIdentity`.
- `auth_application_ids`, `acct_application_ids` and `supported_vendor_ids`: arrays of integers from 0 to 4294967295, default empty. Otherwise the error is `must be an array of integers from 0 to 4294967295`.
- `vendor_specific_application_ids`: an array of tables, default empty, each with exactly the keys `vendor_id` and one of `auth_application_id` or `acct_application_id`, all integers from 0 to 4294967295. Otherwise the error is `each entry needs vendor_id and one of auth_application_id, acct_application_id`.
- `host_ip_address`: an IPv4 or IPv6 address. Otherwise the error is `not an IP address`.
- `tls`, `tls_verify`, `server_name` and `ca_file` with the rules they have for the other `emfor-check` plugins, including `requires tls = true`.
- `tls_cert_file` and `tls_key_file`: absolute paths of a PEM client certificate chain and of its PEM private key, set together and only with `tls = true` (otherwise `requires tls = true`). When only one is set, the error is on that key: `requires tls_key_file` or `requires tls_cert_file`. A path that is not absolute is `not an absolute path`; when options are validated, a file that cannot be read is `file not found` or `cannot read: <kind>`, a certificate file without a PEM certificate is `no certificate found`, and a key file without a PEM private key is `no private key found`.

When `auth_application_ids`, `acct_application_ids` and `vendor_specific_application_ids` are all empty or absent, the plugin SHALL report an error on `auth_application_ids`: `at least one application is required`. A missing `origin_host` or `origin_realm` SHALL be `missing required key`; type errors SHALL be `must be a string` and `must be a boolean`.

#### Scenario: Gx peer options

- **WHEN** a service with `check = "diameter"` sets `check_options = { origin_host = "emfor-app01.example.org", origin_realm = "example.org", vendor_specific_application_ids = [{ vendor_id = 10415, auth_application_id = 16777238 }], supported_vendor_ids = [10415] }`
- **THEN** the plugin reports no errors

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

### Requirement: Diameter check

The `diameter` plugin SHALL open a new TCP connection to the backend on every check (a failure is `down` with a reason starting `connect: `) and, with `tls = true`, perform the TLS handshake at once (a failure is `down` with a reason starting `tls: `). With `tls_cert_file` and `tls_key_file` it SHALL present that certificate chain when the server asks for a client certificate; both files SHALL be read on every check, and a file that cannot be read or holds no certificate or no private key SHALL be `error` with a reason starting `tls_cert_file: ` or `tls_key_file: `, with nothing sent. It SHALL send a Capabilities-Exchange-Request (command code 257, R flag, Application-Id 0, random Hop-by-Hop and End-to-End identifiers) carrying Origin-Host, Origin-Realm, Host-IP-Address (`host_ip_address`, or the local address of the connection), Vendor-Id 0, Product-Name `emfor`, one Supported-Vendor-Id per `supported_vendor_ids` entry, one Auth-Application-Id per `auth_application_ids` entry, one Acct-Application-Id per `acct_application_ids` entry and one grouped Vendor-Specific-Application-Id per `vendor_specific_application_ids` entry, with the M flag set on every AVP except Product-Name.

A message SHALL be the answer to a request when its R flag is clear and its command code and Hop-by-Hop identifier equal the request's; requests from the peer SHALL be ignored and not answered. The plugin SHALL judge in this order:

1. A message whose version is not 1, whose length is below 20 or disagrees with its AVPs, whose AVP length is invalid, or that is longer than 65536 bytes SHALL be `down` with a reason starting `query: `.
2. The connection closed by the peer before the answer SHALL be `down` with reason `cea: connection closed by peer` before the Capabilities-Exchange-Answer and `dwa: connection closed by peer` before the Device-Watchdog-Answer.
3. An answer without Result-Code SHALL be `down` with reason `query: answer without Result-Code`.
4. A Capabilities-Exchange-Answer whose Result-Code is not from 2000 to 2999 SHALL be `down` with reason `cea: <code> <name>`; otherwise the plugin SHALL send a Device-Watchdog-Request (command code 280, R flag, Origin-Host, Origin-Realm), and a Device-Watchdog-Answer whose Result-Code is not from 2000 to 2999 SHALL be `down` with reason `dwa: <code> <name>`. `<name>` SHALL be the result code name defined in RFC 6733 section 7.1, such as `DIAMETER_UNKNOWN_PEER` for 3010, and SHALL be left out (keeping only the code) for codes RFC 6733 does not define.
5. A successful Device-Watchdog-Answer SHALL make the check `up`.

After a successful Capabilities-Exchange-Answer, whatever the result, the plugin SHALL send a Disconnect-Peer-Request (command code 282) with Disconnect-Cause 2 (`DO_NOT_WANT_TO_TALK_TO_YOU`), wait at most 200 ms or the time left before `timeout_ms`, whichever is shorter, for the Disconnect-Peer-Answer, and close the connection; the Disconnect-Peer-Answer SHALL NOT change the result.

#### Scenario: Peer accepts the connection

- **WHEN** the backend answers the CER with Result-Code 2001 and the DWR with Result-Code 2001
- **THEN** the check result is `up` and the plugin sends a DPR with Disconnect-Cause 2 before closing

#### Scenario: Unknown peer

- **WHEN** the backend answers the CER with Result-Code 3010 and the E flag set
- **THEN** the check result is `down` with reason `cea: 3010 DIAMETER_UNKNOWN_PEER`, and no DWR is sent

##### Example: Diameter outcomes

| Backend behavior | Result |
| --- | --- |
| CEA 2001, DWA 2001 | `up` |
| CEA 2002, DWA 2001 | `up` |
| CEA 5010 | `down`, `cea: 5010 DIAMETER_NO_COMMON_APPLICATION` |
| CEA 2001, DWA 3004 | `down`, `dwa: 3004 DIAMETER_TOO_BUSY` |
| CEA 4999 | `down`, `cea: 4999` |
| closes the connection after reading the CER | `down`, `cea: connection closed by peer` |
| CEA 2001, its own DWR, then DWA 2001 | `up` (the peer's DWR is ignored) |
| CEA without Result-Code | `down`, `query: answer without Result-Code` |
| a header with version 2 | `down`, reason starting `query: ` |

#### Scenario: CER encoding

- **WHEN** a CER is built with Hop-by-Hop and End-to-End identifiers 1, `origin_host = "emfor.example.org"`, `origin_realm = "example.org"`, local address `10.0.0.5` and `auth_application_ids = [4]`
- **THEN** it starts with the header bytes `01 <length> 80 00 01 01 00 00 00 00 00 00 00 01 00 00 00 01`, its first AVP is Origin-Host with code 264, flags `40` and the value `emfor.example.org` padded to a multiple of 4 bytes, and its Host-IP-Address value is `00 01 0a 00 00 05`

#### Scenario: Peer requires a client certificate

- **WHEN** the backend accepts TLS only with a client certificate signed by its CA, and the check sets `tls = true`, `tls_cert_file` and `tls_key_file` to such a certificate and its key
- **THEN** the check result is `up`

#### Scenario: Unreadable client key

- **WHEN** the check sets `tls = true`, `tls_cert_file` and `tls_key_file`, and the key file was removed after the options were validated
- **THEN** the check result is `error` with a reason starting `tls_key_file: `, and no connection is opened

#### Scenario: Silent peer

- **WHEN** the backend accepts the TCP connection and never answers the CER, and the check has `timeout_ms` 900
- **THEN** the check result is `down` with reason `timeout`
