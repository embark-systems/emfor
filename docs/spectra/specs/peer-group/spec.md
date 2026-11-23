# peer-group Specification

## Purpose

A peer group is the set of emfor daemons in one `site` that list each other in `[peers]`. Members authenticate every message, track which members are live, and notify each other when a backend fails so that the whole site rechecks it at once; each daemon still decides every backend's state from its own checks.

## Requirements

### Requirement: Peer group configuration

The config SHALL accept an optional `[peers]` table with the keys `listen`, `members` and `secret_file`, all required, and the optional keys `previous_secret_file`, `encrypt`, a boolean that defaults to `true`, `accept_plaintext`, a boolean, and `cipher`, a string that is `"chacha20-poly1305"` (the default) or `"aes-256-gcm"`; any other key SHALL be an error. A `cipher` that is not a string with one of these two values SHALL be an error at `peers.cipher`, and so SHALL `cipher = "aes-256-gcm"` together with `encrypt = false`, since such a daemon sends no encrypted messages. When `accept_plaintext` is not set it SHALL default to `false` if `encrypt` is `true`, which includes the case that `encrypt` is not set, and to `true` if `encrypt` is `false`; a value that is set SHALL win over this default. `accept_plaintext = false` together with `encrypt = false` SHALL be an error at `peers.accept_plaintext`, since such a daemon would send the messages it refuses. `listen` SHALL be one `IPv4:port` or `[IPv6]:port` string with port 1–65535 whose address is not the unspecified address (`0.0.0.0` or `::`). `members` SHALL be an array of 2 to 64 such strings with no repeats, all of the address family (IPv4 or IPv6) of `listen`, and SHALL contain `listen`. `secret_file` SHALL be an absolute path to a file that the daemon can read, whose size, a trailing line feed included, is at most 65536 bytes and whose content after removing one trailing line feed is at least 32 bytes, and whose permissions do not allow other users to read it; `previous_secret_file`, when set, SHALL meet the same conditions. When `[peers]` is present the top-level `site` SHALL be set. Every violation SHALL be reported together with the other config errors, prefixed with its TOML path. Without a `[peers]` table the daemon SHALL open no peer socket and send no peer message.

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
| `cipher = "aes-256-gcm"` | valid |
| `cipher = "chacha20-poly1305"` | valid |
| `cipher = "aes-128-gcm"` | error at `peers.cipher`: cipher must be "chacha20-poly1305" or "aes-256-gcm" |
| `cipher = true` | error at `peers.cipher`: cipher must be "chacha20-poly1305" or "aes-256-gcm" |
| `encrypt = false` and `cipher = "aes-256-gcm"` | error at `peers.cipher`: cipher = "aes-256-gcm" needs encrypt = true |
| `[peers] port = 5302` | error: unknown key |

#### Scenario: Secret file larger than the limit

- **WHEN** `secret_file` is a file of more than 65536 bytes, a trailing line feed included
- **THEN** the config is rejected with an error at `peers.secret_file` saying that the file is too large; a file of exactly 65536 bytes is accepted


<!-- @trace
source: emfor-peer-aes-gcm
updated: 2026-11-23
code:
  - CHANGELOG.md
  - crates/emfor-daemon/Cargo.toml
  - crates/emfor-daemon/src/audit.rs
  - crates/emfor-daemon/src/audit/tests.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/peers/crypto.rs
  - crates/emfor-daemon/src/peers/group.rs
  - crates/emfor-daemon/src/peers/task.rs
  - crates/emfor-daemon/src/peers/wire.rs
  - docs/manual.zh_TW/11-peer-groups.md
  - docs/manual.zh_TW/12-operating.md
  - docs/manual.zh_TW/13-security.md
  - docs/manual.zh_TW/18-config-reference.md
  - docs/manual/11-peer-groups.md
  - docs/manual/12-operating.md
  - docs/manual/13-security.md
  - docs/manual/18-config-reference.md
tests:
  - crates/emfor-daemon/src/peers/test_support.rs
  - crates/emfor-daemon/tests/emp2_kat.py
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Peer message authentication and replay protection

