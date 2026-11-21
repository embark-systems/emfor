## MODIFIED Requirements

### Requirement: Peer group configuration

The config SHALL accept an optional `[peers]` table with the keys `listen`, `members` and `secret_file`, all required, and the optional keys `previous_secret_file`, `encrypt`, a boolean that defaults to `true`, and `accept_plaintext`, a boolean; any other key SHALL be an error. When `accept_plaintext` is not set it SHALL default to `false` if `encrypt` is `true`, which includes the case that `encrypt` is not set, and to `true` if `encrypt` is `false`; a value that is set SHALL win over this default. `accept_plaintext = false` together with `encrypt = false` SHALL be an error at `peers.accept_plaintext`, since such a daemon would send the messages it refuses. `listen` SHALL be one `IPv4:port` or `[IPv6]:port` string with port 1–65535 whose address is not the unspecified address (`0.0.0.0` or `::`). `members` SHALL be an array of 2 to 64 such strings with no repeats, all of the address family (IPv4 or IPv6) of `listen`, and SHALL contain `listen`. `secret_file` SHALL be an absolute path to a file that the daemon can read, whose content after removing one trailing line feed is at least 32 bytes, and whose permissions do not allow other users to read it; `previous_secret_file`, when set, SHALL meet the same conditions. When `[peers]` is present the top-level `site` SHALL be set. Every violation SHALL be reported together with the other config errors, prefixed with its TOML path. Without a `[peers]` table the daemon SHALL open no peer socket and send no peer message.

#### Scenario: Valid peer group

- **WHEN** the config has `site = "dc1"` and `[peers]` with `listen = "127.0.0.1:5302"`, `members = ["127.0.0.1:5302", "127.0.0.1:5303"]` and a `secret_file` of 64 bytes with mode 0600
- **THEN** `emfor check-config` exits 0

#### Scenario: Default of accept_plaintext follows encrypt

- **WHEN** the config has a valid `[peers]` table and sets `encrypt` and `accept_plaintext` as in the table below
- **THEN** the daemon accepts or refuses `EMP1` datagrams as the table says, and `emfor check-config` exits 0 unless the table says error

##### Example: accept_plaintext default

| `encrypt` | `accept_plaintext` | `EMP1` datagrams |
| --- | --- | --- |
| not set | not set | refused |
| `true` | not set | refused |
| `false` | not set | accepted |
| not set | `true` | accepted |
| `true` | `false` | refused |
| `false` | `true` | accepted |
| `false` | `false` | error at `peers.accept_plaintext` |

#### Scenario: No peers table

- **WHEN** the config has no `[peers]` table
- **THEN** the daemon binds no peer socket and `/metrics` contains no series whose name starts with `emfor_peer_`

##### Example: peer config validation

| Config | Result |
| --- | --- |
| `[peers]` without top-level `site` | error at `site`: site is required when [peers] is set |
| `listen = "0.0.0.0:5302"` | error at `peers.listen`: listen must be a specific address |
| `listen = "localhost:5302"` | error at `peers.listen`: listen address must be an IP literal with port |
| `listen = "10.0.0.9:5302"`, not in `members` | error at `peers.listen`: listen must be one of members |
| `members = ["10.0.0.5:5302"]` | error at `peers.members`: members must list 2 to 64 addresses |
| `members` with `"10.0.0.6:5302"` twice | error at `peers.members[<i>]` of the second one: duplicate member |
| `listen = "10.0.0.5:5302"`, `members = ["10.0.0.5:5302", "[2001:db8::6]:5302"]` | error at `peers.members[1]`: member must be of the same address family (IPv4 or IPv6) as listen |
| `secret_file = "peers.key"` | error at `peers.secret_file`: must be an absolute path |
| `secret_file` that does not exist | error at `peers.secret_file`: cannot read |
| `secret_file` with 31 bytes plus a line feed | error at `peers.secret_file`: secret must be at least 32 bytes |
| `secret_file` with mode 0644 | error at `peers.secret_file`: must not be readable by other users |
| `previous_secret_file` with mode 0644 | error at `peers.previous_secret_file`: must not be readable by other users |
| `encrypt = "yes"` | error at `peers.encrypt`: encrypt must be true or false |
| `encrypt = false` | valid |
| `accept_plaintext = 0` | error at `peers.accept_plaintext`: accept_plaintext must be true or false |
| `accept_plaintext = false` | valid |
| `encrypt = false` and `accept_plaintext = false` | error at `peers.accept_plaintext`: accept_plaintext = false needs encrypt = true |
| `[peers] port = 5302` | error: unknown key |

