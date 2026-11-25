# Threat model

This page says what emfor protects, from whom, and where it stops. It
complements [SECURITY.md](../SECURITY.md) ("What emfor trusts"), which says
what counts as a vulnerability. It was written from the code and the manual
of 0.7.x plus the unreleased changes of 2026-11; it is not the result of a
penetration test or of a review by a third party.

## What emfor is

One daemon per host. It checks backends (TCP, HTTP, exec, plugins), keeps the
result in a store file, answers DNS from memory, and shares results with
other emfor daemons over UDP. Applications on the same host read the store
directly through `libemfor` or a client wrapper, without a network call.

## What is worth protecting

| Asset | Why it matters |
| --- | --- |
| The health answers (which backends are up) | A wrong answer sends traffic to a dead backend, or away from live ones. |
| Availability of the daemon, of DNS and of lookups | A health system that stops is worse than none. |
| Credentials in the config (passwords, TLS keys, the peer secret) | They open the backends, or let someone speak as a group member. |
| The host the daemon runs on | The daemon runs checks that may run commands. |

## Components and trust

```
 backends ──(checks)──▶ emfor daemon ──(store file, lock file)──▶ applications
 plugins  ──(stdio)──▶      │  ▲                                  (libemfor)
                            │  └── other emfor daemons (UDP, shared secret)
                            └──▶ DNS clients (UDP/TCP, loopback by default)
                            └──▶ /metrics readers (HTTP, loopback by default)
```

| Boundary | Trusted? | What crosses it |
| --- | --- | --- |
| Config file, `include_dir`, plugin programs | Trusted (operator-controlled) | Settings, commands to run. Checked at start for who may write and own them. |
| Backends and plugin output | **Not trusted** | Check results, reason text. Bounded and escaped before use. |
| Other emfor daemons | Trusted only as far as the shared secret goes | Signed or encrypted UDP messages with results. |
| Local users | Partly trusted | Read the store; write access to the lock file is needed to look up. |
| DNS and `/metrics` clients | **Not trusted**, no authentication | Queries, HTTP requests. |
| Store file contents | Not trusted by clients | A malformed store must end in an error code, not a crash. |

## Who may attack, and what they can try

### A backend, or a plugin that talks to a backend

Can send huge, malformed or hostile answers, or hold the connection open.

- Answers are bounded (reasons cut at 512 characters, plugin lines at 65536
  bytes) and control and bidirectional-override characters are escaped before
  they reach the store, the log, `emfor status` and `/metrics`. The escaping
  covers control characters and the format characters that can reverse text
  (bidirectional overrides, zero-width and tag characters), not every
  Unicode format character.
- Every check has a timeout, and each backend has its own task, so a slow
  backend does not block the others. An `exec` check that runs too long is
  killed together with its process group.
- Not covered: a backend that lies. emfor takes a TCP or HTTP success at its
  word, as far as the check can tell.

### A network attacker who can reach the DNS port

Can send many queries or spoofed sources.

- UDP replies are trimmed to the client's EDNS size (512 to 1232 bytes; 512
  without EDNS) and flagged as truncated, TCP has a cap of 128 connections
  and a 5-second idle timeout, and a rate limit applies per client network
  (default 100 a second; loopback is exempt).
- DNS has no authentication and the answers are not signed. An attacker on
  the path can read them or change them in transit. The default is loopback;
  exposing it to a network is the operator's choice.
- Not covered: DNSSEC, and amplification control beyond the rate limit and
  the size cut.

### A network attacker who can reach `/metrics`

- Limited request size, a 5-second I/O timeout, 16 connections, and an
  optional `[metrics] allow` list of networks.
- No authentication and no TLS. Put a reverse proxy in front of it if it must
  cross a network.

### A network attacker on the peer UDP port

Can capture, replay, modify, delay or forge messages.

- Messages are authenticated (`EMP1`, HMAC-SHA256) or encrypted and
  authenticated (`EMP2` ChaCha20-Poly1305, `EMP3` XChaCha20-Poly1305, `EMP4`
  AES-256-GCM). The sender's name, a time and a sequence number are inside
  the authenticated part. A message more than 5 seconds off the receiver's
  clock is dropped; inside that window a replay is dropped by a sliding
  window of sequence numbers kept per sender and per start of the sender.
  A message from a sender that has restarted is accepted only if it is newer
  than the newest message accepted from that sender before.
- `accept_plaintext` defaults to off when encryption is on, and to on when it
  is off; whoever turns it on accepts unencrypted `EMP1` messages.
- Not covered: the secret is shared by the whole group. Anyone who holds it
  can send results; the receiver checks that the sender's name is a member
  and equals the UDP source address, which is not a cryptographic check and
  can be forged by someone who can spoof source addresses. Forward secrecy
  is not provided.