Every peer message SHALL be one UDP datagram of at most 1200 bytes in one of four formats. Format `EMP1` is the four bytes `EMP1`, then a 32-byte HMAC-SHA256 tag computed with the secret over `EMP1` followed by the payload, then the payload. Format `EMP2` is the four bytes `EMP2`, then a 12-byte nonce taken from the operating system's random generator for each message, then the payload encrypted with ChaCha20-Poly1305 followed by its 16-byte tag, with `EMP2` as additional authenticated data and as key the first 32 bytes of HKDF-SHA256 with salt `emfor peers`, the secret as input key material and info `EMP2 chacha20-poly1305`. Format `EMP3` is the four bytes `EMP3`, then a 24-byte nonce taken from the operating system's random generator for each message, then the payload encrypted with XChaCha20-Poly1305 followed by its 16-byte tag, with `EMP3` as additional authenticated data and as key the first 32 bytes of HKDF-SHA256 with salt `emfor peers`, the secret as input key material and info `EMP3 xchacha20-poly1305`. Format `EMP4` is the four bytes `EMP4`, then the sender's `boot` as 8 bytes, then a counter of 8 bytes in big-endian order, then the payload encrypted with AES-256-GCM followed by its 16-byte tag, with the 4 bytes `EMP4`, the 8 bytes of `boot` and the 8 bytes of the counter as additional authenticated data, with the 12-byte nonce of 4 zero bytes followed by the 8 bytes of the counter, and as key the first 32 bytes of HKDF-SHA256 with salt `emfor peers`, the secret as input key material and, as info, the bytes `EMP4 aes-256-gcm` followed by the 8 bytes of `boot`. The counter SHALL be 1 in the first `EMP4` datagram a daemon sends after it starts and SHALL increase by 1 for each `EMP4` datagram it sends, so that no key is used with the same nonce twice; the `boot` in the datagram SHALL be the `boot` of the payload. A daemon SHALL send `EMP1` to every member when `[peers] encrypt` is `false`, signing with the secret chosen as in "Peer secret reload". When `encrypt` is `true` and `cipher` is `"aes-256-gcm"` it SHALL encrypt with that secret and send `EMP4` to every member, whatever it accepted from them. When `encrypt` is `true` and `cipher` is `"chacha20-poly1305"`, which includes the case that `cipher` is not set, it SHALL encrypt with that secret and send each member the encrypted format chosen for it: `EMP3` when the latest `EMP2` or `EMP3` message the daemon accepted from that member is an `EMP3` message, or an `EMP2` message whose payload has the field `emp3` with the value `true`; `EMP2` otherwise, also before any such message was accepted. An accepted `EMP1` message SHALL set the choice to `EMP2`, since its sender may be an earlier version with `encrypt = false` that cannot read `EMP3`; a dropped message of any format SHALL NOT change the choice. Every `EMP2` message a daemon sends SHALL have the field `emp3` with the value `true`, so that two members of this version choose `EMP3` for each other after the first message, while a member that does not send the field (an earlier version) is sent `EMP2`. A receiver SHALL treat an `emp3` field with any value other than `true` as absent, and SHALL ignore the field in `EMP1` and `EMP3` messages. The choice SHALL be kept in memory only, so that after the daemon starts, and for a member added by a reload, it is `EMP2` until a message from that member is accepted. When the choice for a member changes, the daemon SHALL log one line at `info` level naming the member and the new format. A message sent to members with different choices SHALL be encrypted once for each format needed, with the same `seq`. In every format the payload SHALL be a UTF-8 JSON object with the fields `from` (the sender's `listen`), `site`, `boot` (16 lowercase hex digits chosen at random when the sender starts), `seq` (1 for the sender's first message after it starts, increased by 1 for each message), `ts` (Unix milliseconds when sent) and `kind`. A receiver SHALL accept a message only when all of these hold, and SHALL otherwise drop it and count it under the first failed reason in this order: the datagram starts with `EMP1` and is 36 to 1200 bytes, starts with `EMP2` and is 34 to 1200 bytes, starts with `EMP3` and is 46 to 1200 bytes, or starts with `EMP4` and is 38 to 1200 bytes (`malformed`); the datagram does not start with `EMP1` when `accept_plaintext` is `false` (`plaintext`); the `EMP1` tag matches, or the `EMP2`, `EMP3` or `EMP4` payload decrypts and its tag verifies, with the current secret or a secret accepted as in "Peer secret reload" (`auth`); the payload is a JSON object with all common fields and, for a `kind` the receiver knows, all fields of that kind with valid values (`malformed`); `from` is a member other than the receiver and equals the datagram's source address (`sender`); `site` equals the receiver's `site` (`site`); `ts` is within 5 seconds of the receiver's clock (`stale`); and, against the messages accepted from that sender, when `boot` is that of the last accepted one, `seq` is greater than the highest `seq` accepted with it, or is one of the 63 numbers below that highest one and was not accepted yet, so that a message overtaken by later ones on the network is still accepted once; when `boot` differs, `ts` is greater than the highest accepted `ts` (`replay`). The receiver SHALL check the `EMP1` tag before it parses the payload. A receiver keeps no replay state across its own restart, so a message recorded less than 5 seconds before it starts can be accepted once after it starts; such a message is one the sender sent, and acts as if it had been delayed. The known kinds SHALL be `heartbeat`, `down` and `result`. Unknown JSON fields SHALL be ignored, and so SHALL the fields of a `kind` the receiver does not know, also when a known kind uses the same name with another type. An accepted message whose `kind` the receiver does not know SHALL count only for liveness. A dropped message SHALL be logged at `warn` level at most once per 60 seconds for each source address and reason; when many other source addresses have been logged, the daemon may stop logging new ones, but never for a source address that is in `members`. A receiver SHALL accept all four formats whatever its own `encrypt` and `cipher` settings while `accept_plaintext` is `true`; with `accept_plaintext = false` it SHALL drop every `EMP1` datagram without checking its tag and SHALL accept `EMP2`, `EMP3` and `EMP4`. An accepted `EMP4` message SHALL NOT change the format chosen for its sender for `EMP2` and `EMP3`, and SHALL NOT be treated as a plaintext message. When `encrypt` is `true` and an `EMP1` message is accepted, the daemon SHALL log at `warn` level, at most once per 60 seconds for each source address, a line naming the source and saying that it sends plaintext messages.

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

#### Scenario: Members send AES-GCM

- **WHEN** members A and B both run this version with `encrypt = true` and `cipher = "aes-256-gcm"`
- **THEN** every datagram A sends B starts with `EMP4`, its payload does not contain the sender's `site` in plaintext, B accepts it, and the counters of A's `EMP4` datagrams are 1, 2, 3 and so on

#### Scenario: AES-GCM key and counter after a restart

- **WHEN** A, with `cipher = "aes-256-gcm"`, is restarted
- **THEN** its first `EMP4` datagram after the start has counter 1 and a new `boot`, it is encrypted under a key that differs from the one it used before, and B accepts it

#### Scenario: Cipher changed by a reload

- **WHEN** A sends `EMP3` to B, and a reload sets `cipher = "aes-256-gcm"` at A
- **THEN** A's next datagram to B starts with `EMP4`, and after a reload that sets `cipher = "chacha20-poly1305"` it starts with `EMP3` again, since A still accepts `EMP3` and `EMP2` from B as before

#### Scenario: Earlier version receives AES-GCM

- **WHEN** A, with `cipher = "aes-256-gcm"`, sends to B, a version that does not know `EMP4`
- **THEN** B counts every datagram as `malformed` and A is not live at B, while B's messages are accepted by A; after B is upgraded, A is live at B

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
| valid `EMP4` heartbeat from member 10.0.0.6:5302 | accepted |
| `EMP4` datagram of 37 bytes | rejected: malformed |
| `EMP4` datagram of 38 bytes that does not decrypt | rejected: auth |
| `EMP4` datagram of 1201 bytes | rejected: malformed |
| `EMP4` heartbeat with one byte of the `boot` changed | rejected: auth |
| `EMP4` heartbeat with one byte of the counter changed | rejected: auth |
| `EMP4` heartbeat encrypted with the previous secret, replaced 90 seconds ago | accepted |
| `EMP4` heartbeat encrypted with the previous secret, replaced 130 seconds ago | rejected: auth |
| valid `EMP4` heartbeat at a receiver with `accept_plaintext = false` | accepted |
| `EMP4` heartbeat whose `boot` in the header differs from the `boot` in its payload | rejected: malformed |
| datagram starting with `EMP5` | rejected: malformed |
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


<!-- @trace
source: emfor-peer-aes-gcm
updated: 2026-11-23
code:
  - CHANGELOG.md
  - crates/emfor-daemon/Cargo.toml
  - crates/emfor-daemon/src/audit.rs
  - crates/emfor-daemon/src/audit/tests.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/peers/crypto.rs
  - crates/emfor-daemon/src/peers/group.rs
  - crates/emfor-daemon/src/peers/task.rs
  - crates/emfor-daemon/src/peers/wire.rs
  - docs/manual.zh_TW/11-peer-groups.md
  - docs/manual.zh_TW/12-operating.md
  - docs/manual.zh_TW/13-security.md
  - docs/manual.zh_TW/18-config-reference.md
  - docs/manual/11-peer-groups.md
  - docs/manual/12-operating.md
  - docs/manual/13-security.md
  - docs/manual/18-config-reference.md
tests:
  - crates/emfor-daemon/src/peers/test_support.rs
  - crates/emfor-daemon/tests/emp2_kat.py
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Peer secret reload

The daemon SHALL notice a change of `secret_file` and apply it before sending the next round of heartbeats. Where the platform allows (inotify on Linux, kqueue on FreeBSD and macOS), the daemon SHALL watch the directory that holds `secret_file`, and with kqueue also the file itself, and SHALL read the file after each event, so that both a file written in place and a file replaced by renaming another over it are noticed; it SHALL also read the file every 30 seconds, so that a missed event delays a change by no more than that. When the watch cannot be set up, or the platform has none, or a watch stops working, the daemon SHALL read the file before each round of heartbeats instead, and SHALL log one line at `info` level saying that it does and why. When `secret_file` changes in a reload, the daemon SHALL watch the new path.  When its content changed and is valid, the new content SHALL become the current secret. For 60 seconds after the change the daemon SHALL still sign its messages with the secret it replaced, and SHALL still accept messages signed with the replaced secret for 120 seconds after the change, so that when every member's file changes within 60 seconds of the others no message between any two members is dropped. When the file changes again while the daemon still signs with an earlier secret, the daemon SHALL keep signing with that earlier secret, for 60 seconds after the latest change, and SHALL accept it for 120 seconds after the latest change; every other replaced secret SHALL be accepted for 120 seconds after it was replaced. The daemon SHALL keep a record of the secret it uses, in the file named after the store path with `-peers` appended (mode 0600), holding a SHA-256 fingerprint of the current secret, and once the daemon has replaced a secret, the fingerprint of the secret it replaced and the time of that change. The daemon SHALL write the record at start and each time the current secret changes, and SHALL NOT store a secret itself. When the daemon starts and the fingerprint of the current secret differs from the one in the record, the secret in the record SHALL count as the one that was replaced, at the modification time of `secret_file`. When the daemon starts with `previous_secret_file` set to a secret other than the current one, and that secret is the one the record says was replaced, and that change was less than 120 seconds before, the daemon SHALL treat it as the change: it signs with the previous secret until 60 seconds and accepts it until 120 seconds after that time, as if it had been running. Otherwise `previous_secret_file` SHALL have no effect, so that a secret that was retired earlier, or that the daemon never used, is not used again because a file holds it or `secret_file` was touched; when a `previous_secret_file` is set and has no effect because the record is missing, cannot be read or does not name that secret, the daemon SHALL log one line at `info` level saying so. `previous_secret_file` is read only when the daemon starts. A daemon that finds no record, such as at its first start after an upgrade, SHALL ignore `previous_secret_file`. When the record cannot be written, the daemon SHALL log at `warn` level at most once per minute and SHALL keep running. Without it, a daemon restarted during a change knows only the new secret, and members whose file has not changed yet drop its messages as `auth`, and it theirs, until their files change. When the file cannot be read or its content is not valid, the daemon SHALL keep the last valid secret and log at `warn` level at most once per minute. A read SHALL NOT delay the heartbeats, the liveness checks or the handling of received messages: when a read of `secret_file` has not finished within 200 milliseconds the daemon SHALL carry on with the last valid secret, at most one read of `secret_file` SHALL be in progress at any time, and a new read SHALL start only after the previous one has finished. The result of a read that finishes late SHALL be applied as any other, except that when `secret_file` was changed by a reload while the read was in progress, the result SHALL be dropped, since it is the content of the old path; the new path SHALL be read as soon as that read is over. A read that has been in progress for more than 5 seconds SHALL be reported by a line at `warn` level, at most once per minute.

#### Scenario: Secret rotated on one host first

- **WHEN** host A's `secret_file` is changed to a new valid secret, and host B's file is changed to the same secret 40 seconds later
- **THEN** every message A sends to B and B sends to A is accepted, before, during and after the change, and A signs with the new secret from 60 seconds after its own change

#### Scenario: Secret changed twice

- **WHEN** host A's `secret_file` changes from secret K0 to K1, and 30 seconds later to K2
- **THEN** A signs with K0 until 90 seconds after the first change and with K2 from then on, and accepts K0, K1 and K2 until 150 seconds after the first change

#### Scenario: Restart during a secret change

- **WHEN** host A has `previous_secret_file` holding the old secret, its `secret_file` changes to a new secret while A runs, A restarts 30 seconds later, and host B's file has not changed yet
- **THEN** B accepts A's messages and A accepts B's, and A signs with the new secret from 60 seconds after its file changed

#### Scenario: Stop, change the secret, start

- **WHEN** host A, whose record holds secret K0, is stopped, its `secret_file` is changed to K1 with `previous_secret_file` holding K0, and A is started 30 seconds later, and host B's file has not changed yet
- **THEN** A treats K0 as replaced at the modification time of `secret_file`, B accepts A's messages and A accepts B's, and A signs with K1 from 60 seconds after that time

#### Scenario: Previous secret retired by an earlier rotation

- **WHEN** `previous_secret_file` still holds K0, the daemon's record says K1 was replaced by K2 and K2 is the current secret, and the daemon restarts 30 seconds after `secret_file` changed to K2
- **THEN** `previous_secret_file` has no effect, the daemon signs and accepts with K2 only, and it logs one line at `info` level saying the previous secret is not the one that was replaced

#### Scenario: Secret file touched

- **WHEN** `previous_secret_file` holds K0, the daemon's record says K0 was replaced by the current secret K1 more than 120 seconds ago, `secret_file` is rewritten with the same content K1 so that its modification time is now, and the daemon restarts
- **THEN** the daemon uses K1 only

#### Scenario: First start after an upgrade

- **WHEN** the daemon starts with `previous_secret_file` set and there is no record, and `secret_file` was modified 30 seconds before
- **THEN** `previous_secret_file` has no effect, the daemon logs one line at `info` level saying there is no record, and it writes a record holding the fingerprint of the current secret

#### Scenario: Read of the secret file hangs

- **WHEN** a read of host A's `secret_file` does not return, for example because the file is on a network file system that does not answer
- **THEN** A keeps sending a heartbeat every second, keeps checking liveness and handling messages from members, and no second read of the file starts; after 5 seconds A logs one `warn` line saying that the read has not finished and that the last valid secret is kept, and no second such line within the next minute; when the read returns, its result is applied

#### Scenario: Slow read of the secret file

- **WHEN** a read of A's `secret_file` takes 3 seconds and returns a new valid secret
- **THEN** A sends its heartbeats during the 3 seconds with the secret it had, applies the new secret when the read returns, and logs no warning

#### Scenario: Normal read applied at once

- **WHEN** A's `secret_file` is changed to a new valid secret on a file system that answers at once
- **THEN** A applies the new secret before the next round of heartbeats, as it did before reads were limited to one

#### Scenario: Secret file replaced by a rename

- **WHEN** a watch is active and a new file holding a new valid secret is renamed over host A's `secret_file`
- **THEN** A applies the new secret before its next round of heartbeats, when the read finishes within 200 milliseconds, without waiting for a periodic read

#### Scenario: Secret file written in place

- **WHEN** a watch is active and host A's `secret_file` is overwritten in place with a new valid secret
- **THEN** A applies the new secret before its next round of heartbeats, when the read finishes within 200 milliseconds

#### Scenario: Watch cannot be set up

- **WHEN** the watch of A's `secret_file` cannot be set up
- **THEN** A reads the file before each round of heartbeats, logs one `info` line saying why, and applies a changed secret by the next round of heartbeats as it did before watching was added

#### Scenario: Late result of the old path

- **WHEN** a read of the old `secret_file` is still in progress when a reload changes `secret_file`, and it then returns a valid secret that differs from the current one
- **THEN** that secret is not applied, and the content of the new `secret_file` is read once the old read is over


<!-- @trace
source: emfor-secret-read-hardening
updated: 2026-11-21
code:
  - docs/release-signing.md
  - docs/casa-assessment.md
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual/05-configuring.md
  - CHANGELOG.md
  - crates/emfor-daemon/src/peers/task.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/peers/watch.rs
-->

---
### Requirement: Peer heartbeats and liveness

A daemon with `[peers]` SHALL send a `heartbeat` message every second to every member other than itself. A member SHALL be live while the daemon has accepted at least one message from it within the last 3 seconds. When a member becomes live or stops being live the daemon SHALL log one line at `info` level naming the member.

The service tag of a service SHALL be the first 4 bytes of SHA-256 over the UTF-8 bytes of the service name, written as 8 lowercase hex digits. When this daemon has services whose effective `peers` setting is `false` (see "Services left out of the peer group"), each heartbeat SHALL carry the field `off`: an array of the distinct service tags of those services in byte order, at most 80 of them; with more than 80 distinct tags only the first 80 in that order SHALL be sent, and the daemon SHALL log one line at `warn` level, at start and after each reload that changes those services, giving the number of tags not sent. Without such services the heartbeat SHALL NOT carry `off`. A `heartbeat` whose `off` is present and is not an array of strings of 8 lowercase hex digits each SHALL be dropped as `malformed`.

For each member the daemon SHALL keep the service tags of the latest heartbeat it accepted from that member, none when that heartbeat has no `off`, and SHALL forget them when the member stops being live. A heartbeat whose `boot` is that of the heartbeat the kept tags came from and whose `seq` is lower SHALL NOT change the kept tags (it was overtaken by a later one on the network; it still counts for liveness and as an accepted message); a heartbeat with another `boot` SHALL replace them, since the replay check accepts one only when its `ts` is later. The `boot` and `seq` of the heartbeat the kept tags came from SHALL be remembered for as long as the member stays in `members`, also while the member is not live and has no kept tags. Messages of other kinds SHALL NOT change the kept tags.

#### Scenario: Member stops

- **WHEN** two daemons form a peer group and one of them stops
- **THEN** within 4 seconds the other logs that the stopped member is not live and its `emfor_peer_up` series for that member is 0

#### Scenario: Heartbeat announces services left out

- **WHEN** this daemon has `[peers]` and the services `api.svc.emfor` and `db.svc.emfor` have `peers = false` while `web.svc.emfor` has `peers = true`
- **THEN** every heartbeat it sends carries `"off":["a513a047","fa6f63b4"]`, and after a reload that sets `peers = true` for both, heartbeats carry no `off`

##### Example: service tags

| Service | Service tag |
| --- | --- |
| `api.svc.emfor` | `a513a047` |
| `db.svc.emfor` | `fa6f63b4` |
| `web.svc.emfor` | `99e6433f` |

#### Scenario: More services left out than fit

- **WHEN** this daemon starts with 81 services with `peers = false` whose tags are distinct
- **THEN** every heartbeat carries the 80 smallest of those tags in byte order, the datagram is at most 1200 bytes, and the daemon logs one `warn` line saying 1 tag is not sent

#### Scenario: Kept tags follow the latest heartbeat

- **WHEN** member `10.0.0.6:5302` sends a heartbeat with `"off":["a513a047"]`, then a `result` message, then a heartbeat without `off`
- **THEN** after the first heartbeat and after the `result` message this daemon keeps the tag `a513a047` for that member, and after the second heartbeat it keeps none

##### Example: heartbeat `off` field

| `off` in an otherwise valid heartbeat | Result |
| --- | --- |
| absent | accepted, no tags kept |
| `[]` | accepted, no tags kept |
| `["a513a047","fa6f63b4"]` | accepted, both tags kept |
| `["A513A047"]` | rejected: malformed |
| `["a513a04"]` | rejected: malformed |
| `"a513a047"` | rejected: malformed |
| `[42]` | rejected: malformed |

#### Scenario: Older heartbeat overtaken on the network

- **WHEN** member B's heartbeat with `seq` 11 announces service tag T and is accepted, and B's heartbeat with `seq` 10 and the same `boot`, announcing no tags, is accepted afterwards
- **THEN** the kept tags of B are still T, and B stays live

#### Scenario: Heartbeat of a restarted sender

- **WHEN** the kept tags of member B came from `seq` 900 with `boot` `aaaaaaaaaaaaaaaa`, and B restarts and sends `seq` 1 with `boot` `bbbbbbbbbbbbbbbb`, a later `ts` and no tags
- **THEN** the kept tags of B are replaced by none


<!-- @trace
source: emfor-peer-state-order
updated: 2026-11-21
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/peers/task.rs
  - crates/emfor-daemon/src/quorum.rs
-->

---
### Requirement: Check definition digest

Every service SHALL have a check definition digest: the first 16 bytes, as 32 lowercase hex digits, of the SHA-256 of the JSON object `{"checker":<checker identity>,"options":<check_options>,"timeout_ms":<timeout in milliseconds>}` written with every object's keys in lexicographic order and no whitespace. The checker identity SHALL be the check name for a built-in checker, `plugin:<name>:<command as JSON>` for a plugin without `env_pass`, and `plugin:<name>:<command as JSON>:<env_pass as JSON>` for a plugin whose `env_pass` is not empty. For an `exec` check, `env_pass` is a key of `check_options` and so is part of the digest like every other option. Two configs whose service differs only in the order of keys, in `interval`, `rise`, `fall`, `jitter`, `balance`, `on_all_down`, `dns_ttl` or `backends` SHALL give that service the same digest.

#### Scenario: Key order does not matter

- **WHEN** host A writes a service's `check_options` as `{ path = "/health", expect_status = ["200"] }` and host B writes `{ expect_status = ["200"], path = "/health" }`, with the same `check` and `timeout`
- **THEN** both hosts compute the same digest for that service

#### Scenario: Plugin env_pass changes the digest

- **WHEN** host A declares `[plugins."redis"]` with `env_pass = ["http_proxy"]` and host B declares it with the same `command` and no `env_pass`, and a service uses that plugin on both
- **THEN** the two hosts compute different digests for that service, and a plugin without `env_pass` has the identity it had before `env_pass` existed

##### Example: what changes the digest

| Difference between two configs of one service | Same digest |
| --- | --- |
| `interval = "2s"` vs `"5s"` | yes |
| `rise = 2` vs `3` | yes |
| `backends` with one more entry | yes |
| `timeout = "1s"` vs `"500ms"` | no |
| `check = "tcp"` vs `"http"` | no |
| `check_options.path = "/health"` vs `"/ready"` | no |
| `exec` `check_options.env_pass = ["HOME"]` vs no `env_pass` | no |
| plugin `env_pass = ["http_proxy"]` vs none | no |


<!-- @trace
source: emfor-child-environment
updated: 2026-11-21
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/checker/exec.rs
  - crates/emfor-daemon/src/child.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/src/plugin/host.rs
  - crates/emfor-daemon/src/plugin/mod.rs
  - crates/emfor-daemon/src/plugin/set.rs
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual.zh_TW/04-health-checks.md
  - docs/manual.zh_TW/08-operations.md
  - docs/manual/05-configuring.md
  - docs/manual/06-health-checks.md
  - docs/manual/12-operating.md
tests:
  - crates/emfor-daemon/tests/peers.rs
  - crates/emfor-daemon/tests/plugin_host.rs
  - crates/emfor-daemon/tests/plugins.rs
-->

---
### Requirement: Backend failure notification

When one of this daemon's own checks moves a backend from `healthy` to `unhealthy`, the daemon SHALL send one `down` message to every member other than itself, with the fields `service`, `digest` (the check definition digest of the definition the check ran with, also when a reload changed it during the check), `backend` (the backend id) and `reason` (the transition's reason, cut at a character boundary to at most 256 bytes of UTF-8). A move from `unknown` to `unhealthy`, or a move made at a check time decided by votes alone (see "Split checks"), SHALL NOT send a message. A message SHALL NOT be sent again.

