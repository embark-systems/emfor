## MODIFIED Requirements

### Requirement: Peer secret reload

The daemon SHALL notice a change of `secret_file` and apply it before sending the next round of heartbeats. Where the platform allows (inotify on Linux, kqueue on FreeBSD and macOS), the daemon SHALL watch the directory that holds `secret_file`, and with kqueue also the file itself, and SHALL read the file after each event, so that both a file written in place and a file replaced by renaming another over it are noticed; it SHALL also read the file every 30 seconds, so that a missed event delays a change by no more than that. When the watch cannot be set up, or the platform has none, or a watch stops working, the daemon SHALL read the file before each round of heartbeats instead, and SHALL log one line at `info` level saying that it does and why. When `secret_file` changes in a reload, the daemon SHALL watch the new path.  When its content changed and is valid, the new content SHALL become the current secret. For 60 seconds after the change the daemon SHALL still sign its messages with the secret it replaced, and SHALL still accept messages signed with the replaced secret for 120 seconds after the change, so that when every member's file changes within 60 seconds of the others no message between any two members is dropped. When the file changes again while the daemon still signs with an earlier secret, the daemon SHALL keep signing with that earlier secret, for 60 seconds after the latest change, and SHALL accept it for 120 seconds after the latest change; every other replaced secret SHALL be accepted for 120 seconds after it was replaced. When the daemon starts with `previous_secret_file` set to a secret other than the current one, and `secret_file` was last modified less than 120 seconds before, the daemon SHALL treat that modification time as the change: it signs with the previous secret until 60 seconds and accepts it until 120 seconds after that time, as if it had been running. Otherwise `previous_secret_file` SHALL have no effect; it is read only when the daemon starts. Without it, a daemon restarted during a change knows only the new secret, and members whose file has not changed yet drop its messages as `auth`, and it theirs, until their files change. When the file cannot be read or its content is not valid, the daemon SHALL keep the last valid secret and log at `warn` level at most once per minute. A read SHALL NOT delay the heartbeats, the liveness checks or the handling of received messages: when a read of `secret_file` has not finished within 200 milliseconds the daemon SHALL carry on with the last valid secret, at most one read of `secret_file` SHALL be in progress at any time, and a new read SHALL start only after the previous one has finished. The result of a read that finishes late SHALL be applied as any other. A read that has been in progress for more than 5 seconds SHALL be reported by a line at `warn` level, at most once per minute.

#### Scenario: Secret rotated on one host first

- **WHEN** host A's `secret_file` is changed to a new valid secret, and host B's file is changed to the same secret 40 seconds later
- **THEN** every message A sends to B and B sends to A is accepted, before, during and after the change, and A signs with the new secret from 60 seconds after its own change

#### Scenario: Secret changed twice

- **WHEN** host A's `secret_file` changes from secret K0 to K1, and 30 seconds later to K2
- **THEN** A signs with K0 until 90 seconds after the first change and with K2 from then on, and accepts K0, K1 and K2 until 150 seconds after the first change

#### Scenario: Restart during a secret change

- **WHEN** host A has `previous_secret_file` holding the old secret, its `secret_file` changes to a new secret, A restarts 30 seconds later, and host B's file has not changed yet
- **THEN** B accepts A's messages and A accepts B's, and A signs with the new secret from 60 seconds after its file changed

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

