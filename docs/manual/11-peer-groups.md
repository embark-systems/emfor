# 11. Peer groups

[Contents](README.md) · Previous: [10. DNS](10-dns.md) · Next: [12. Operating emfor](12-operating.md)

Without a peer group every daemon decides alone, which is simple and
survives any network failure, but has two costs: each host needs `fall`
failed checks of its own to notice a failure, and every host checks every
backend. A **peer group** of the daemons of one data centre shares that work:

- **Failure notices.** When a member sees a backend go from `healthy` to
  `unhealthy`, it tells the others. Each of them, if the backend is `healthy`
  there, starts a **fast recheck run**: checks every 200 ms (or every
  `interval`, if shorter) until a check passes, the backend is `unhealthy`, or
  `fall` checks were made. So the whole group follows the first member within
  about `fall × 200 ms`.
- **Votes.** With three or more live members, three of them are the
  **assigned checkers** of each backend. When two of the three see it down,
  it counts as down on every member, whatever that member's own check says.
- **Fewer checks.** A member that is not an assigned checker of a backend may
  take the result from the votes instead of checking it itself.

Lookups and DNS still read only the local daemon. A member that cannot reach
the others decides from its own checks, as without a group.

## Configuration

Every member has a `[peers]` table, and a `site`:

```toml
site = "dc1"

[peers]
listen = "10.0.1.11:5302"
members = ["10.0.1.11:5302", "10.0.1.12:5302", "10.0.1.13:5302"]
secret_file = "/etc/emfor/peers.key"

[services."api.svc.emfor"]
check = "http"
backends = ["10.0.2.21:8080", "10.0.2.22:8080"]
```