When a daemon accepts a `down` message for a service it has with the same digest and a backend of that service, it SHALL start a fast recheck run for that backend when all of these hold: the backend is `healthy` in this daemon, no fast recheck run is in progress for it, and its last fast recheck run ended more than `fall × interval` ago. Otherwise it SHALL ignore the message. A `down` message SHALL NOT change any backend's state by itself; states change only through this daemon's checks and their combined results (see "Region verdict and combined result"). Starting a fast recheck run SHALL be logged at `info` level as `peer <member> reports backend <backend> of <service> down (<reason>); checking now`.

The `down` and `result` messages of backend tasks SHALL go to the peer task through a queue of at most 4096 messages; a check SHALL NOT wait for room in it. A message that finds the queue full SHALL be dropped and counted in `emfor_peer_queue_dropped_total` (spec metrics-endpoint), and the daemon SHALL log at `warn` level, at most once per 60 seconds, that the peer task is behind.

#### Scenario: Peer queue full

- **WHEN** backend tasks queue messages while 4096 are already waiting for the peer task
- **THEN** each new message is dropped, `emfor_peer_queue_dropped_total` grows by one for each, at most one `warn` line is logged per 60 seconds, and the messages already queued are still sent

#### Scenario: Peer sees the failure first

- **WHEN** daemons A and B in one peer group both check a TCP backend of `api.svc.emfor` with `fall = 3`, A with `interval = "1s"` and B with `interval = "30s"` (the digest leaves `interval` out), both see it `healthy`, and the backend stops listening
- **THEN** B, which on its own would need at least a minute, marks it `unhealthy` within 2 seconds after A did