- What a member that holds the secret can do to the verdict: a result is
  counted only if it comes from one of the three assigned checkers of that
  backend (as the receiver ranks them), and a region verdict of down needs 2
  down votes, one of which can be the receiver's own result. One lying member
  cannot turn a backend down against two honest members. Two members that
  agree, one liar plus a member with a bad network path, or one secret holder
  who can also spoof the source addresses of two other members, can. A group
  of two members has no votes at all; each decides alone.
- The other direction: a member that is not assigned to a backend may skip
  its own check when 2 other assigned checkers send fresh `up` results.
  Two lying assigned checkers can make it do so for a backend that is down,
  but at most for 2 check times in a row: the third is always its own check,
  and once an own check fails every later one is an own check. The backend
  turns unhealthy after about 3 intervals plus a fraction of a second. An
  assigned checker always runs its own check.
- A captured message can make a member's clock look wrong in monitoring: the
  daemon measures the clock offset of a member before it drops a message as
  `stale`, and it leaves out only a replay of a message that it accepted from
  that member before. For a member from which nothing was accepted since the
  daemon started, or whose messages are all dropped as `stale`, someone who
  holds an old datagram and can spoof the member's source address can have
  the member shown as "clock of B is N.N s behind this host", with a `warn`
  line and a value in `emfor_peer_clock_offset_seconds`. Votes, liveness and
  the assigned checkers do not change.
- The replay state is in memory only. After the receiver restarts, a message
  captured earlier is accepted once if it is still inside the 5-second
  window. This bound comes from reading the code; it was not measured.

### A local user without privileges

- Can read the store (mode 0640 by default, so only the daemon's group) and
  needs write access to the lock file for lookups (0660 by default). With
  `lock_file_mode = "0666"` any local user can disturb lookups; the daemon
  warns.
- Can try to place or replace files the daemon reads: the config, a fragment,
  a password file, a secret file. The daemon refuses config files that others
  can write, and reads credential files only if they are regular files of a
  bounded size; the mode of credential files gives a warning (the owner is not
  checked, except for the peer `secret_file`).
- Not covered: a user who can write the daemon's config or its plugin
  programs is the operator for emfor's purposes.

### A malicious or compromised plugin

- Runs with the daemon's rights and the unit's sandbox (no new privileges, no
  capabilities, read-only file system, a system call filter, address families
  limited to UNIX and IP). The environment of the child is cleared except for
  what the config lists.
- Not covered: a plugin can still do whatever those limits allow, and it sees
  what the config passes to it (including a password).

### The operator's own mistakes

emfor warns about the common ones: world-readable credential files and a
password sent without TLS to a database check. It does not refuse them,
because each can be intended. A config file that others can write is refused. See the manual, chapter 13.

## What is out of scope

- A compromised host or root: they can read the store, the memory and the
  secrets.
- Denial of service by an attacker who can already exhaust the host's CPU or
  its network.
- Supply chain compromise of the build. Dependencies are checked with
  `cargo deny` and CodeQL; releases are not signed yet (see
  [release-signing.md](release-signing.md)).
- Anything a third-party plugin does.

## Where the defenses are

| Threat | Where | Test |
| --- | --- | --- |
| Hostile backend text | `emfor-daemon` checker output handling | unit tests; fuzz target `plugins` |
| Hostile plugin lines | plugin host line limit | unit tests; fuzz target `plugin_line` |
| DNS floods | `dns.rs` rate limit, TCP cap | unit tests; fuzz target `dns` |
| Peer forgery and replay | `peers/crypto.rs`, `peers/group.rs` | unit tests; fuzz target `peers` |
| Hostile store content | `libemfor` client | client tests; there is no fuzz target for the store |
| Writable config or fragments | `config.rs` `read_trusted` | unit tests |
| Credential files | `emfor-tls` bounded reader, `credential_warnings` | unit tests |
| Unit sandbox | `packages/common/emfor.service` | `systemd-analyze security`, VM runs |

## Known gaps

These are open and stated elsewhere; this page only collects them.

- No signed releases (waiting for a release key).
- No authentication on DNS and `/metrics`.
- Peer secret is shared by the group; no forward secrecy; the sender's
  identity is checked against the UDP source address only. Two colluding
  members (or one holder that can spoof two source addresses) can force a
  backend down; one cannot.
- A message captured before a receiver restart is accepted once inside the
  5-second window (from the code, not measured).
- A recorded peer message can put a member into the "clock offset" state in
  monitoring (see above); it does not change any verdict.
- `tls` of database checks defaults to off; only a warning.
- `MemoryDenyWriteExecute` is not on, because a plugin that generates code
  would fail.
- No penetration test and no DAST.
