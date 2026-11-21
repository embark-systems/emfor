## MODIFIED Requirements

### Requirement: Peer group configuration

The config SHALL accept an optional `[peers]` table with the keys `listen`, `members` and `secret_file`, all required, and the optional keys `previous_secret_file`, `encrypt`, a boolean that defaults to `true`, and `accept_plaintext`, a boolean; any other key SHALL be an error. When `accept_plaintext` is not set it SHALL default to `false` if `encrypt` is `true`, which includes the case that `encrypt` is not set, and to `true` if `encrypt` is `false`; a value that is set SHALL win over this default. `accept_plaintext = false` together with `encrypt = false` SHALL be an error at `peers.accept_plaintext`, since such a daemon would send the messages it refuses. `listen` SHALL be one `IPv4:port` or `[IPv6]:port` string with port 1–65535 whose address is not the unspecified address (`0.0.0.0` or `::`). `members` SHALL be an array of 2 to 64 such strings with no repeats, all of the address family (IPv4 or IPv6) of `listen`, and SHALL contain `listen`. `secret_file` SHALL be an absolute path to a file that the daemon can read, whose size, a trailing line feed included, is at most 65536 bytes and whose content after removing one trailing line feed is at least 32 bytes, and whose permissions do not allow other users to read it; `previous_secret_file`, when set, SHALL meet the same conditions. When `[peers]` is present the top-level `site` SHALL be set. Every violation SHALL be reported together with the other config errors, prefixed with its TOML path. Without a `[peers]` table the daemon SHALL open no peer socket and send no peer message.

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

#### Scenario: Secret file larger than the limit

- **WHEN** `secret_file` is a file of more than 65536 bytes, a trailing line feed included
- **THEN** the config is rejected with an error at `peers.secret_file` saying that the file is too large; a file of exactly 65536 bytes is accepted

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