| Key | Default | Rule |
| --- | --- | --- |
| `listen` | required | This host's `ip:port` (UDP), not `0.0.0.0` or `::`. Changing it, or adding or removing `[peers]`, needs a restart. |
| `members` | required | 2–64 addresses with no repeats, all of the family of `listen`, and including `listen`. Reloadable. |
| `secret_file` | required | The shared secret ([below](#the-secret)). Reloadable, and watched for changes. |
| `previous_secret_file` | none | Read at start only, for a restart during a secret change ([below](#rotating-the-secret)). |
| `encrypt` | `true` | Encrypt messages. `false` only signs them. Reloadable. |
| `accept_plaintext` | `false` when `encrypt` is `true`, else `true` | Accept signed messages that are not encrypted. `false` together with `encrypt = false` is an error. Reloadable. |
| `cipher` | `"chacha20-poly1305"` | The cipher of encrypted messages: `"chacha20-poly1305"` (`EMP2`/`EMP3`) or `"aes-256-gcm"` (`EMP4`). `"aes-256-gcm"` needs `encrypt = true`. Reloadable. See [below](#choosing-aes-gcm). |

`site` is required with `[peers]`. Members of a group should be in one data
centre: the votes assume that what one member sees, the others would see too.

All members should list the same services with the same check definition.
Members compare a **digest** of each service's check (the check name or
plugin with its command and `env_pass`, `check_options` and `timeout`); a
member ignores votes and notices for a service whose digest differs, and
`emfor status` shows a digest mismatch. `interval`, `rise`, `fall`,
`balance`, `on_all_down` and the backend list may differ, but a different
service set or backend list costs votes ([below](#when-members-differ)).

Open the UDP port between the members. Each message is one datagram of at most
1200 bytes; every member sends a heartbeat to every other one each second.

### Leaving a service out

`peers = false` on a service (or in `[defaults]`) keeps that service out of
the group on this host: it is checked here at every interval, its results are
not sent, and votes and notices for it are ignored. The host tells the others
in its heartbeats, so they do not count it as a checker of that service.

### When members differ

The ranking below knows only the service name, the backend address and the live
members (minus those that left the service out). It does not know which member
has the service or the backend. So when the members do not list the same
services and backends:

- A member that has no such service, or no such backend, still ranks as a
  candidate on the others, but it never checks the backend and never sends a
  result. The others get fewer than three votes for it. With two voters left,
  both must see the backend down for the votes to apply `down`; a peer's `down`
  alone no longer does. The member ignores the messages for a service or
  backend it does not have (a line at `debug`); the others see the missing
  votes as a **silent checker** ([below](#silent-checkers)).
- A member whose digest differs does send results, but the others store them
  and do not count them, so its place is lost the same way. This is the only
  difference `emfor status` shows: a digest mismatch, until 60 seconds after
  the last message from that member.
- `interval`, `rise`, `fall`, `balance` and `on_all_down` may differ without
  any effect on the votes.

#### Silent checkers

A member that is an assigned checker of a backend, is live and does not leave
the service out, but sends no fresh result for the backend, is a **silent
checker**. Each member looks for them once a second, from what it has kept; it
needs no new message. A member is reported when it has been silent for the
longer of 10 seconds and 6 times the `interval` of the service, so a member
that has just started or become live, and one lost datagram, report nothing.
It shows in four places:

- `emfor status`: `peers: no results from <member> for <service> (<n> backends)`,
  after the lines for digest mismatches;
- `emfor status --json`: `peers.silent_checkers`, one
  `{"service", "backend", "member"}` for each backend; the key is absent when
  nobody is silent;
- the metric `emfor_peer_silent_checkers`, the number of those entries;
- the log: a `warn` when the first backend of a service becomes silent for a
  member, and an `info` when none is left.

The cause is not told. A member without the service or backend, one that loses
the packets of this member only, and one whose check keeps giving `error` (that
sends no result) all look the same. A member with another check definition is
shown as a digest mismatch instead, and one that leaves the service out is not
an assigned checker. A silent checker changes nothing in the votes: it still
holds one of the three places.

Keep the services and their backend lists equal on every member, for example
by generating one `conf.d` file for all of them. A service that only some
members have should be `peers = false` on the members that have it: they check
it alone, and the others do not count them. To give one member another
backend list, use another service name for it.

### Several data centres

A peer group is for one data centre. With two or more, run **one group per
data centre** and nothing across them:

- Give each data centre its own `site`, its own `members` list and its own
  secret. A member whose `site` differs from the receiver's is rejected
  (`reason="site"`, [chapter 16](16-troubleshooting.md)), and a message
  signed with another secret fails authentication first. Two groups on the
  same network must not share a secret.
- Do not put members of two data centres in one group. The votes assume that
  what one member sees, the others would see too, which is not true across
  sites: a member that is cut off from the other site's backend votes `down`
  for a backend the rest can reach. Messages are single UDP datagrams with
  no retransmission; a member is live while a valid message arrived within
  the last 3 seconds, and a message more than 5 seconds from the receiver's
  clock is dropped. Each change in who is live reshuffles which members are
  assigned to check which backend.
  `tools/peer-wan-test.sh` put a delay or a loss on the peer messages of
  three members for 60 seconds each: with a delay of 100 ms, or of 1.5 s ±
  0.5 s, and with 10 % or 30 % loss, A saw both others live the whole time;
  with 50 % loss it saw fewer than two live for 6 of the 60 seconds. The time
  until two down votes made A show a backend `unhealthy` went from 1 second
  (no impairment) to 2 to 5 seconds. So an ordinary link between sites does
  not break the group; a link that loses half of its packets, or delays them
  by more than 3 seconds, does.
- A backend in another data centre is still a backend like any other. Each
  group checks it from its own side, so its state is what that data centre
  sees: if the link between the sites fails, both sides may call the other's
  backends `unhealthy`, and each side keeps working with its own. Use
  `site` on the backends and `balance = "locality"` ([chapter
  8](08-choosing-a-backend.md)) so that a host prefers its own data
  centre's backends and uses the others only when none passes, or mark them
  `backup = true`.
- A check that crosses the WAN costs the hosts that run it. In a group, three
  assigned members of the near data centre check each far backend and the
  others take their results; without a group (`peers = false`), every host
  checks it. Leave `peers` at its default for such services.

The numbers come from three daemons in docker with `tc netem` on the peer
port; clock differences, a real link and the checks of a far backend were not
tried.

## How the votes work

For each backend, every member ranks the candidates (itself and every live
member that does not leave the service out) by an MD5 hash of
`<service>/<backend>/<member listen>`, and takes the top three as assigned
checkers. Every member computes the same three, without talking about it. With
fewer than three candidates there are no assigned checkers and every member
checks alone.

After each check, a member counts the latest results of the three (a result
counts if it is recent, under `3 × interval`, from a live member, with the
same digest):

| Votes | Applied result |
| --- | --- |
| No assigned checkers, or no votes | This member's own result |
| 2 or more `down` | `down`, reason `peers <d>/<f>: <reason of the first down vote>` |
| otherwise | This member's own result; a `down` gets the reason `local: <reason>` |

So a member whose own check passes can still mark a backend down when two of
the three assigned checkers see it down. That is a **disagreement**, logged
once at `info`:
`backend <backend> of <service> is down at <d>/<f> peers but up here`, and
counted in `emfor_peer_disagreements`. A disagreement usually means a network
problem between this host and the backend, or a different check definition.

A member that is not an assigned checker of a backend skips its own check
when two fresh votes agree (`emfor_peer_checks_skipped_total` counts those),
but checks anyway after two skipped turns in a row, when its own last result
was not `up`, during a fast recheck run, or when the votes are not new. Each
result counts at one check time only, so another member's result never moves
`rise` or `fall` twice.

`emfor status` shows how many members are live. It does not name the
assigned checkers; `emfor status --json` lists them for each backend
(`assigned_checkers`, [chapter 12](12-operating.md#emfor-status)). The text
shows only where this host stands: when it is not an assigned checker of some
of the shown backends, the `peers:` line ends in
`, checks <n>/<m> backends here` and those backends are marked `voted`; when
none of them has assigned checkers, the line ends in
`, too few live members to vote`. In a group of three every member is an
assigned checker of every backend, so with all three running the line is
only:

```text
peers: 2/2 live
```

After one member of three stopped:

```text
peers: 1/2 live, not live 127.0.0.3:5302, too few live members to vote
```

(The counts are of the **other** members.) A member is live while a valid
message from it arrived in the last 3 seconds. The log says
`peer group on 127.0.0.1:5302 with 2 other members`, then
`peer 127.0.0.2:5302 is live`, and later
`peer 127.0.0.3:5302 is not live (no valid message for 3s)`.

These outputs come from three daemons on one Debian container
(`127.0.0.1`–`127.0.0.3`); a group across real hosts and a real network was
not tried for this manual.

## The secret

All members share one secret. Messages are authenticated with it, and, with
`encrypt = true`, encrypted. Create it once and copy it to every member:

```sh
# head -c 32 /dev/urandom | base64 > /etc/emfor/peers.key
# chown root:emfor /etc/emfor/peers.key
# chmod 0640 /etc/emfor/peers.key
```

The file must:

- be an absolute path, readable by the daemon;
- be owned by root or the daemon's user (the group does not matter);
- not be readable by others, nor writable by group or others;
- hold at least 32 bytes after one trailing line feed is removed, and at most
  65536 bytes in all.

The bytes of the file are the secret; it does not have to be base64.
`emfor check-config` checks the owner against the user who runs it: a
`root:emfor` file, as created above, passes both as the daemon's user and as
root, while a file owned by `emfor` fails when root runs the check.

### Message formats

| Format | Used when | Protection |
| --- | --- | --- |
| `EMP1` | `encrypt = false` | HMAC-SHA256: authenticated, readable on the network. |
| `EMP2` | `encrypt = true`, to a member not yet known to read `EMP3` (0.6 and earlier) | ChaCha20-Poly1305 |
| `EMP3` | `encrypt = true`, to a member that sent `EMP3` or announced it | XChaCha20-Poly1305 |
| `EMP4` | `encrypt = true` and `cipher = "aes-256-gcm"`, to every member | AES-256-GCM |

A member starts by sending `EMP2` to everyone, and switches to `EMP3` for a
member as soon as one of its messages shows it reads `EMP3`; the log says
`sending EMP3 to peer 127.0.0.2:5302 from now on`. You configure nothing.

### Choosing AES-GCM

With `cipher = "aes-256-gcm"` (and `encrypt = true`) a member sends `EMP4` to
every member and no longer chooses between `EMP2` and `EMP3`. Use it when a
policy asks for AES-GCM rather than ChaCha20; the group is no safer
otherwise, and the default stays as it was. Every daemon always *reads*
`EMP4`, whatever its own `cipher` and `accept_plaintext`.

An `EMP4` message has a counter that starts at 1 each time the daemon starts
and grows by one per datagram; it is the nonce, so no nonce is used twice
under one key, and each start of the daemon gets its own key.

**Upgrade order.** A member of an earlier version does not know `EMP4`: it
drops the message and counts it as `malformed`, and shows as not live to the
member that sent it. So upgrade every member first, and set `cipher` only
when all of them run a version that reads `EMP4`; reload each member (`SIGHUP`)
to apply it. To go back, remove `cipher` and reload: the next message is
`EMP2` again, and members that still send `EMP4` are still read.

Every message carries a timestamp, a boot id and a sequence number; a
replayed, too old, malformed or wrongly signed message, one from an address
not in `members`, one from another site, and (by default) a plaintext one are
dropped and counted in `emfor_peer_messages_rejected_total{reason}` as
`replay`, `stale`, `malformed`, `auth`, `sender`, `site` or `plaintext`.

"Too old" is measured against the receiver's own clock, and a timestamp in
the future counts the same way: a message whose timestamp differs from the
receiver's clock by more than **5 seconds**, in either direction, is dropped
as `stale`. The limit is fixed. So the clocks of the members must agree to
well within 5 seconds (chrony or ntpd does that); a member whose clock is
further off than that is never heard by the others, and shows as not live
for them.

The daemon says so before that. From every message that passes the checks up
to the site, including one that is then dropped as `stale`, it measures how far
the member's clock is from its own: the timestamp of the message minus the
host's clock when it arrives (so it includes the time of the datagram, which
inside a data centre is milliseconds). A member whose last three messages were
all at least 2 seconds off in the same direction is reported, until its offset
is below 1.5 seconds or it sends nothing for 10 seconds. It shows in four
places:

- `emfor status`: `peers: clock of <member> is <n.n> s behind this host
  (messages more than 5 s apart are dropped)`, or `ahead of this host`, after
  the lines for silent checkers. When every member of a group of at least
  three is reported, one line `peers: clock of this host differs from every
  member by up to <n.n> s; check this host's clock (...)` replaces them: the
  clock of this host is the likely one. With only one other member nobody can
  tell whose clock is wrong, so the line names that member;
- `emfor status --json`: `peers.clock_offsets`, one `{"member", "offset_ms"}`
  for each member, negative when its clock is behind; the key is absent when
  no member is reported;
- the metric `emfor_peer_clock_offset_seconds{peer}`, in seconds with the same
  sign, for every member that has been measured, reported or not;
- the log: a `warn` when a member is reported and an `info` when it is not any
  more, one of each, besides the `stale` warnings.

Nothing else changes: the offset is not corrected for, and the votes do not
use it.

## Rotating the secret

The daemon notices a change of `secret_file` by itself (inotify on Linux,
kqueue on FreeBSD and macOS, and a read every 30 seconds in any case). No
reload is needed. After a change it:

- goes on **signing** with the old secret for 60 seconds;
- goes on **accepting** the old secret for 120 seconds.

So: write the new secret on every member **within 60 seconds**, and no message
is lost.

```sh
# head -c 32 /dev/urandom | base64 > /tmp/peers.key.new
# for h in host1 host2 host3; do
>   scp /tmp/peers.key.new $h:/etc/emfor/peers.key.new &&
>   ssh $h 'chown root:emfor /etc/emfor/peers.key.new && chmod 0640 /etc/emfor/peers.key.new &&
>           mv /etc/emfor/peers.key.new /etc/emfor/peers.key'
> done
```

Each member logs the change at `info` level, once per change, with
`change=peers_secret_rotated` and `secret_file=<path>` (the path only, never
the secret or a hash of it). It is not a reload line: a rotation needs no
reload. A file written again with the same content logs nothing.

Replace the file with `mv` (a rename), so the daemon never reads half a file.
A file it cannot read, or an invalid one, is ignored: it keeps the last valid
secret and warns at most once a minute. Watch
`emfor_peer_messages_rejected_total{reason="auth"}`: it rises when members
use different secrets.

### A restart during the change

A daemon that restarts in the middle of a change knows only the new secret,
and drops the messages of members that still sign with the old one until they
switch. To bridge that, keep the old secret in `previous_secret_file`:

```toml
site = "dc1"

[peers]
listen = "10.0.1.11:5302"
members = ["10.0.1.11:5302", "10.0.1.12:5302", "10.0.1.13:5302"]
secret_file = "/etc/emfor/peers.key"
previous_secret_file = "/etc/emfor/peers.key.old"
```

`previous_secret_file` is used only if all of this holds: the daemon's record
`<store>-peers` (`/var/lib/emfor/state.mdb-peers`, mode 0600) says that exactly
that secret was replaced, less than 120 seconds ago. The record holds SHA-256
fingerprints and times, never a secret. So an old secret cannot come back
later because a file still holds it. At the **first start after an upgrade
from 0.6** there is no record yet, and `previous_secret_file` is ignored, with
one `info` line.

### Many members

The loop above changes one host after the other, and each `scp` and `ssh`
takes time; with a dozen members or more it can run past the 60 seconds. Stage
first, switch together: copy the new secret to `/etc/emfor/peers.key.new` on
every member (nothing reads that name), check the owner and mode on each, and
only then run the `mv` on all members at the same moment, from a parallel tool
(pdsh, Ansible with `forks`, a `xargs -P` loop). A rename takes no time, so the
spread is a few seconds. Afterwards watch
`emfor_peer_messages_rejected_total{reason="auth"}`: a member that did not
switch shows up there on all the others.

If a member missed the window, it signs with the old secret and the others
drop its messages until you replace its file too; there is nothing to repair
on the others.

## Members from a cloud API

`members` can be changed with a reload, so a group whose hosts come and go
can be kept up to date by a timer.
[examples/deploy/emfor-peers-sync.sh](../../examples/deploy/emfor-peers-sync.sh)
takes the list of members on standard input, writes it to one `conf.d` file
that holds only `[peers] members`, runs `emfor check-config` as the daemon's
user, puts the old file back if that fails, and reloads:

```sh
# some-command-that-lists-the-hosts | /usr/local/sbin/emfor-peers-sync.sh /etc/emfor/conf.d/50-peers.toml
```

It keeps the old file when the list has fewer than 2 or more than 64 entries
(an empty answer from the API is the usual cause) or a line that is not an
address. The rest of `[peers]` stays in the main file. A member added by a
reload starts as not live; a removed one is forgotten. The CHANGELOG records
that this script was run in a test container; it was not run for this manual.

## Upgrading a group from 0.6

- 0.7 refuses plaintext (`EMP1`) messages by default. If your 0.6 group ran
  with `encrypt = false`, either turn encryption on everywhere, or set
  `accept_plaintext = true` on the 0.7 members until every member is
  upgraded.
- The secret file must now be owned by root or the daemon's user.
- The first 0.7 start writes the `<store>-peers` record; a restart during a
  secret change at that moment cannot use `previous_secret_file`.

[Chapter 15](15-upgrading.md) has the whole list.

## Metrics

| Metric | Meaning |
| --- | --- |
| `emfor_peer_up{peer}` | 1 while the member is live. |
| `emfor_peer_messages_rejected_total{reason}` | Dropped messages by reason. |
| `emfor_peer_notifications_total{direction}` | `down` notices sent and received. |
| `emfor_peer_results_total{direction}` | Check results sent and received. |
| `emfor_peer_disagreements` | Backends that are down by votes but up here. |
| `emfor_peer_silent_checkers` | Assigned checkers that send no results for a backend (one per service, backend and member). |
| `emfor_peer_clock_offset_seconds{peer}` | The clock of the member minus this host's clock, in seconds; negative when the member is behind. |
| `emfor_peer_checks_skipped_total` | Check times decided by votes alone. |
| `emfor_peer_queue_dropped_total` | Messages dropped because the peer task was behind. |
