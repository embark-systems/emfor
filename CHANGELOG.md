# Changelog

## Unreleased

### Daemon

- A plugin that stops answering is restarted. Until now only a plugin
  that exited or closed its stdout was restarted: one that kept running
  but answered nothing (stuck in a loop, deadlocked, stopped) left every
  check of its backends `error` for good, and the requests for it queued
  up without bound. Now, when the callers of 3 requests in a row give up
  waiting and no answer to any request arrives in between, the daemon
  logs that the plugin stopped answering, kills it and restarts it with
  the usual backoff. A plugin that answers within `timeout_ms` is never
  affected (see [Plugin protocol](docs/plugins/protocol-v1.md#rules)).
- A single key or value in the store that is not UTF-8 no longer blocks
  the daemon. Under `svc/` it made every commit fail from start-up on,
  so clients saw a stale store for good; under `hist/` it kept every
  service's history from loading. Such entries are now deleted, or
  discarded with a warning, on their own.
- `http` `host_header` must be a host with an optional port
  (`api.internal`, `api.internal:8443`, `[2001:db8::1]`). Values with
  spaces, a path or a scheme passed `check-config` but made every check
  down, or silently checked whatever host the name resolved to instead
  of the backend.
- State changes already queued when one is written are committed in the
  same transaction, so a burst of changes (a network split) costs one
  commit, not one per backend.
- A reload that changes `interval` or `jitter` applies to the wait
  already under way. Before, going from `10m` to `2s` took up to 11
  minutes to apply.
- The 200ms checks while a backend is `unknown` stop after `rise + fall`
  in a row; results that never reach either count no longer check about
  5 times a second for good.
- `interval` is at most `24h` and `timeout` at least `10ms`. Larger
  intervals could panic the check task (with a huge `fall`), and a 1ms
  timeout gave plugins 0ms.
- `[store] map_size` is rounded up to a multiple of 64KiB. A size LMDB
  refuses (such as `"1048577"`) passed `check-config` and then stopped
  the daemon from starting.
- A check task that is restarted after a bug now records the backend's
  move to `unknown` as a transition (reason `check task restarted`);
  before, the stored state changed silently.
- The DNS server's view of a service is updated per backend instead of
  copying the whole service on every check.
- DNS over TCP serves at most 128 connections at once and closes a
  connection with no query for 5 seconds, so clients can no longer use
  up the daemon's file descriptors.
- If a DNS listener stops while the daemon runs, the daemon logs it and
  exits with status 70, so the service manager restarts it; before, DNS
  went silent.
- DNS answers BADVERS to EDNS versions above 0 and REFUSED to classes
  other than IN; before, a CH query got IN records.
- DNS compares names label by label: a label containing a `.` byte no
  longer puts a name outside the zone inside it.
- Over UDP, a reply whose single record or SOA does not fit (names near
  255 bytes) is truncated by emfor itself and keeps its EDNS record.
- Plugins run in a process group of their own. When a plugin exits, is
  stopped, or is killed because it stopped answering, the processes it
  started are killed too; before, they could outlive it.
- A plugin's stderr lines and invalid stdout lines are rate limited: up
  to 100 at once, then 10 a second each. The log notes how many lines
  were dropped, so a plugin writing in a loop can no longer fill the
  log.
- A `ca_file` is parsed again only when it changes (inode, size or
  times), not on every check. A replaced file still takes effect at the
  next check.
- http clients for checks (one per backend with a domain `host_header`)
  are built off the async worker threads, because building one reads the
  system CA store. The peer `secret_file` is read off them too.
- Documented: a connection that fails because this host has no route to
  the backend, or its own firewall blocks it, is `down`, not `error`.
  That has always been the behaviour.

### Peers

- A kept result counts once. A member kept another member's latest result
  for 3 of the sender's intervals and counted it at every one of its own
  check times in that span, so with `interval = "1s"` here and `"30s"` at
  the assigned checkers one `down` from each of two checkers moved
  `fall` up to about 90 times and marked a backend down that the checkers
  themselves still saw healthy. Now each result counts at one check time;
  a check time decided by votes alone needs a new vote, and an own check
  that only repeats counted `down` votes while it passes changes nothing
  but is still counted in `emfor_backend_checks_total`. With equal
  intervals most check times get new results, so little changes (see
  [spec peer-group](docs/spectra/specs/peer-group/spec.md)).
- New optional `[peers] previous_secret_file`: the secret `secret_file`
  held before its last change. A daemon restarted within 120 seconds of
  that change keeps signing with and accepting the old secret as if it
  had not restarted. Before, members whose file had not changed yet
  rejected it as `auth` until they changed. See
  [`[peers]`](docs/manual/03-configuration.md#peers).
- A second change of `secret_file` within 60 seconds of the first no
  longer signs for a minute with the intermediate secret, which members
  whose file had not changed did not know. The daemon keeps signing with
  the earlier one, and accepts every secret replaced in the last 120
  seconds.
- Peer messages that arrive out of order are accepted once, instead of
  being dropped as `replay`. An `EMP1` message whose tag does not match
  now counts as `auth` even when its payload is not JSON: the tag is
  checked before the payload is parsed. A message of a kind this version
  does not know is accepted (for liveness) even when it reuses a known
  field name with another type.
- A flood of datagrams from many source addresses no longer suppresses
  the warnings about real members' dropped messages.
- A `result` or `down` sent after a reload that changed `check`,
  `check_options` or `timeout` during the check now carries the check
  definition digest the check ran with, not the new one.

### emfor-check

- Reasons longer than 512 characters are cut to 512 followed by `…`
  before they are sent (the daemon keeps no more). A backend's long error
  text, such as a 70 KB redis `-ERR`, made the answer line longer than the
  daemon's 65536-byte limit, so the daemon restarted the plugin, again at
  every check of that backend.
- `mongodb`: a server asking for more than 1000000 SCRAM iterations is
  down with `auth: server asks for <n> SCRAM iterations, more than
  1000000`; one asking for 4294967295 held a worker thread for many
  minutes. The key derivation now runs off the async workers, so the
  check's timeout ends a slow login.
- `sip` over TCP or TLS: a response with a Content-Length near the
  largest 64-bit number made the plugin loop forever on the same
  provisional response, holding a worker thread at full CPU, with no
  timeout to end it. Such a response is now `down` with
  `query: response longer than 65536 bytes`.
- A check that panics is answered `error` with `check panicked:
  <message>` (emfor-plugin crate). Until now it sent no answer, so the
  daemon waited out the timeout.
- `mysql`: a NULL or non-integer answer to `SELECT 1` or
  `@@global.read_only` is down with `query: <query> answered <value>,
  want an integer`. Until now the check panicked and the backend ended
  up `unknown`.
- `mysql`: a check cut off by its timeout closes its connection within 1
  s. mysql_async's cleanup task waited with no limit for a server that
  stalled mid-result, leaking a task and a socket at every check.
- `mysql`: a CA certificate that cannot be used as a root is skipped, as
  in the other plugins, instead of failing every TLS handshake.
- `redis`: a reply over 2 MiB is down with `query: invalid reply: reply
  too long`, and a partly received reply is parsed again only after a
  line feed arrives.
- `mongodb`: a `hello` reply nesting documents more than 100 deep is
  down with `query: malformed reply`. A reply of about 8000 levels
  overflowed the stack and aborted the plugin.
- `mongodb`: validation refuses a `password_file` whose password is not
  printable ASCII. Until then such a service was `error` at every check.
- `quic`: HTTP/3 response headers are limited to 65536 bytes (h3 allowed
  any size).

### emfor status

- `emfor status | head -1` keeps the normal exit status instead of
  exiting 2 silently; other output write errors are printed.
- `--watch` opens the store again when the file is replaced, instead of
  showing the old file as stale forever.

### Clients

- A running application keeps reading the store after the daemon is
  restarted with a larger `[store] map_size` and the store grows past
  the old size. Before, every lookup failed with `UNAVAILABLE`
  (`MDB_MAP_RESIZED`) until the application restarted.
- A child forked while other threads of the parent were looking up no
  longer hangs in its first lookup.
- A store file that exists but cannot be opened is tried again at most
  once a second; lookups in between fail with the same `UNAVAILABLE`
  error. A missing store is still tried on every lookup.
- `emfor.h` no longer defines the internal macros `REPLACE_CHECK_MS` and
  `MAX_STICKY_WEIGHT`.
- Python: the library file named for this release (`libemfor.so.0.4`,
  `libemfor.0.4.dylib`) is tried before what `ctypes.util.find_library`
  finds, which may be another release's.
- C API: `emfor_lookup` called from a thread-local destructor during
  thread exit returns a status instead of aborting the process.

### Deployment

- The example systemd unit sets `LimitNOFILE=65536`.

### Development

- `tests/plugins/mysql.sh [version]` runs the `mysql` plugin's live tests
  against MySQL from the `mysql` Docker images (8.4 unless given), with
  the layout `tests/plugins/run.sh` uses for MariaDB. They pass on MySQL
  8.0 and 8.4, so the 0.4.0 known limit that the plugin was checked only
  against MariaDB no longer holds for those versions.
- `emfor-conformance` writes a new store just before each run of the
  lookup program, so slow runs earlier in a case cannot make a `fresh`
  store stale. A failure in a run longer than the staleness limit says
  so. An expected error now needs a non-empty message.

## 0.4.0 — 2026-11-14

**Breaking: the store format is version 2.** The daemon writes
`meta/schema_version` 2, which adds the backend field `backup`. Clients
and `emfor status` from this release read versions 1 and 2; older ones
fail every lookup on a version 2 store with `INCOMPATIBLE`. Upgrade the
clients on a host before the daemon (see
[Store format version](docs/manual/06-clients.md#store-format-version)).

**Peer messages are encrypted by default.** Members running 0.3.0 cannot
read them and drop them as `malformed`. To upgrade a peer group, set
`encrypt = false` in `[peers]` on each host as it is upgraded; once every
member runs this release, remove the line and reload each daemon.

**Breaking, on FreeBSD only: `examples/rc.d/emfor` is gone**, and the
example port keeps the store in `/var/db/emfor` (see Deployment).

The main additions are backup backends, slow start, a preference for
backends whose last check passed, sticky lookups that honour `weight`,
encrypted peer messages, and the `smtp`, `amqp`, `mongodb`, `grpc` and
`quic` plugins of `emfor-check`. The plugin protocol (1) is unchanged.
As with every release, install `libemfor` from the same release as the
daemon.

### Daemon

- Peer group messages are encrypted (`EMP2`: ChaCha20-Poly1305 with a key
  derived from `secret_file` by HKDF-SHA256 and a random nonce per
  message), so backend names, addresses and reasons no longer travel in
  the clear. New `[peers] encrypt` (default `true`, applied on reload):
  `false` sends the signed plaintext `EMP1` of 0.3.0. Both formats are
  accepted either way; with `encrypt = true` the daemon warns once a
  minute per member that still sends `EMP1`. A message that does not
  decrypt counts as `auth` in `emfor_peer_messages_rejected_total`.
  Secret rotation works as before. There is no forward secrecy: whoever
  later gets the secret can read recorded messages, so rotate it
  regularly. See [`[peers]`](docs/manual/03-configuration.md#peers).

- Backup backends: a backend table takes `backup = true`. Backups are
  used only while no other backend passes its checks: clients and the DNS
  server take the primaries whose last check passed, else the backups
  whose last check passed, else the healthy primaries, else the healthy
  backups, and `balance` works within that set; `fastest` decides which
  backends are fast among the primaries and among the backups
  separately. When no backend is
  healthy, `on_all_down` decides as before and backups count like the
  others. SRV answers list only the chosen set. At least one backend of a
  service must not be a backup, and `backup` is applied on reload. See
  [Backup backends](docs/manual/05-backend-selection.md#backup-backends).
- Clients and the DNS server leave out a healthy backend whose last
  check failed (its `reason` is set) while another healthy backend passed
  its last check, so a backend stops being chosen at its first failed
  check instead of after `fall` of them. When every healthy backend is
  failing, all of them are used as before. The store format is
  unchanged.
- `slow_start` (per service, or in `[defaults]`; `"0s"`, off, by
  default, else `1s` to `1h`): a backend that turns from unhealthy to
  healthy gets a growing share of traffic over that time, in 10 steps,
  instead of its full share at once. It applies to client lookups and
  DNS A/AAAA answers for every method but `failover` and `sticky` with a
  key, and to SRV weights. The store gains `slow_start_ms` on services
  and `recovered_at` on backends; clients that do not know them (as
  emfor-client 0.3.0 reading a version 1 store) give a recovered backend
  its full share at once. See
  [Slow start](docs/manual/05-backend-selection.md#slow-start).
- With `on_all_down = "open"` and no healthy backend, clients and the DNS
  server now use the `unknown` backends (not decided yet, or whose checks
  only give errors) before the `unhealthy` ones, which failed `fall`
  checks in a row: first the `unknown` ones whose last check passed, then
  every `unknown` one, then every backend as before. `closed` and a stale
  store are unchanged, and so is the store format; older clients still
  choose among all backends.
- The `http` check takes `expect_body`: text the response body must
  contain, for health endpoints that answer 200 either way. The body is
  read only when the status matches, up to 64 KiB.
- On macOS, `exec` check commands and plugins no longer inherit the
  daemon's sockets and pipes. A plugin could keep a check connection
  open for as long as it ran, so the backend never saw it closed.
- Peer groups notice a failure that only a member which is not one of the
  backend's assigned checkers sees sooner (the known limit of 0.3.0).
  Such a member now checks the backend itself at least every third
  `interval` instead of every fifth, and when its own check fails while
  the assigned checkers see the backend up it starts a fast recheck run
  (every 200 ms, up to `fall` checks), as after a peer's `down` report.
  With the defaults the backend is down there after at most about 6.4
  seconds instead of about 14 (estimated from the schedule; 6 without a
  group). In exchange a backend gets about 3 + (N − 3) / 3 checks per
  `interval` from N members instead of 3 + (N − 3) / 5. Measured with
  `interval = "2s"` and `fall = 3`: 0.8 to 5.8 seconds, 3.3 seconds
  median, over 12 failures started at different points of the schedule.
### Clients

- **Behaviour change:** `balance = "sticky"` with an affinity key now
  honours `weight`: a backend of weight w gets w shares of the keys. Its
  value is the highest of `MD5(key + id)` and `MD5(key + id + "#" + i)`
  for i from 1 to w − 1, integers only, so every host and language still
  picks the same backend for a key. With every weight 1 nothing changes.
  A sticky service that already sets other weights moves some keys when
  its clients are upgraded, and old and new clients pick differently for
  those keys meanwhile, so upgrade the clients of such a service
  together. Raising a weight only moves keys to that backend; slow start
  does not apply, and a weight above 100 in the store counts as 100. A lookup costs one MD5 per unit of weight: 0.8 µs for 10
  backends of weight 1, 72 µs for 10 of weight 100 (Apple M4 Max). The
  Rust API adds `testing::rendezvous_pick_weighted`. See
  [sticky](docs/manual/05-backend-selection.md#sticky).

### emfor-check

- `role = "replica"` for `mysql` and `pgsql`, and for all three database
  plugins it now means a replica that is replicating: a redis replica
  whose link to its master is `connected`, a MySQL or MariaDB replica
  whose IO and SQL threads both run, a PostgreSQL standby with a WAL
  receiver. Otherwise the backend is down with a `replication: ` reason.
  **A redis replica that was up with `role = "replica"` while its link
  was down is now down.**
- `max_lag`: with `role = "replica"`, the seconds a replica may be behind
  before it is down. See [Replicas](docs/deploy/plugins.md#replicas) for
  what each plugin measures and the rights the checks need.
- `emfor-check smtp` checks SMTP servers: the greeting must be 220 and
  `EHLO` must answer 250, on a new connection, optionally upgraded with
  `STARTTLS` (`starttls = true`) or TLS from the start (`tls = true`,
  port 465); then it sends `QUIT`. It never logs in or sends mail. It is
  down with `greeting: `, `ehlo: ` or `tls: STARTTLS not offered`. See
  [docs/deploy/plugins.md](docs/deploy/plugins.md#smtp).
- `emfor-check amqp` checks AMQP 0-9-1 brokers such as RabbitMQ: a
  `PLAIN` login and Connection.Open of `vhost` on a new connection, over
  plaintext or TLS, then a normal Connection.Close. It is down with
  `auth: 403 ACCESS_REFUSED - …` for a rejected login and
  `vhost: 530 NOT_ALLOWED - …` for a vhost the user cannot open. See
  [docs/deploy/plugins.md](docs/deploy/plugins.md#amqp).
- `emfor-check mongodb` checks MongoDB 4.4 and later: `hello` on a new
  connection, plaintext or TLS, an optional SCRAM-SHA-256 login (`user`,
  `password_file`, `auth_source`), and `role = "primary"` (a replica set
  primary, a standalone server or mongos) or `role = "secondary"`, so
  `balance = "failover"` follows the replica set's primary. See
  [docs/deploy/plugins.md](docs/deploy/plugins.md#mongodb).
- `emfor-check grpc` checks gRPC services with the standard health
  service: `grpc.health.v1.Health/Check` over HTTP/2, plaintext or TLS
  with ALPN `h2`, is up when it answers `SERVING`. It is down with
  `status: NOT_SERVING`, `grpc: 12 UNIMPLEMENTED` (no health service) or
  `grpc: 5 NOT_FOUND: …` (an unknown `service`). See
  [docs/deploy/plugins.md](docs/deploy/plugins.md#grpc).
- `emfor-check quic` checks QUIC servers over UDP: a new QUIC version 1
  handshake on every check, without 0-RTT, offering the ALPN values of
  `alpn` (default `["h3"]`) and verifying the certificate (`tls_verify`,
  `server_name`, `ca_file`; always TLS 1.3). It is up when the handshake
  completes. With `http3 = true` it also sends an HTTP/3 `GET` for `path`
  and judges `expect_status` and `expect_body` like the built-in `http`
  check. It is down with `quic: version negotiation failed`, `tls: …`
  (for example `tls: no application protocol (ALPN)`),
  `quic: closed by peer: <code>[: <reason>]`, `quic: stateless reset`,
  `query: HTTP/3: …`, `unexpected status <n>` or `expect_body: got "…"`.
  UDP has no "connection refused", so a closed port or a firewall that
  drops UDP is a `timeout`. The plugin uses quinn 0.11, h3 0.0.8 and
  h3-quinn 0.0.10, which make the `emfor-check` release binary about
  1.3 MiB (13%) larger, 11.2 MiB on macOS arm64; the daemon does not
  link them. See [docs/deploy/plugins.md](docs/deploy/plugins.md#quic).
- `ldap` reasons no longer end with a NUL byte when the server is Active
  Directory, which ends its diagnostic message with one: a wrong password
  is now `auth: 49 invalidCredentials: 80090308: LdapErr: …, data 52e, …`.
- A server that closes or resets the connection during the TLS handshake
  is down with `tls: connection closed during the TLS handshake (does the
  server have a certificate?)` instead of `tls: tls handshake eof` or
  `tls: Connection reset by peer (os error 54)`. Active Directory does
  this on port 636 when it has no certificate. The same reason applies to
  the daemon's `tcp` check with `tls = true`.

### Deployment

- The FreeBSD rc.d script starts `daemon(8)` with `-f`. Without it,
  `daemon(8)` and emfor kept the terminal or SSH session that ran
  `service emfor start`, so that session never closed. The rc.d script,
  the port and the tests have now run on FreeBSD 15.1-RELEASE (arm64).
- The example FreeBSD port puts the store in `/var/db/emfor`, where
  FreeBSD keeps daemon state, instead of `/var/lib/emfor`: it creates that
  directory, and its sample config sets `[store] path =
  "/var/db/emfor/state.mdb"`. The daemon's and the clients' default stay
  `/var/lib/emfor/state.mdb`, so applications on the host set
  `EMFOR_STORE`. After updating an installed port, move the store or keep
  the old `[store] path`; the history is all it holds.
- `examples/rc.d/emfor` is gone. The port's
  `examples/freebsd/files/emfor.in` is the only rc.d script; to use it
  without the port, fill in `%%PREFIX%%` (chapter 2 of the manual shows
  the command). Its `emfor_user` defaults to `emfor`, not `root`, so
  create that user or set `emfor_user`.

### Documentation

- The operations chapter of the manual says what a host crash loses with
  `[store] sync = false` (the state history only) and how to keep the
  store on a tmpfs ([Keeping the store in
  memory](docs/manual/08-operations.md#keeping-the-store-in-memory)).
- [docs/deploy/plugins.md](docs/deploy/plugins.md) says which servers
  each plugin was checked against, and what was checked by hand against
  Active Directory.

### Development

- cargo-fuzz targets for plugin answers, peer messages, metrics, plugin
  lines and the config ([fuzz/](fuzz/README.md)); RELEASING.md runs each
  for 10 minutes before a release.

### Known limits

- FreeBSD has been tested on 15.1-RELEASE (arm64, in a VM), the port with
  `make stage check-plist install`; not with poudriere, on 14.x or on
  amd64.
- Peer messages have no forward secrecy: whoever later gets
  `secret_file` can read recorded messages. Rotate the secret regularly.
- The `ldap` plugin has been checked against Active Directory only
  without a certificate and without a successful login; the `mysql`
  plugin only against MariaDB.
- Unchanged from 0.1.0: lookup throughput across threads levels off at
  about two million a second on Linux
  ([docs/performance.md](docs/performance.md)).
- Unchanged from 0.2.0: with `[store] sync = false` (the default), a host
  crash can leave the store damaged; the daemon then sets it aside at the
  next start and the state history is lost.
- Unchanged from 0.3.0: a host announces at most 80 services left out of
  the peer group; the other members still count it as a candidate for
  the rest.

## 0.3.0 — 2026-11-14

**Breaking, on FreeBSD only: the rc.d script keeps its pid files in
`/var/run/emfor/`.** Scripts that read `/var/run/emfor.child.pid` must read
`/var/run/emfor/emfor.child.pid`, and installed copies of
`examples/rc.d/emfor` must be replaced (see
Deployment). Nothing else needs a change: configs, commands and the
`--json` output of 0.2.0 work as before.

The main addition is peer groups: the daemons of one site can pass
backend failures on, vote on a backend's state and split the checks
between them. Without a `[peers]` table the daemon behaves as in 0.2.0.
The store format (`meta/schema_version` 1) and the plugin protocol (1) are
unchanged; the new store key `meta/peers` is read only by `emfor status`.
As with every release, install `libemfor` from the same release as the
daemon.

### Daemon

- New optional `[peers]` table: the emfor daemons of one `site` that list
  each other form a peer group. Members send each other a heartbeat every
  second over UDP, signed with HMAC-SHA256 using the secret in
  `secret_file`. When a member sees a backend go from healthy to unhealthy
  it tells the others, which recheck that backend at once and every 200 ms
  (up to `fall` checks) instead of waiting for their next interval. A
  report never changes a backend's state by itself. Reports are shared
  only for services whose `check`, `check_options` and `timeout` match.
  New metrics `emfor_peer_up`, `emfor_peer_messages_rejected_total` and
  `emfor_peer_notifications_total`. Without `[peers]` nothing changes.
  See
  [docs/manual/03-configuration.md](docs/manual/03-configuration.md#peers)
  and [docs/deploy/daemon.md](docs/deploy/daemon.md).
- Majority vote: with three or more live members, each backend has three
  assigned checkers, chosen by rendezvous hashing over the live members.
  They send their own check results to the others, and two `down` votes
  mark the backend down on every member, also where its own check passes;
  the reason then reads `peers 2/3: …`, and a failure only this host sees
  reads `local: …`. With fewer members or no recent vote each daemon
  decides from its own checks as before. `emfor status` shows a `peers:`
  line, members whose check definition differs, and the marker `local-up`;
  `--json` has `peers`, from the new store key `meta/peers` (format
  version stays 1, clients ignore it). New metrics
  `emfor_peer_results_total` and `emfor_peer_disagreements`.
- Split checks: a member that is not one of a backend's assigned checkers
  no longer checks it every interval. While two assigned checkers agree
  their results decide (`up`, or `down` with `peers 2/3: …`); it still
  checks the backend itself, with the service's own check, at least every
  fifth interval, every interval while its own check fails, and whenever
  the votes do not agree. A failure only such a member sees is noticed up
  to about five intervals later. New metric
  `emfor_peer_checks_skipped_total`.
- New setting `peers` (default `true`), per service or in `[defaults]`:
  `peers = false` leaves a service out of the peer group on this host. It
  sends no reports or results for it, ignores those it receives, and
  decides from its own checks only; a reload applies it and keeps the
  backends' state. `meta/peers` lists such services in `services_off`.
  The host announces those services in its heartbeats (`off`: the first
  4 bytes of SHA-256 of each name, as 8 hex digits, at most 80; with more
  it logs a warning), and the other members leave it out when choosing
  those services' assigned checkers, so each host can set it on its own.
  Each entry of `members` in `meta/peers` has `services_off` with the
  services that member leaves out, and `emfor status` uses it for `voted`
  and `assigned_checkers`.

### emfor status

- `emfor status` marks backends this daemon is not an assigned checker of
  with `voted`, adds `checks N/M backends here` to the `peers:` line when
  there are any, and `--json` has `assigned_checkers` (each backend's
  assigned checkers, worked out from `meta/peers`).
- `emfor status` shows, after the `peers:` line, one line per member that
  leaves shown services out of the peer group:
  `peers: 10.0.0.6:5302 leaves out api.svc.emfor db.svc.emfor`. Only the
  services shown are listed (so `--service` narrows it); `--json` is
  unchanged.
- `emfor status --watch` scrolls when the services and recent changes do
  not fit in the terminal: the `store:` and `daemon:` lines stay at the
  top, PgUp/PgDn (or Space) move one screen, Up/Down one line, Home/End
  to the start or end, and the last row shows which lines are on screen.
  Lines wider than the terminal are cut instead of wrapped. `q` or Ctrl-C
  ends the watch, and the terminal is restored also after a crash. When
  it cannot install its signal handlers it now exits 2 instead of 0.

### emfor-check

- `emfor-check dns` checks DNS servers: it sends one query (`name`,
  `type`, over UDP or TCP) and is up when the response code is one of
  `expect_rcode` (default `NOERROR`) and, with `expect_answer = true`, the
  answer has a record of the asked type. Over UDP the query is sent again
  once at half of the `timeout`. Reasons start with `rcode: `, `answer: `,
  `query: `, `connect: ` or `timeout`. See
  [docs/deploy/plugins.md](docs/deploy/plugins.md).
- `emfor-check radius` checks RADIUS servers: it sends a Status-Server
  (RFC 5997) by default, or an Access-Request with PAP for a test
  account, with Message-Authenticator first, and is up when the server
  answers with an accept signed with the shared secret from
  `secret_file`. Reasons start with `auth: `, `query: `, `connect: ` or
  `timeout`; servers that check Message-Authenticator, such as
  FreeRADIUS, do not answer a request made with the wrong secret, which
  then shows as `timeout`. See
  [docs/deploy/plugins.md](docs/deploy/plugins.md).
- `emfor-check sip` checks SIP servers with an OPTIONS request over UDP,
  TCP or TLS, and is up when the final status code is in
  `expect_status`, by default 200 to 499: a proxy that answers 404 or
  407 is working, while 503 (overload, maintenance) and other 5xx and
  6xx codes are down with `status: <code> <reason>`. See
  [docs/deploy/plugins.md](docs/deploy/plugins.md).
- `emfor-check diameter` checks Diameter nodes (HSS, PCRF, OCS, DRA)
  over TCP or TLS: a capabilities exchange and a device watchdog on a
  new connection, then a Disconnect-Peer-Request. It is down with the
  Result-Code and its RFC 6733 name, such as
  `cea: 3010 DIAMETER_UNKNOWN_PEER` or
  `cea: 5010 DIAMETER_NO_COMMON_APPLICATION`. TLS can present a client
  certificate (`tls_cert_file`, `tls_key_file`), which nodes such as
  freeDiameter require. See
  [docs/deploy/plugins.md](docs/deploy/plugins.md).

### Deployment

- The FreeBSD rc.d script failed to start emfor with any `emfor_user`
  other than `root`, such as the port's default `emfor`: `daemon: pidfile
  ``/var/run/emfor.child.pid'': Permission denied`. rc.subr runs the whole
  command as `${name}_user`, the daemon(8) supervisor included, and that
  user cannot write `/var/run`. The pid files are now in `/var/run/emfor/`,
  which `service emfor start` creates owned by `emfor_user`, and daemon(8)
  no longer gets `-u` (it needs root). Replace installed copies of
  `examples/rc.d/emfor`; scripts that read
  `/var/run/emfor.child.pid` read `/var/run/emfor/emfor.child.pid` now.

### Documentation

- A user manual in English ([docs/manual](docs/manual/README.md)) and
  Traditional Chinese ([docs/manual.zh_TW](docs/manual.zh_TW/README.md)):
  concepts, installation, configuration, health checks, backend
  selection, clients, DNS, operations and troubleshooting.
- [SECURITY.md](SECURITY.md) (how to report a vulnerability),
  [CONTRIBUTING.md](CONTRIBUTING.md) and
  [docs/compatibility.md](docs/compatibility.md), which says which
  interfaces stay stable across releases.

### Known limits

- Unchanged from 0.2.0: FreeBSD has not been tested on a host yet, peer
  groups included ([tests/resolver/freebsd.sh](tests/resolver/freebsd.sh)
  checks the daemon, the rc.d script and the port example there).
- Unchanged from 0.1.0: lookup throughput across threads levels off at
  about two million a second on Linux
  ([docs/performance.md](docs/performance.md)).
- Unchanged from 0.2.0: with `[store] sync = false` (the default), a host
  crash can leave the store damaged; the daemon then sets it aside at the
  next start and the state history is lost.
- In a peer group, a failure that only a member which is not one of the
  backend's assigned checkers sees is noticed there up to about five
  intervals later than without a group.
- A host announces at most 80 services left out of the peer group; the
  other members still count it as a candidate for the rest.

## 0.2.0 — 2026-11-13

**Breaking: the daemon is started with `emfor server`.** `emfor` without a
subcommand now only prints its usage and exits 2, so running it to see the
options no longer starts a second daemon. When upgrading from 0.1.x, change
the service definition before restarting:

| 0.1.x | 0.2.0 |
| --- | --- |
| `emfor --config X` | `emfor server --config X` |
| `emfor --check-config --config X` | `emfor check-config --config X` |

`--check-config` now only prints `--check-config was replaced by "emfor
check-config"` and exits 2. `emfor status` is unchanged.
`examples/rc.d/emfor`,
[examples/systemd/emfor.service](examples/systemd/emfor.service) and the
FreeBSD port example use the new commands; copies of the 0.1.x versions
must be replaced. See [docs/deploy/daemon.md](docs/deploy/daemon.md#commands).

The store format (`meta/schema_version` 1) and the plugin protocol (1) are
unchanged. As with every release, install `libemfor` from the same release
as the daemon: the daemon and the clients must also agree on the lock
file's reader slots (below).

### Daemon

- **The store is no longer flushed to disk on every commit.** The new
  `[store] sync` setting defaults to `false` (LMDB's `MDB_NOSYNC`): a
  stalled disk could make the heartbeat late and every client on the host
  treat the store as stale. The store is rewritten on every start, so
  nothing in it needs to survive a host crash. Set `sync = true` to keep the
  old behavior. Commits now run on a separate thread.
- When the daemon starts and finds the store damaged (`MDB_CORRUPTED`,
  `MDB_PAGE_NOTFOUND` or `MDB_INVALID`, as after a power loss without
  flushing), it moves the file to `<path>.corrupt`, deletes the lock file,
  and starts with a new store instead of exiting. The state history of
  `emfor status --history` is lost with it. See
  [docs/deploy/daemon.md](docs/deploy/daemon.md#the-store-and-the-disk).
- An application process killed in the middle of a lookup (SIGKILL, OOM,
  a crash) left its LMDB reader slot taken. Each such slot kept every page
  freed after it from being reused, so the store grew until commits failed
  with `MDB_MAP_FULL`, every client then saw a stale store, and restarting
  the daemon did not help while applications kept the store open. The
  daemon now frees these slots when it opens the store, about once a minute
  after that, and right after a failed commit, and logs how many it freed.
- The store's lock file has 1024 reader slots instead of LMDB's default
  of 126. The daemon and every client ask for the same number, since the
  process that opens the store first sizes the lock file.
- While a backend's state is `unknown` (after a start, a reload that adds
  it, or a run of errors) and a check returns `up` or `down`, the next check
  runs after `min(interval, 200ms)` instead of `interval`, and the first
  check starts within that time too. With `interval = "2s"` and `rise = 2`,
  backends are known within about half a second of a start instead of 2 to
  4 seconds, during which services with `on_all_down = "closed"` answered
  `ALL_DOWN`.
- Check reasons are escaped and bounded before they reach the store, the
  logs and `emfor status`: tab, CR and LF become `\t`, `\r`, `\n`, other
  control characters `\u{1b}` and the like, and a reason longer than 512
  characters is cut and ends in `…`. A backend or plugin could send
  terminal escape sequences, or reasons large enough to slow every lookup.
- A plugin line longer than 65536 bytes no longer grows the daemon's memory
  without bound: on stdout the plugin is stopped and restarted as if it had
  exited; on stderr the rest of the line is discarded.
- Signals: `emfor server` installs its signal handlers before it loads the
  config. Loading starts plugins and can take seconds; a SIGHUP in that
  time (a `service emfor reload` or config management reloading right after
  a start) used to kill the daemon, and now reloads once it has started. A
  SIGTERM or SIGINT while loading at start, or during a reload, ends the
  loading at once (a reload is then not applied) and the daemon exits 0,
  instead of waiting up to about 10 seconds for slow plugins.
- If the state writer stopped while the daemon ran, the daemon kept running
  without a heartbeat and every client saw a stale store; it now exits with
  status 70 (`EX_SOFTWARE`), which the systemd unit restarts. A backend
  check task that stopped left its backend in its last state; it is now
  started again from `unknown`, at once and then after 1s, 2s, 4s and so on
  up to 60s if it keeps stopping. Both only happen through a bug (a panic).
- A `[dns] zone` longer than 242 characters made the daemon panic on every
  query in the zone: the SOA name `hostmaster.<zone>` went past the
  253-character limit. Such a zone is now a config error.
- `/metrics` reports the store and the plugins:
  `emfor_store_last_commit_timestamp_seconds`, `emfor_store_used_bytes`
  and `emfor_store_map_size_bytes` (commits fail with `MDB_MAP_FULL` when
  the used bytes reach the map size), and `emfor_plugin_up` and
  `emfor_plugin_restarts_total` per plugin. Before, a crashing plugin showed
  only as `error` checks.
- The `http` checker shares one HTTP client among a service's backends
  unless `host_header` is a domain name (which needs one client per
  backend to connect to it), and no longer builds clients while holding a
  lock.
- `emfor status --watch` restored the terminal only on Ctrl-C. Killed, or
  ended by a closed ssh session (SIGHUP), it left the terminal on the
  alternate screen with the cursor hidden.

### Clients

- Applications kept reading the old store file after it was replaced (the
  daemon moving a damaged store aside, or the file deleted and created
  again), so they saw a stale store until they were restarted. A lookup
  that finds the store stale, or fails with an error other than `ALL_DOWN`
  from a fresh store, now checks whether the store path holds another file,
  at most once every 100 ms per process, and switches to it. When the new
  file cannot be opened yet, lookups keep using the old one. This covers
  every client library.
- Lookups take read locks only, so lookups on many threads no longer queue
  on a mutex, and a round-robin lookup no longer allocates the service name
  each time. Failing and stale lookups run about as fast as successful
  ones.
- The Python client looked for `libemfor.so.0`, a name libemfor is never
  installed under, so with only the runtime library installed (no
  `libemfor.so` symlink, not in the `ldconfig` cache) `import emfor`
  failed. It now looks for the versioned names derived from
  `emfor.__version__`: `libemfor.so.0.2` and `libemfor.0.2.dylib` for 0.2.x.

### Deployment

- The FreeBSD rc.d script has `daemon(8)` start `emfor` again 2 seconds
  after it exits, like `Restart=on-failure` in the systemd unit. The
  supervisor's pid is in `/var/run/emfor.pid` and emfor's in
  `/var/run/emfor.child.pid`. `service emfor reload` checks the config first
  and fails without touching the daemon when it is invalid.
- `service emfor configtest`, which the rc.d script also runs before
  starting, checked the config as root and so started the config's plugins
  as root even with `emfor_user` set. It now runs as `emfor_user`.
  Replace copies of `examples/rc.d/emfor`.
- The example FreeBSD port installs the rc.d script with `USE_RC_SUBR`
  from `files/emfor.in`; a test keeps it in step with
  `examples/rc.d/emfor`.
- The systemd unit and [docs/deploy/daemon.md](docs/deploy/daemon.md) use
  `/usr/local/bin/emfor`, where `cargo install` and the FreeBSD port put
  it, instead of `/usr/local/sbin/emfor`. Change the unit's paths if you
  installed to `sbin`.
- The docs spell out what the default `[store] lock_file_mode = "0666"`
  allows: any local user can write the lock file and so stop the daemon's
  commits or disturb lookups. See "Who can use the store" in
  [docs/deploy/daemon.md](docs/deploy/daemon.md#who-can-use-the-store) for
  limiting the store to a group. The default is unchanged.

### Known limits

- FreeBSD has not been tested on a host yet: neither the daemon nor the
  rc.d script and the port example have run there
  ([tests/resolver/freebsd.sh](tests/resolver/freebsd.sh) checks them).
- Unchanged from 0.1.0: lookup throughput across threads levels off at
  about two million a second on Linux
  ([docs/performance.md](docs/performance.md)).
- With `[store] sync = false` (the default), a host crash can leave the
  store damaged; the daemon then sets it aside at the next start and the
  state history is lost.

## 0.1.1 — 2026-11-13

The repository moved to `github.com/embark-systems/emfor`. The daemon,
`emfor-check`, the store format and the clients behave as in 0.1.0. As
with every release, install `libemfor` from the same release as the
daemon.

### Packaging

- `cargo install --path crates/emfor-daemon` installs only `emfor`. In
  0.1.0 it also installed `emfor-test-plugin`, a plugin used only by
  emfor's own tests; it is now built only with the `test-plugin` feature.
- [docs/deploy/daemon.md](docs/deploy/daemon.md) gives the `cargo install`
  commands for `crates/emfor-daemon` and `crates/emfor-check`, including
  for packaging systems that run `cargo install` per crate. The repository
  root is a workspace, so `cargo install --path .` fails.

### Clients

- The Go module path is `github.com/embark-systems/emfor/clients/go`
  (was `github.com/vanillahsu/emfor/clients/go`). Its versions are tags
  `clients/go/vX.Y.Z`:
  `go get github.com/embark-systems/emfor/clients/go@v0.1.1`.

### Development

- [RELEASING.md](RELEASING.md) lists the steps of a release; a test checks
  that the Python, Perl, PHP and Go clients carry the workspace version.
- `tests/clients/Dockerfile` describes the Debian image the local checks
  run in.

### Known limits

- Unchanged from 0.1.0: FreeBSD has not been tested on a host yet, and
  lookup throughput across threads levels off at about two million a
  second on Linux ([docs/performance.md](docs/performance.md)).

## 0.1.0 — 2026-11-13

The first release. The daemon, `emfor-check` and every client come from
this one version; `libemfor` and the clients built on it must be upgraded
together with the daemon (they share its LMDB build).

### Daemon

- Health checks: `tcp` with optional `send` / `expect` and TLS,
  `http` / `https`, `exec` for Nagios-style scripts, and long-running
  plugins speaking [protocol v1](docs/plugins/protocol-v1.md). Per-service
  `interval`, `timeout`, `rise`, `fall` and `jitter`, with `[defaults]`.
- State store: a local LMDB file, record format version 1, with a
  heartbeat so readers notice a daemon that stopped. Backend state,
  latency, the last failure reason and the last 20 state changes per
  backend.
- Balance methods: `round-robin`, `sticky` (rendezvous hashing on an
  affinity key), `failover`, `weighted`, `locality` (same `site` first) and
  `fastest` (backends within a latency margin of the quickest).
  `on_all_down = "open"` or `"closed"` decides what an all-down service
  returns.
- DNS server (optional `[dns]`): A, AAAA and SRV answers for its own zone
  (default `svc.emfor`), meant to sit behind the host's resolver; a warning
  when a service's backends use different ports.
- `/metrics` (optional `[metrics]`): Prometheus metrics for the daemon,
  every service and backend.
- `emfor status`: the daemon's state, every service and backend, recent
  state changes and flapping backends; `--service`, `--json` and `--watch`.
- `SIGHUP` reloads the config when it is fully valid; `--check-config`
  checks a config without running.
- `EMFOR_PREFIX` at build time sets the default config path
  (`/etc/emfor/emfor.toml` without it).

### emfor-check

- Plugins for redis, MySQL / MariaDB, PostgreSQL and LDAP, with TLS.
- The `emfor-plugin` crate for writing plugins in Rust.

### Clients

- One implementation in Rust (`emfor-client`), also built with cargo-c as
  the C library `libemfor`.
- Go, Python, Perl and PHP clients wrapping `libemfor`.
- Every client returns `ip:port` or one of the error codes `UNAVAILABLE`,
  `INCOMPATIBLE`, `NOT_FOUND` and `ALL_DOWN`, checked against shared
  conformance data (`tests/conformance/v1`). Configured only by
  `EMFOR_STORE` and `EMFOR_MAX_STALENESS`; safe across threads and `fork`.
- A lookup takes about 1 µs; see [docs/performance.md](docs/performance.md).

### Deployment

- Guides for the daemon, systemd, metrics and alerting
  ([docs/deploy/daemon.md](docs/deploy/daemon.md)), for forwarding the DNS
  zone from dnsmasq, unbound, systemd-resolved and macOS
  ([docs/deploy/dns-resolver.md](docs/deploy/dns-resolver.md)), and for
  `emfor-check` ([docs/deploy/plugins.md](docs/deploy/plugins.md)).
- Examples: a fully commented config, a systemd unit (run under systemd
  257), a FreeBSD rc.d script, and Prometheus alerting rules with promtool
  tests.

### Known limits

- FreeBSD is a deployment platform but has not been tested on a host yet;
  the CI job that covers it is run by hand and has not passed yet.
- Lookup throughput across threads levels off at about two million a
  second on Linux; see [docs/performance.md](docs/performance.md).