### Requirement: Peer message authentication and replay protection

Every peer message SHALL be one UDP datagram of at most 1200 bytes in one of three formats. Format `EMP1` is the four bytes `EMP1`, then a 32-byte HMAC-SHA256 tag computed with the secret over `EMP1` followed by the payload, then the payload. Format `EMP2` is the four bytes `EMP2`, then a 12-byte nonce taken from the operating system's random generator for each message, then the payload encrypted with ChaCha20-Poly1305 followed by its 16-byte tag, with `EMP2` as additional authenticated data and as key the first 32 bytes of HKDF-SHA256 with salt `emfor peers`, the secret as input key material and info `EMP2 chacha20-poly1305`. Format `EMP3` is the four bytes `EMP3`, then a 24-byte nonce taken from the operating system's random generator for each message, then the payload encrypted with XChaCha20-Poly1305 followed by its 16-byte tag, with `EMP3` as additional authenticated data and as key the first 32 bytes of HKDF-SHA256 with salt `emfor peers`, the secret as input key material and info `EMP3 xchacha20-poly1305`. A daemon SHALL send `EMP1` to every member when `[peers] encrypt` is `false`, signing with the secret chosen as in "Peer secret reload". When `encrypt` is `true` it SHALL encrypt with that secret and send each member the encrypted format chosen for it: `EMP3` when the latest `EMP2` or `EMP3` message the daemon accepted from that member is an `EMP3` message, or an `EMP2` message whose payload has the field `emp3` with the value `true`; `EMP2` otherwise, also before any such message was accepted. An accepted `EMP1` message SHALL set the choice to `EMP2`, since its sender may be an earlier version with `encrypt = false` that cannot read `EMP3`; a dropped message of any format SHALL NOT change the choice. Every `EMP2` message a daemon sends SHALL have the field `emp3` with the value `true`, so that two members of this version choose `EMP3` for each other after the first message, while a member that does not send the field (an earlier version) is sent `EMP2`. A receiver SHALL treat an `emp3` field with any value other than `true` as absent, and SHALL ignore the field in `EMP1` and `EMP3` messages. The choice SHALL be kept in memory only, so that after the daemon starts, and for a member added by a reload, it is `EMP2` until a message from that member is accepted. When the choice for a member changes, the daemon SHALL log one line at `info` level naming the member and the new format. A message sent to members with different choices SHALL be encrypted once for each format needed, with the same `seq`. In every format the payload SHALL be a UTF-8 JSON object with the fields `from` (the sender's `listen`), `site`, `boot` (16 lowercase hex digits chosen at random when the sender starts), `seq` (1 for the sender's first message after it starts, increased by 1 for each message), `ts` (Unix milliseconds when sent) and `kind`. A receiver SHALL accept a message only when all of these hold, and SHALL otherwise drop it and count it under the first failed reason in this order: the datagram starts with `EMP1` and is 36 to 1200 bytes, starts with `EMP2` and is 34 to 1200 bytes, or starts with `EMP3` and is 46 to 1200 bytes (`malformed`); the datagram does not start with `EMP1` when `accept_plaintext` is `false` (`plaintext`); the `EMP1` tag matches, or the `EMP2` or `EMP3` payload decrypts and its tag verifies, with the current secret or a secret accepted as in "Peer secret reload" (`auth`); the payload is a JSON object with all common fields and, for a `kind` the receiver knows, all fields of that kind with valid values (`malformed`); `from` is a member other than the receiver and equals the datagram's source address (`sender`); `site` equals the receiver's `site` (`site`); `ts` is within 5 seconds of the receiver's clock (`stale`); and, against the messages accepted from that sender, when `boot` is that of the last accepted one, `seq` is greater than the highest `seq` accepted with it, or is one of the 63 numbers below that highest one and was not accepted yet, so that a message overtaken by later ones on the network is still accepted once; when `boot` differs, `ts` is greater than the highest accepted `ts` (`replay`). The receiver SHALL check the `EMP1` tag before it parses the payload. A receiver keeps no replay state across its own restart, so a message recorded less than 5 seconds before it starts can be accepted once after it starts; such a message is one the sender sent, and acts as if it had been delayed. The known kinds SHALL be `heartbeat`, `down` and `result`. Unknown JSON fields SHALL be ignored, and so SHALL the fields of a `kind` the receiver does not know, also when a known kind uses the same name with another type. An accepted message whose `kind` the receiver does not know SHALL count only for liveness. A dropped message SHALL be logged at `warn` level at most once per 60 seconds for each source address and reason; when many other source addresses have been logged, the daemon may stop logging new ones, but never for a source address that is in `members`. A receiver SHALL accept all three formats whatever its own `encrypt` setting while `accept_plaintext` is `true`; with `accept_plaintext = false` it SHALL drop every `EMP1` datagram without checking its tag and SHALL accept `EMP2` and `EMP3`. When `encrypt` is `true` and an `EMP1` message is accepted, the daemon SHALL log at `warn` level, at most once per 60 seconds for each source address, a line naming the source and saying that it sends plaintext messages.