#### Scenario: Digest differs

- **WHEN** A and B check the same backend but B's service has `timeout = "500ms"` and A's has `timeout = "1s"`, and A sends a `down` message for it
- **THEN** B ignores the message and checks the backend on its normal schedule

#### Scenario: Moves decided by votes are not announced

- **WHEN** a daemon that is not an assigned checker of a `healthy` backend marks it `unhealthy` at a check time decided by votes alone
- **THEN** it sends no `down` message for it

#### Scenario: Startup failures are not announced

- **WHEN** a daemon starts and a backend goes from `unknown` to `unhealthy`
- **THEN** the daemon sends no `down` message for it

##### Example: whether a received down message starts a fast recheck run

| Local state of the backend | Fast run in progress | Last fast run ended | Result |
| --- | --- | --- | --- |
| healthy | no | never | start a fast run |
| healthy | no | `fall × interval` + 1s ago | start a fast run |
| healthy | no | 1s ago (`fall × interval` = 6s) | ignore |
| healthy | yes | — | ignore |
| unhealthy | no | never | ignore |
| unknown | no | never | ignore |


<!-- @trace
source: emfor-peers-split-checks
updated: 2026-11-13
code:
  - crates/emfor-daemon/src/schedule.rs
  - docs/manual/12-operating.md
  - crates/emfor-daemon/src/peers/
  - docs/manual/03-concepts.md
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/quorum.rs
  - crates/emfor-daemon/src/metrics.rs
  - CHANGELOG.md
  - docs/manual/05-configuring.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Peer listen requires restart

A change to `[peers] listen`, including adding or removing the `[peers]` table, SHALL be a validation error on reload with the message "peers listen settings require restart". Changes to `[peers] members`, `[peers] secret_file`, `[peers] encrypt` and `[peers] accept_plaintext` SHALL take effect on reload (`previous_secret_file` is validated, but read only at start), a changed `encrypt` from the next message sent and a changed `accept_plaintext` from the next message received; a removed member's replay state and metrics series SHALL be dropped, and an added member SHALL start as not live. Binding `listen` at start SHALL be a startup error when it fails.

#### Scenario: Member added on reload

- **WHEN** a daemon runs with `members = ["127.0.0.1:5302", "127.0.0.1:5303"]`, `127.0.0.1:5304` is added to `members` and the daemon receives `SIGHUP`
- **THEN** the reload succeeds, `emfor_peer_up{peer="127.0.0.1:5304"}` appears with value 0, and the daemon sends heartbeats to 127.0.0.1:5304

#### Scenario: Listen change rejected

- **WHEN** `[peers] listen` is changed from `"127.0.0.1:5302"` to `"127.0.0.1:5305"` and the daemon receives `SIGHUP`
- **THEN** the reload is rejected with the error "peers listen settings require restart" and the daemon keeps receiving on 127.0.0.1:5302


<!-- @trace
source: emfor-peer-encryption
updated: 2026-11-14
code:
  - crates/emfor-daemon/Cargo.toml
  - fuzz/seeds/peers/heartbeat-emp1
  - fuzz/seeds/peers/result-payload
  - RELEASING.md
  - fuzz/seeds/peers/down-emp1
  - fuzz/seeds/peers/heartbeat-payload
  - crates/emfor-daemon/src/peers/
  - docs/manual/05-configuring.md
  - examples/emfor.toml
  - docs/manual.zh_TW/03-configuration.md
  - fuzz/seeds/peers/result-emp1
  - docs/deploy/daemon.md
  - fuzz/seeds/peers/heartbeat-emp2
  - fuzz/README.md
  - crates/emfor-daemon/src/config.rs
  - docs/manual.zh_TW/08-operations.md
  - fuzz/seeds/peers/result-emp2
  - CHANGELOG.md
  - examples/freebsd/Makefile.crates
  - fuzz/seeds/peers/down-payload
  - fuzz/seeds/peers.py
  - docs/manual/12-operating.md
  - fuzz/seeds/peers/down-emp2
tests:
  - crates/emfor-daemon/tests/peers.rs
  - crates/emfor-daemon/tests/emp2_kat.py
-->

---
### Requirement: Assigned checkers and check results

For every backend of every service, a daemon with `[peers]` SHALL choose the backend's assigned checkers among the candidates: itself and every member that is currently live and whose kept service tags (see "Peer heartbeats and liveness") do not include the service tag of the backend's service. For each candidate it SHALL compute MD5 over the UTF-8 bytes of `<service name>/<backend id>/<candidate listen>`, where the candidate listen is written as in the config (IPv6 as `[addr]:port`), read the first 4 bytes of the digest as a big-endian unsigned 32-bit integer, and take the 3 candidates with the largest values as the assigned checkers, ranked from the largest; on equal values the candidate whose listen string is smaller in byte order SHALL rank first. With fewer than 3 candidates the backend SHALL have no assigned checkers. The choice SHALL be made again for every check, from the members live and the tags kept at that moment. When two services of this daemon have the same service tag, a member that announces the tag SHALL be left out of the candidates of both.

When the daemon is an assigned checker of a backend and its own check of that backend gives `up` or `down`, it SHALL send one `result` message to every member other than itself, with the fields `service`, `digest` (the check definition digest of the definition the check ran with, also when a reload changed it during the check), `backend` (the backend id), `outcome` (`"up"` or `"down"`, the result of its own check, never the combined result), `reason` (only when `outcome` is `"down"`: the check's reason cut at a character boundary to at most 256 bytes of UTF-8) and `interval_ms` (the service's `interval` in milliseconds). An `error` result SHALL NOT send a message. A `result` message that lacks one of these fields, has another `outcome`, or has an `interval_ms` that is not a positive integer SHALL be dropped as `malformed`. An accepted `result` message for a service and backend the receiver has SHALL be kept as that sender's latest result for the backend, together with its digest, its `interval_ms` and the time the receiver accepted it; a result whose `boot` is that of the kept one and whose `seq` is lower than the kept one's SHALL NOT replace it (it was overtaken by a later result on the network; it still counts as an accepted message), and a result with another `boot` SHALL replace it; one for a service or backend the receiver does not have SHALL be ignored.

#### Scenario: Assigned checkers among five live members

- **WHEN** members `10.0.0.5:5302` (this daemon) through `10.0.0.9:5302` are all live and the daemon checks backend `10.0.0.11:8080` of `api.svc.emfor`
- **THEN** the assigned checkers are, in rank order, `10.0.0.5:5302`, `10.0.0.8:5302` and `10.0.0.6:5302`, and this daemon sends a `result` message after each `up` or `down` check of that backend

##### Example: rendezvous values for api.svc.emfor

| Backend | Member | Value |
| --- | --- | --- |
| `10.0.0.11:8080` | `10.0.0.5:5302` | 3974634306 |
| `10.0.0.11:8080` | `10.0.0.8:5302` | 3444584285 |
| `10.0.0.11:8080` | `10.0.0.6:5302` | 1483479382 |
| `10.0.0.11:8080` | `10.0.0.7:5302` | 1161570282 |
| `10.0.0.11:8080` | `10.0.0.9:5302` | 367848677 |
| `10.0.0.12:8080` | `10.0.0.9:5302` | 4163305238 |
| `10.0.0.12:8080` | `10.0.0.6:5302` | 3934586029 |
| `10.0.0.12:8080` | `10.0.0.7:5302` | 2645478105 |
| `10.0.0.12:8080` | `10.0.0.8:5302` | 2352845107 |
| `10.0.0.12:8080` | `10.0.0.5:5302` | 729780139 |

#### Scenario: A member stops being live

- **WHEN** in the group above `10.0.0.8:5302` stops being live
- **THEN** the assigned checkers of `10.0.0.11:8080` become `10.0.0.5:5302`, `10.0.0.6:5302` and `10.0.0.7:5302`

#### Scenario: A member announces a service left out

- **WHEN** in the group above `10.0.0.8:5302` stays live and its heartbeats carry `"off":["a513a047"]`, the service tag of `api.svc.emfor`
- **THEN** the assigned checkers of `10.0.0.11:8080` become `10.0.0.5:5302`, `10.0.0.6:5302` and `10.0.0.7:5302`, and those of `10.0.0.12:8080` stay `10.0.0.9:5302`, `10.0.0.6:5302` and `10.0.0.7:5302`

#### Scenario: Not an assigned checker

- **WHEN** this daemon is `10.0.0.5:5302` in the group above and checks backend `10.0.0.12:8080`
- **THEN** it sends no `result` message for that backend

#### Scenario: Error results are not sent

- **WHEN** an assigned checker's own check of a backend gives `error`
- **THEN** it sends no `result` message for that check

#### Scenario: Two-member group

- **WHEN** a group has 2 members and both are live
- **THEN** no backend has assigned checkers and no `result` message is sent

#### Scenario: Older result overtaken on the network

- **WHEN** member B sent `up` for backend X in `seq` 10 and `down` in `seq` 11 with the same `boot`, and this daemon accepts `seq` 11 first and `seq` 10 after it
- **THEN** the result kept for B and X is `down`, and `seq` 10 is counted as received but does not count as a vote

#### Scenario: Result of a restarted sender

- **WHEN** the kept result of member B for backend X came from `seq` 900 with `boot` `aaaaaaaaaaaaaaaa`, and B restarts and sends `up` in `seq` 1 with `boot` `bbbbbbbbbbbbbbbb` and a later `ts`
- **THEN** the result kept for B and X is `up`


<!-- @trace
source: emfor-peer-state-order
updated: 2026-11-21
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/peers/task.rs
  - crates/emfor-daemon/src/quorum.rs
-->

---
### Requirement: Region verdict and combined result

After each of its own checks of a backend, a daemon with `[peers]` SHALL count the votes of the backend's assigned checkers. The vote of this daemon, when it is an assigned checker, SHALL be the result of this check when it is `up` or `down`. The vote of another assigned checker SHALL be its latest kept `result` when that result's digest equals this daemon's current digest for the service, it was accepted less than `3 × interval_ms` of that result ago, and the sender is live; otherwise that checker SHALL have no vote. With `f` votes of which `d` are `down`:

- with no assigned checkers or `f` = 0, the combined result SHALL be this daemon's own result, with its reason unchanged;
- with `d` ≥ 2, the combined result SHALL be `down` with the reason `peers <d>/<f>: <reason>`, where `<reason>` is the reason of the highest-ranked `down` vote, whatever this daemon's own result is;
- otherwise the combined result SHALL be this daemon's own result, and when that result is `down` its reason SHALL be `local: <own reason>`.

A kept result SHALL count toward the health state at one check time of this daemon only, so that one result of another member never moves `rise` or `fall` more than once, whatever the senders' and this daemon's intervals. Every check time that counts the votes of a backend (an own check, or a check time decided by votes alone, see "Split checks") SHALL mark the kept results it counted as counted; a newly accepted `result` message replaces the kept result and is not counted. When `d` ≥ 2 but none of the `down` votes of other assigned checkers is new (not yet counted) and this daemon's own result is not `down`, the check SHALL NOT be applied: it SHALL NOT change the health state machine, the stored state and reason, the check latency, the backend state history or whether the backend is a disagreement, and it SHALL NOT be stored, logged as a state change or announced. Since the check ran (and an assigned checker sent its result), it SHALL still be counted in `emfor_backend_checks_total` with this daemon's own result. An own `down` result is always new.

The combined result SHALL take the place of the check result everywhere a check result is used: the health state machine, the stored state and reason, the backend state history, state change logs, check counters and fast recheck runs. Reasons with a prefix SHALL be normalized and bounded like any other reason. The check latency SHALL still be measured from this daemon's own check and SHALL count only when the combined result is `up`.

A backend whose combined result is `down` with `d` ≥ 2 while this daemon's own result was `up` SHALL be a disagreement until a later combined result of that backend is not, or until the backend or its service is removed. When a backend becomes a disagreement the daemon SHALL log one line at `info` level: `backend <backend> of <service> is down at <d>/<f> peers but up here`. When the daemon accepts a `down` or `result` message for a service it has whose digest differs from its own, it SHALL log at `debug` level and record a digest mismatch for that service and member, which SHALL last until no such message from that member for that service has been accepted for 60 seconds, or the member is removed.

#### Scenario: Two of three assigned checkers see the backend down

- **WHEN** in a group of three daemons with the same check definition, two daemons' own checks of a backend give `down` with reason `connect: Connection refused` and the third daemon's own checks give `up`
- **THEN** the third daemon marks the backend `unhealthy` after `fall` checks, its stored reason starts with `peers 2/`, and the backend is a disagreement there