#### Scenario: Tampered message

- **WHEN** a member receives a `heartbeat` whose payload was changed after the tag was computed
- **THEN** the message is dropped and `emfor_peer_messages_rejected_total{reason="auth"}` increases by 1

#### Scenario: Encrypted heartbeat

- **WHEN** a daemon with `encrypt = true` sends a `heartbeat` to a member
- **THEN** the datagram starts with `EMP2`, with the field `emp3` set to `true`, until the daemon accepts an `EMP3` message, or an `EMP2` message with `emp3` set to `true`, from that member, and with `EMP3` after it has; in both cases its payload does not contain the sender's `site` in plaintext, and the member accepts it

#### Scenario: Format follows the member

- **WHEN** members A and B both run this version with `encrypt = true`, B is restarted as a version that sends only `EMP2` without the `emp3` field, and is later restarted as this version again
- **THEN** A sends B `EMP3` from the first message after it accepts a message from B; after it accepts B's first `EMP2` message without `emp3` it sends `EMP2` again, and `EMP3` once it accepts a message from B of this version again; A logs one `info` line at each change, and B stays live at A except while B restarts

#### Scenario: Member rolled back with encryption off

- **WHEN** A, which has `accept_plaintext = true`, sends B `EMP3`, and B is restarted as a version that does not know `EMP3` with `encrypt = false`
- **THEN** after A accepts B's first `EMP1` message it sends B `EMP2`, logs one `info` line, and A becomes live at B again

#### Scenario: One message for members with different formats

- **WHEN** a daemon with `encrypt = true` sends a `down` message while its choice is `EMP2` for member B and `EMP3` for member C
- **THEN** B receives an `EMP2` datagram and C an `EMP3` datagram with the same `seq`, and each accepts it

#### Scenario: Mixed formats during an upgrade

- **WHEN** member A has `encrypt = true` and `accept_plaintext = true`, and member B has `encrypt = false`
- **THEN** each accepts the other's heartbeats, both stay live, and A logs a `warn` line naming B as sending plaintext messages

#### Scenario: Plaintext refused by default

- **WHEN** member A sets neither `encrypt` nor `accept_plaintext`, and member B has `encrypt = false`
- **THEN** A drops every message from B, `emfor_peer_messages_rejected_total{reason="plaintext"}` increases at A, and B is not live at A, while A's messages are accepted by B; after A reloads with `accept_plaintext = true`, B becomes live at A

#### Scenario: Plaintext refused