#### Scenario: Only one daemon sees the backend down

- **WHEN** in a group of three daemons only one daemon's own checks of a backend give `down` with reason `timeout`
- **THEN** that daemon marks the backend `unhealthy` with reason `local: timeout`, and the other two keep it `healthy`

#### Scenario: Too few live members

- **WHEN** a group has 3 members and one of them stops being live
- **THEN** each remaining daemon decides every backend from its own checks only, with reasons as its checks give them

#### Scenario: Votes from members with a longer interval count once

- **WHEN** this daemon has `interval = "1s"` and `fall = 3`, its own checks of a `healthy` backend give `up`, and two of the three assigned checkers, with `interval = "30s"`, each send one `down` result
- **THEN** the region verdict counts at one check time only, the backend stays `healthy` here until those checkers send `down` results at two more of their check times, and the check times in between store nothing

##### Example: combined result

"This daemon assigned" tells whether this daemon is one of the three assigned checkers; when it is, its own `up` or `down` result is a vote too. "Kept votes" are the fresh kept results of the other assigned checkers, in rank order.

| This daemon assigned | Kept votes | Own result | Combined result | Reason |
| --- | --- | --- | --- | --- |
| — (2 candidates, no assigned checkers) | — | down `timeout` | down | `timeout` |
| no | none | down `timeout` | down | `timeout` |
| yes | down `refused`, down `refused` | up | down | `peers 2/3: refused` |
| no | down `refused`, down `reset` | up | down | `peers 2/2: refused` |
| yes | down `refused`, down `reset` | error `plugin exited` | down | `peers 2/2: refused` |
| yes | up, up | down `timeout` | down | `local: timeout` |
| yes | down `refused` | up | up | — |
| no | down `refused` (only fresh vote) | up | up | — |
| no | down `refused`, down `refused`, up | error `plugin exited` | down | `peers 2/3: refused` |
| yes | up, up | error `plugin exited` | error | `plugin exited` |

##### Example: which kept results are votes

| Kept result of another assigned checker | Vote |
| --- | --- |
| same digest, accepted 1s ago, `interval_ms` 2000, sender live | yes |
| same digest, accepted 7s ago, `interval_ms` 2000, sender live | no (older than 6s) |
| same digest, accepted 7s ago, `interval_ms` 5000, sender live | yes |
| different digest, accepted 1s ago | no, and a digest mismatch is recorded |
| same digest, accepted 1s ago, sender not live | no |

<!-- @trace
source: emfor-peers-quorum
updated: 2026-11-13
code:
  - crates/emfor-daemon/Cargo.toml
  - docs/manual/05-configuring.md
  - CHANGELOG.md
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/metrics.rs
  - docs/manual/12-operating.md
  - crates/emfor-daemon/src/peers/
  - crates/emfor-daemon/src/lib.rs
  - crates/emfor-daemon/src/schedule.rs
  - crates/emfor-daemon/src/quorum.rs
  - crates/emfor-daemon/src/writer.rs
  - crates/emfor-daemon/src/cli.rs
  - crates/emfor-format/src/lib.rs
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/supervisor.rs
  - docs/manual/03-concepts.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Split checks

For a backend that has assigned checkers of which this daemon is not one, the daemon SHALL decide each scheduled check time as follows, counting the fresh votes of the other assigned checkers exactly as in "Region verdict and combined result" (`f` votes, `d` of them `down`, `u` of them `up`). It SHALL make its own check when any of these holds: it has not made an own check of the backend since the backend's task started; its last own result of the backend was not `up`; it decided the previous 2 check times of the backend by votes alone; a fast recheck run is in progress; neither `d` ≥ 2 nor `u` ≥ 2; `d` ≥ 2 and none of the `down` votes is new, or `d` < 2, `u` ≥ 2 and none of the `up` votes is new (see "Region verdict and combined result": a kept result counts at one check time only); or `d` < 2 and `u` ≥ 2 while the backend has no latency (spec state-store, "Check latency"), so that a backend never turns healthy here without a measured latency. The result of such an own check SHALL be combined as in "Region verdict and combined result". Otherwise it SHALL NOT check, and the applied result SHALL be `down` with the reason `peers <d>/<f>: <reason>`, `<reason>` being the reason of the highest-ranked `down` vote, when `d` ≥ 2, and `up` otherwise. The assigned checkers SHALL be chosen again at every check time, from the members live at that moment; a daemon that is an assigned checker, or a backend without assigned checkers, SHALL be checked at every check time as in "Region verdict and combined result".

When such an own check of a daemon that is not an assigned checker gives `down` while `d` < 2 and `u` ≥ 2, so that the combined result is `local: <reason>`, the daemon SHALL start a fast recheck run for the backend (see "Backend failure notification") when all of these hold after the result is applied: the backend is `healthy` in this daemon, no fast recheck run is in progress for it, and its last fast recheck run ended more than `fall × interval` ago. An own check that gives `error`, an own check of an assigned checker, and an own check made while `u` < 2 or `d` ≥ 2 SHALL NOT start a run. Starting such a run SHALL be logged at `info` level as `backend <backend> of <service> fails here while its assigned checkers see it up (<reason>); checking now`.

A result decided by votes alone SHALL be applied like a check result to the health state machine, the stored state and reason, the backend state history, state change logs and check counters. It SHALL NOT add a latency sample (the latency still becomes empty when the backend turns `unhealthy`, spec state-store "Check latency"), send a `result` or `down` message, or change whether the backend is a disagreement. The daemon SHALL count each check time decided by votes alone in `emfor_peer_checks_skipped_total`.

#### Scenario: Sparse own checks while the assigned checkers see the backend up

- **WHEN** this daemon is not an assigned checker of a backend with `interval = "1s"` and `jitter = "0%"`, every assigned checker's fresh vote is `up`, its own checks give `up`, and 10 check times pass starting with an own check
- **THEN** the daemon makes own checks at the 1st, 4th, 7th and 10th check times only, and the backend stays `healthy`

#### Scenario: Local failure while the region is up

- **WHEN** in the case above with `fall = 3` the own check at the 4th check time gives `down` with reason `connection refused`, and so do the following own checks
- **THEN** the daemon makes two more own checks 200 ms apart, the first 200 ms after the 4th check time, and after the second of them, about 400 ms after the 4th check time, the backend is `unhealthy` with reason `local: connection refused`

#### Scenario: Assigned checker does not start a run on its own failure

- **WHEN** this daemon is an assigned checker of a `healthy` backend with `interval = "1s"`, `jitter = "0%"` and `fall = 3`, the other two assigned checkers' fresh votes are `up`, and its own check gives `down` with reason `connection refused`
- **THEN** its next own check is 1 second later, and the backend is `unhealthy` after the third failed check, 2 seconds after the first

#### Scenario: Region down without an own check

- **WHEN** this daemon is not an assigned checker of a `healthy` backend with `fall = 3`, its own check at the previous check time gave `up`, and from now on two of the three assigned checkers' fresh votes are `down` with reason `refused` and the third is `up`
- **THEN** the next 2 check times are decided by votes alone, the 3rd is an own check whose result is `peers 2/3: refused` whatever the own check gives, and after the 3rd the backend is `unhealthy` with reason `peers 2/3: refused`; because that move is made by an own check, the daemon sends one `down` message (see "Backend failure notification")

#### Scenario: Recovery decided by votes waits for a latency

- **WHEN** a backend turned `unhealthy` at check times decided by votes alone, so its latency is empty, and the assigned checkers' fresh votes become `up` again
- **THEN** the daemon makes its own check at the next check time, and the backend is `healthy` again only with a latency from that check

#### Scenario: Assigned checkers stop being live

- **WHEN** in a group of four members two members stop being live while this daemon is not an assigned checker of a backend
- **THEN** the backend has no assigned checkers and the daemon checks it at every check time

##### Example: what this daemon does at one check time

Backend `10.0.0.12:8080` of `api.svc.emfor`; this daemon is `10.0.0.5:5302`; the assigned checkers are `10.0.0.9:5302`, `10.0.0.6:5302` and `10.0.0.7:5302`, and "Votes" lists their fresh votes in that order.

| Own check made since start | Last own result | Latency | Check times decided by votes in a row | Votes | Action | Applied result |
| --- | --- | --- | --- | --- | --- | --- |
| no | — | none | 0 | up, up, up | own check, `up` | up |
| yes | up | 820µs | 0 | up, up, up | votes alone | up |
| yes | up | 820µs | 1 | up, up, up | votes alone | up |
| yes | up | 820µs | 1 | down `refused`, down `refused`, up | votes alone | down `peers 2/3: refused` |
| yes | up | none | 1 | down `refused`, down `refused`, up | votes alone | down `peers 2/3: refused` |
| yes | up | none | 0 | up, up, up | own check, `up` | up |
| yes | up | 820µs | 2 | up, up, up | own check, `up` | up |
| yes | up | 820µs | 1 | up, down `refused` | own check, `up` | up |
| yes | up | 820µs | 1 | none | own check, `down` `timeout` | down `timeout` |
| yes | down `timeout` | 820µs | 0 | up, up, up | own check, `down` `timeout` | down `local: timeout` |
| yes | error `plugin exited` | 820µs | 0 | up, up | own check, `error` `plugin exited` | error `plugin exited` |

##### Example: whether a failed own check starts a fast recheck run

Same backend and daemon as above, `interval = "2s"`, `fall = 3`, so `fall × interval` is 6 s. "State after the result" is the backend's state once the own check's combined result is applied.

| Assigned checker | Votes | Own result | State after the result | Fast run in progress | Last fast run ended | Result |
| --- | --- | --- | --- | --- | --- | --- |
| no | up, up, up | down `refused` | healthy | no | never | start a fast run |
| no | up, up, down `refused` | down `refused` | healthy | no | 7s ago | start a fast run |
| no | up, up, up | down `refused` | healthy | no | 1s ago | no run |
| no | up, up, up | down `refused` | healthy | yes | — | no new run; the run in progress goes on |
| no | up, up, up | down `refused` | unhealthy | no | never | no run |
| no | up, up, up | error `plugin exited` | healthy | no | never | no run |
| no | up, down `refused` | down `refused` | healthy | no | never | no run |
| no | down `refused`, down `refused`, up | down `refused` | healthy | no | never | no run |
| yes | up, up | down `refused` | healthy | no | never | no run |


<!-- @trace
source: emfor-peer-faster-local-failure
updated: 2026-11-14
code:
  - docs/deploy/daemon.md
  - crates/emfor-daemon/src/schedule.rs
  - docs/manual/12-operating.md
  - CHANGELOG.md
  - docs/manual.zh_TW/08-operations.md
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual/03-concepts.md
  - crates/emfor-daemon/src/quorum.rs
  - docs/manual.zh_TW/01-concepts.md
  - docs/manual/05-configuring.md
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Services left out of the peer group

A service whose effective `peers` setting (spec service-config) is `false` SHALL take no part in the peer group on this daemon, while the daemon and its other services stay members. For such a service the daemon SHALL:

- not send `down` or `result` messages for any of its backends;
- check every backend at every check time and apply its own result with its reason unchanged, without counting votes, deciding by votes alone, or marking a disagreement;
- ignore every accepted `down` or `result` message for the service as one for a service it does not have: start no fast recheck run, keep no result, and record no digest mismatch;
- not count check times of the service in `emfor_peer_checks_skipped_total`.

When a reload turns `peers` of a service from `true` to `false`, the daemon SHALL drop the kept results, disagreements and digest mismatches of that service, and a fast recheck run already in progress SHALL run to its end. When a reload turns it back to `true`, the service SHALL take part again from its next check time, starting with no kept results. This daemon SHALL announce the services it leaves out in the `off` field of its heartbeats (see "Peer heartbeats and liveness"), so that other members leave it out of the candidates for those services' backends; a member that does not read `off` keeps counting this daemon as a candidate. Tags not sent because of the limit of 80 SHALL leave this daemon a candidate for those services on every member. Without a `[peers]` table the setting SHALL have no effect.

#### Scenario: A service left out on one member

- **WHEN** a group has three live members with the same check definition for `api.svc.emfor`, `api.svc.emfor` has `peers = false` on member A only, and the other two members' own checks of a backend give `down` with reason `refused` while A's give `up`
- **THEN** the backend stays `healthy` on A with no disagreement, A sends no `result` or `down` message for `api.svc.emfor`, the backend has no assigned checkers on the other two members because A is not a candidate there, and the other two members apply their own results and mark it `unhealthy`

#### Scenario: Down message for a service left out

- **WHEN** a member sends a `down` message for a `healthy` backend of a service that has `peers = false` on this daemon
- **THEN** this daemon starts no fast recheck run and checks the backend at its next `interval`

#### Scenario: Leaving the group on reload drops kept state

- **WHEN** a backend of `api.svc.emfor` is a disagreement on this daemon and a reload sets `peers = false` for `api.svc.emfor`
- **THEN** within 2 seconds the disagreement is no longer in `meta/peers`, and the next applied result of the backend is this daemon's own result

##### Example: what this daemon does for one backend

This daemon is not an assigned checker of the backend; the assigned checkers' fresh votes are `down` `refused`, `down` `refused`, `up`; this daemon makes its own check at this check time and it gives `up`.

| `peers` of the service | Votes counted | Applied result | `result` message sent | Disagreement |
| --- | --- | --- | --- | --- |
| `true` | yes | down `peers 2/3: refused` | no | yes |
| `false` | no | up | no | no |

<!-- @trace
source: emfor-peers-off-announce
updated: 2026-11-13
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/peers/
  - docs/manual/05-configuring.md
  - crates/emfor-daemon/src/status.rs
  - crates/emfor-daemon/src/quorum.rs
  - CHANGELOG.md
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual/12-operating.md
  - docs/manual.zh_TW/01-concepts.md
  - crates/emfor-daemon/src/supervisor.rs
  - docs/manual/03-concepts.md
  - crates/emfor-daemon/src/metrics.rs
  - docs/manual.zh_TW/08-operations.md
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/peers.rs
-->

---
### Requirement: Peer secret file must not be writable

A `secret_file` or `previous_secret_file` that is writable by group or others (mode `& 0o022` not zero) SHALL be a config error at that key, whose message says the file must not be writable by group or other users. A file with mode 0600 or 0640 SHALL be accepted.

#### Scenario: Writable secret

- **WHEN** the `secret_file` has mode 0620, 0602 or 0660
- **THEN** the config is rejected with an error at `peers.secret_file`

---
### Requirement: Peer failure reason in the log

A backend failure reason received from a peer SHALL be cut to 256 characters and escaped before it is written to the log, so that it cannot add log lines or fill the log.

#### Scenario: Reason with a line break

- **WHEN** an authenticated member reports a backend down with a reason that contains a line break
- **THEN** the log line shows the line break escaped, on one line

---
### Requirement: Peer secret file owner

A `secret_file` or `previous_secret_file` SHALL be owned by root (uid 0) or by the effective user of the process that checks it; otherwise it SHALL be a config error at that key, whose message says the file is owned by uid N, not root or the user running emfor. The check SHALL apply at start, in `emfor check-config` and at reload, like the mode checks, and the process that checks is the daemon at start and at reload and the process of whoever runs `emfor check-config`, so `emfor check-config` compares the owner with the user that runs it, not with the user the daemon runs as. The owner SHALL be that of the file that was opened, so for a link it is the owner of the file the link points to, and the check SHALL come after the file has been opened: a user that cannot read the file gets a cannot-read error, not an owner error. The group of the file SHALL NOT matter for the owner check (the group can still decide whether the file can be read).

#### Scenario: Secret owned by another user

- **WHEN** the daemon runs as uid 998 and the `secret_file` has mode 0640 and is owned by uid 1000, and the daemon user can read it (for example through the group)
- **THEN** the config is rejected with an error at `peers.secret_file` that names uid 1000

#### Scenario: Secret owned by root or by the checking user

- **WHEN** the `secret_file` has mode 0640 and is owned by root, or by the user that does the check (the daemon user at start and at reload), whatever its group, provided the checking user can read it
- **THEN** the config is accepted

#### Scenario: Previous secret owned by another user

- **WHEN** the `previous_secret_file` has mode 0640, the checking user can read it, and it is owned by a user other than root and the user that does the check
- **THEN** the config is rejected with an error at `peers.previous_secret_file` that names the owner's uid

#### Scenario: Link to a file of another owner

- **WHEN** the `secret_file` is a link, owned by root, to a file owned by uid 1000 that is not the user that does the check, and the file has mode 0640 and the checking user can read it
- **THEN** the config is rejected with an error at `peers.secret_file` that names uid 1000

#### Scenario: Check-config run by an account that is not the daemon user

- **WHEN** a user with uid 1001, who is not root, runs `emfor check-config` and the `secret_file` has mode 0640 and is owned by uid 998, the user the daemon runs as, and uid 1001 can read the file (for example it is in the group of the file)
- **THEN** the config is rejected with an error at `peers.secret_file` that names uid 998