- **WHEN** member A has `accept_plaintext = false` and member B has `encrypt = false`
- **THEN** A drops every message from B, `emfor_peer_messages_rejected_total{reason="plaintext"}` increases at A, and B is not live at A; after A reloads with `accept_plaintext = true`, B becomes live at A

#### Scenario: Messages arrive out of order

- **WHEN** a member accepts `seq` 3 and then receives `seq` 1 and 2 of the same `boot`, each for the first time
- **THEN** both are accepted, and `seq` 1 received again is rejected as `replay`

#### Scenario: Sender restarts

- **WHEN** a member has accepted messages up to `seq` 900 with `boot` `aaaaaaaaaaaaaaaa`, and the sender restarts and sends `seq` 1 with `boot` `bbbbbbbbbbbbbbbb` and a later `ts`
- **THEN** the message is accepted

##### Example: acceptance checks

| Situation | Result |
| --- | --- |
| valid heartbeat from member 10.0.0.6:5302 | accepted |
| valid `EMP1` heartbeat from member 10.0.0.6:5302 at a receiver with `encrypt = true` and `accept_plaintext = true` | accepted, `warn` logged for 10.0.0.6:5302 |
| valid `EMP1` heartbeat from member 10.0.0.6:5302 at a receiver that sets neither `encrypt` nor `accept_plaintext` | rejected: plaintext |
| `EMP2` datagram of 33 bytes | rejected: malformed |
| `EMP2` datagram of 1201 bytes | rejected: malformed |
| `EMP2` heartbeat with one byte of the nonce changed | rejected: auth |
| `EMP2` heartbeat encrypted with the previous secret, replaced 90 seconds ago | accepted |
| `EMP2` heartbeat encrypted with a different secret | rejected: auth |
| `EMP2` heartbeat whose decrypted payload is not JSON | rejected: malformed |
| valid `EMP3` heartbeat from member 10.0.0.6:5302 | accepted |
| `EMP3` datagram of 45 bytes | rejected: malformed |
| `EMP3` datagram of 46 bytes that does not decrypt | rejected: auth |
| `EMP3` datagram of 1201 bytes | rejected: malformed |
| `EMP3` heartbeat with one byte of the nonce changed | rejected: auth |
| `EMP3` heartbeat encrypted with the previous secret, replaced 90 seconds ago | accepted |
| `EMP3` heartbeat encrypted with the previous secret, replaced 130 seconds ago | rejected: auth |
| valid `EMP3` heartbeat at a receiver with `accept_plaintext = false` | accepted |
| valid `EMP2` heartbeat with `"emp3":"yes"` | accepted, `emp3` treated as absent |
| datagram starting with `EMP4` | rejected: malformed |
| datagram of 20 bytes | rejected: malformed |
| `EMP1` datagram whose tag does not match and whose payload is not JSON | rejected: auth |
| `EMP1` datagram whose tag matches and whose payload is not JSON | rejected: malformed |
| `kind` = `"result"` with `outcome` = `"pending"` | rejected: malformed |
| tag computed with a different secret | rejected: auth |
| tag computed with the previous secret, replaced 30 seconds ago | accepted |
| tag computed with the previous secret, replaced 90 seconds ago | accepted |
| tag computed with the previous secret, replaced 130 seconds ago | rejected: auth |
| `from` = 10.0.0.6:5302 but sent from 10.0.0.8:5302 | rejected: sender |
| `from` not in `members` | rejected: sender |
| `site` = `dc2` at a receiver in `dc1` | rejected: site |
| `ts` 6 seconds behind the receiver's clock | rejected: stale |
| same `boot`, `seq` equal to the last accepted | rejected: replay |
| same `boot`, `seq` 1 after `seq` 3 was accepted and 1 was not | accepted |
| same `boot`, `seq` 6 after `seq` 70 was accepted and 6 was not | rejected: replay |
| `kind` = `"assign"` (unknown to this version), otherwise valid | accepted, used only for liveness |
| `kind` = `"assign"` with `"service":42`, otherwise valid | accepted, used only for liveness |