#### Scenario: Check-config accepts a secret owned by root for every account

- **WHEN** a user with uid 1001, who is not root, runs `emfor check-config` and the `secret_file` has mode 0640 and is owned by root, and uid 1001 can read the file (for example it is in the group of the file)
- **THEN** the config is accepted

##### Example: Owner and checking user

Every row assumes that the file is a regular file that the checking user can read, has no mode bit for others to read and none for group or others to write (for example 0640), and has at least 32 bytes after one trailing line feed is removed.

| File owner | Checking user | Result |
| ---------- | ------------- | ------ |
| 0 | 998 | accepted |
| 998 | 998 | accepted |
| 1000 | 998 | error naming uid 1000 |
| 0 | 0 | accepted |
| 1000 | 0 | error naming uid 1000 |

<!-- @trace
source: emfor-secret-owner-wording
updated: 2026-11-20
code:
  - CHANGELOG.md
  - docs/manual.zh_TW/03-configuration.md
  - docs/release-signing.md
  - docs/manual/05-configuring.md
  - docs/casa-assessment.md
  - crates/emfor-daemon/src/config.rs
-->

---
### Requirement: Silent assigned checkers

A daemon with `[peers]` SHALL keep, for every backend it checks, the assigned checkers other than itself that it chose at its latest own check of that backend, with the check definition digest and the service's `interval` of that check and the time of the check. A record whose check is older than the window below SHALL be dropped.

Once per second the daemon SHALL treat a member M as silent for a backend when all of the following hold: M is among the kept assigned checkers of the backend; M is live now and its kept service tags do not include the service tag of the backend's service; the service has no digest mismatch with M (see "Check definition digest"); the daemon keeps no fresh result of M for the backend that has the digest of the kept record, fresh as defined in "Assigned checkers and check results"; and these conditions have held at every scan from the first scan at which M was assigned and had no fresh result for the backend, for at least the window. The window SHALL be the larger of 10 seconds and 6 times the service's `interval`. A fresh result of M for the backend, M not being live, M announcing the service tag, M no longer being among the kept assigned checkers, a digest mismatch with M, or the record being dropped SHALL end the silence and restart the count.

A silent checker SHALL NOT change the assigned checkers, the votes or the combined result. A member whose check gives `error` sends no result and SHALL be reported like any other silent member.

The daemon SHALL log at `warn` once when the first backend of a service becomes silent for a member, with the member, the service and that backend, and at `info` once when no backend of that service is silent for that member any more.

#### Scenario: Member without the backend becomes silent

- **WHEN** members A, B and C are live, `interval` is 1 second, A and B check backend `10.0.0.21:8080` of `app.svc.emfor`, C does not have that backend, C is among the assigned checkers, and 10 seconds pass
- **THEN** A reports C as silent for `app.svc.emfor` backend `10.0.0.21:8080` and logs one `warn` line naming C, `app.svc.emfor` and the backend

#### Scenario: Not reported before the window

- **WHEN** the same group has run for 9 seconds with C sending no result for the backend
- **THEN** no silent checker is reported

#### Scenario: A result ends the silence

- **WHEN** C is reported silent and then sends a fresh result for the backend
- **THEN** within 2 seconds C is no longer reported and A logs one `info` line naming C and `app.svc.emfor`

#### Scenario: A slower service waits longer

- **WHEN** the service has `interval = "30s"` and C sends no result
- **THEN** C is not reported before 180 seconds without a fresh result

#### Scenario: Member with another digest is not silent

- **WHEN** C sends results whose digest differs from A's for the service
- **THEN** C is reported in `digest_mismatch` and not as silent

#### Scenario: Member that leaves the service out is not silent

- **WHEN** C announces the service tag of `app.svc.emfor` in its heartbeats
- **THEN** C is not among the candidates and is not reported as silent for that service

#### Scenario: All members send results

- **WHEN** A, B and C all list the backend and check it
- **THEN** no silent checker is reported and nothing is logged for it

<!-- @trace
source: emfor-peer-silent-checker
updated: 2026-11-23
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/metrics.rs
  - crates/emfor-daemon/src/peers/stats.rs
  - crates/emfor-daemon/src/peers/task.rs
  - crates/emfor-daemon/src/quorum.rs
  - crates/emfor-daemon/src/schedule.rs
  - crates/emfor-daemon/src/status.rs
  - docs/manual.zh_TW/11-peer-groups.md
  - docs/manual.zh_TW/12-operating.md
  - docs/manual.zh_TW/16-troubleshooting.md
  - docs/manual/11-peer-groups.md
  - docs/manual/12-operating.md
  - docs/manual/16-troubleshooting.md
  - tools/peer-mismatch-test.sh
-->

---
### Requirement: Clock offset of members

A daemon with `[peers]` SHALL measure, from every message of a member that passes the checks of format, authentication, sender and `site` (including a message that is then dropped because its timestamp is more than 5 seconds from the receiver's clock, but not a replay of a message of that member that was accepted before), the clock offset of that member: the timestamp of the message minus the receiver's clock in milliseconds at its arrival, positive when the member's clock is ahead. For each member the daemon SHALL keep the latest measurement and the time it was made, and SHALL drop the measurement when no such message has arrived for 10 seconds. Measuring SHALL NOT change which messages are accepted, the liveness of members, the assigned checkers or the votes.

Once per second the daemon SHALL put a member into the state "clock offset" when the absolute value of the offsets of its latest three messages are all at least 2000 milliseconds and have the same sign, and SHALL take it out of that state when the absolute value of the offset of its latest message is below 1500 milliseconds or its measurement is dropped. Between the two limits the state SHALL stay as it is.

The daemon SHALL log at `warn` once when a member enters the state, with the member, the direction (`behind` or `ahead` of this host) and the offset in seconds, and at `info` once when it leaves it. These lines are in addition to the log line for dropped messages.

#### Scenario: Member behind by 3 seconds

- **WHEN** three messages of member C arrive whose timestamps are 3000 milliseconds before the receiver's clock
- **THEN** within 1 second C is in the state "clock offset" with offset -3000, and one `warn` line names C, `behind` and 3.0 seconds

#### Scenario: Offset beyond the limit is still measured

- **WHEN** messages of member C arrive whose timestamps are 6000 milliseconds before the receiver's clock and are dropped as `stale`
- **THEN** the dropped messages are counted as `stale` as before, and C is in the state "clock offset" with offset -6000

#### Scenario: One delayed message does not enter the state

- **WHEN** one message of member C arrives with an offset of -2500 milliseconds, between messages with an offset of -20 milliseconds
- **THEN** C is not in the state "clock offset" and nothing is logged

#### Scenario: Hysteresis

- **WHEN** C is in the state and its next message has an offset of -1800 milliseconds
- **THEN** C stays in the state; after a message with an offset of -400 milliseconds C is out of the state and one `info` line names C

#### Scenario: Ahead of this host

- **WHEN** three messages of member C arrive whose timestamps are 2500 milliseconds after the receiver's clock
- **THEN** C is in the state with offset 2500 and the `warn` line says `ahead`

#### Scenario: The measurement expires

- **WHEN** C is in the state and no message of C passes the checks for 10 seconds
- **THEN** the measurement of C is dropped, C is out of the state and one `info` line names C

#### Scenario: A replay is not measured

- **WHEN** member C's messages with sequence numbers 1 to 4 were accepted with offsets of 0, and the datagram with sequence number 1 is sent to the receiver again 20 seconds later
- **THEN** it is dropped as `stale` as before, and the latest offset of C stays 0 and C is not in the state "clock offset"

#### Scenario: Offsets do not change the votes

- **WHEN** C is in the state "clock offset" and sends results that are fresh
- **THEN** the liveness of C, the assigned checkers and the combined results are the same as without the measurement

#### Scenario: Clocks agree

- **WHEN** all members have offsets below 500 milliseconds
- **THEN** no member is in the state and nothing is logged for offsets

<!-- @trace
source: emfor-peer-clock-offset
updated: 2026-11-23
code:
  - CHANGELOG.md
  - crates/emfor-daemon/src/metrics.rs
  - crates/emfor-daemon/src/peers/group.rs
  - crates/emfor-daemon/src/peers/stats.rs
  - crates/emfor-daemon/src/peers/task.rs
  - crates/emfor-daemon/src/quorum.rs
  - crates/emfor-daemon/src/status.rs
  - docs/manual.zh_TW/11-peer-groups.md
  - docs/manual.zh_TW/12-operating.md
  - docs/manual.zh_TW/16-troubleshooting.md
  - docs/manual/11-peer-groups.md
  - docs/manual/12-operating.md
  - docs/manual/16-troubleshooting.md
  - tools/peer-clock-test.sh
-->