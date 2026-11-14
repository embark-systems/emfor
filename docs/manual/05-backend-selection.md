# 5. Choosing a backend

[Manual](README.md) · Previous: [4. Health checks](04-health-checks.md) · Next: [6. Client libraries](06-clients.md)

When an application looks up a service, the client library first finds the
**candidates** (normally the healthy backends whose last check passed, in
config order) and then
lets the service's `balance` choose one. The DNS server does the same for
A and AAAA queries, with a few differences noted below.

## Balance methods

| `balance` | Client lookup | Use it for |
| --- | --- | --- |
| `round-robin` (default) | Candidates in turn | Stateless services |
| `sticky` | The same affinity key gets the same backend while it is healthy | Caches and sessions, where a user should keep hitting one backend |
| `failover` | Always the first candidate in config order | A primary with standbys |
| `weighted` | In proportion to each backend's `weight` | Backends of different sizes |
| `locality` | Candidates in the host's `site` first, in turn | Several data centres |
| `fastest` | In turn among the backends whose checks answer fastest | Backends at different network distances |

### round-robin

Each process keeps one counter per service, starting at a random value, so
the processes on a host spread out without coordinating. Each lookup takes
the next candidate.

### sticky

The application passes an **affinity key**, such as a user or session ID.
For each candidate, the client computes `MD5(key + backend id)` and takes
the candidate with the highest value (rendezvous hashing). The same key
therefore picks the same backend in every process, on every host and in
every language; when that backend goes down, only its keys move, and they
come back when it recovers. Without a key (or with an empty one), `sticky`
works like `round-robin`.

A backend with `weight` w gets w times the keys of a backend with weight 1:
its value is the highest of `MD5(key + id)` and `MD5(key + id + "#" + i)`
for i from 1 to w − 1, so a backend of weight 3 gets 3/5 of the keys
next to two of weight 1. With every weight 1 the picks are the same as
in releases before this one. Raising a backend's weight only moves keys
to that backend, and lowering it only moves keys away from it. Slow start
does not change the weights `sticky` uses with a key. A weight above 100 in
the store, which the daemon never writes, counts as 100. A lookup computes
one MD5 per unit of weight among the candidates: about 0.8 µs for 10
backends of weight 1 and 72 µs for 10 of weight 100 (measured on an
Apple M4 Max), so keep weights small for sticky services that see many
lookups.

### failover

The first healthy backend in config order. List the primary first. With
the database plugins' `role` option, only the current primary passes its
check, so `failover` follows a switchover once the new primary reaches
`rise`.

### weighted

```toml
backends = ["10.0.4.11:9200", { addr = "10.0.4.12:9200", weight = 3 }, "10.0.4.13:9200"]
```

Over every 5 lookups in one process, `10.0.4.12` is returned 3 times and
the others once each. Only healthy backends count; their weights are
summed again when one goes down.

### locality

```toml
site = "dc1"

[services."cache.svc.emfor"]
check = "tcp"
balance = "locality"
backends = [
  { addr = "10.0.5.11:6379", site = "dc1" },
  { addr = "10.0.5.12:6379", site = "dc1" },
  { addr = "10.1.5.11:6379", site = "dc2" },
]
```

While any backend in the host's site is a candidate, lookups rotate over
those; otherwise over all candidates. Without a top-level `site`,
`locality` works like `round-robin`.

### fastest

The daemon decides which healthy backends are fast, from their smoothed
check latency, and writes this **pool** to the store; clients rotate over
the pool. A backend joins the pool when its latency is within 1.5 times
the lowest (or 1 ms above it), and leaves when it is over 2 times the
lowest (and more than 2 ms above it), so that small variations do not move
backends in and out. When the pool is empty, or the store is stale,
`fastest` works like `round-robin`.

### Unknown values

A client that reads a `balance` it does not know (written by a newer
daemon) uses `round-robin`. A backend `state` it does not know counts as
`unhealthy`, so such a backend is used only when an `open` service has
nothing better, and an `on_all_down` it does not know counts as `open`,
the default. `emfor status` shows them as `unrecognized` and
`on_all_down=unknown`. Unknown fields in the store are ignored.

## A backend whose checks start failing

A backend turns unhealthy only after `fall` failed checks in a row; until
then it is still healthy, but its last check failed and its `reason` is
set (as `emfor status` shows). While at least one healthy backend passed
its last check, such a failing backend is left out of the candidates, so
lookups and DNS answers stop choosing it at its first failed check. When
every healthy backend is failing, all of them are candidates, as before.
A `down` and an `error` result count alike: if one checker problem hits
every backend, nothing changes.

- The client sees a new `reason` up to about one second late (the daemon
  writes `reason` changes to the store with its heartbeat); the DNS server
  answers from the daemon's memory and sees it at once.
- With `sticky`, the keys of a failing backend move to the other
  candidates and come back after its next passed check.
- In a peer group, this host's own failed check counts even while its
  peers still find the backend up (the reason starts with `local: `), so
  this host stops choosing it at once.

## Backup backends

A backend with `backup = true` is held back for when the others cannot
serve, for example a replica in another data centre or a smaller machine.
While the store is fresh, the candidates are the first of these that is
not empty:

1. the primary backends (not backups) whose last check passed;
2. the backups whose last check passed;
3. the healthy primary backends, failing ones included;
4. the healthy backups, failing ones included.

So backups take over as soon as no primary passes its checks, not only
after the primaries turn unhealthy, and they give way again at a
primary's next passed check. Within the chosen set, `balance` works as
usual: `failover` takes the first backup in config order, `weighted`
uses the backups' weights, and so on. `fastest` decides which backends
are fast among the primaries and among the backups separately, so a fast
backup does not push the primaries out of the pool. When no backend is
healthy at all,
`on_all_down` decides and primaries and backups count alike (see below).
A stale store also uses every backend.

DNS answers follow the same order; SRV records list only the chosen set.
Backups need clients that read store format version 2 ([chapter
6](06-clients.md#store-format-version)).

## Slow start

A backend that comes back may have cold caches or connection pools. With
`slow_start` set (for example `slow_start = "30s"`), a backend that turns
from `unhealthy` to `healthy` gets a growing share of traffic over that
time instead of its full share at once.

- The time is split into 10 equal steps. In the first tenth the backend
  counts as 1/10 of its weight, in the second as 2/10, and so on; after
  `slow_start` it counts in full. Its weight is its `weight` for
  `weighted` and 1 for every other method.
- While at least one candidate is in slow start, `round-robin`,
  `weighted`, `locality`, `fastest` and `sticky` without a key choose by
  these reduced weights. `locality` and `fastest` first narrow to the
  site or the pool, as usual. `failover` and `sticky` with a key are not
  affected.
- Only a recovery counts: a backend that is `unknown` when the daemon
  starts, or that a reload adds or restarts, gets its full share at once.
  A backend that turns unhealthy again leaves slow start.
- It has an effect only when some backends recover and others are already
  healthy. A service with one backend still sends it everything, and
  backends that recover together keep equal shares.
- A stale store has no slow start, and a clock that went back ends it.
- Clients older than this feature ignore it: a recovered backend gets its
  full share at once.

DNS A/AAAA answers follow the same reduced weights, and SRV records carry
them as their weight ([chapter 7](07-dns.md#srv-records)).

## When no backend is healthy

| `on_all_down` | Lookup | DNS A/AAAA/SRV |
| --- | --- | --- |
| `open` (default) | Chooses among the open candidates below | Answers with the open candidates below |
| `closed` | Fails with `ALL_DOWN` | `SERVFAIL` |

With `open`, the candidates are, in this order, the first set that is not
empty:

1. the `unknown` backends whose last check passed (no `reason`);
2. every `unknown` backend;
3. every backend.

An `unhealthy` backend failed `fall` checks in a row, while an `unknown`
one is not decided yet (just started, added or reset, or its checks only
gave errors for `fall × interval`), so it is the better bet. When the
daemon starts, every backend is `unknown` and all of them are candidates.
A stale store uses every backend. Clients older than this rule choose
among all backends.

The lookup does not tell the application that it came from an all-down
service. Use `open` when trying a backend is better than failing (the
checks themselves may be at fault), and `closed` when sending traffic to a
broken backend is worse than failing fast.

## A stale store

When the daemon's heartbeat is older than `EMFOR_MAX_STALENESS` (3 s by
default), or the daemon stopped cleanly, the client treats no backend as
healthy and `on_all_down` decides, exactly as above. A `fastest` service
ignores its pool. Lookups never block or wait for the daemon.

## DNS differences

The DNS server answers from the daemon's memory with the same health
states, but:

- An A query considers only IPv4 backends and an AAAA query only IPv6
  backends. A service with no backend of that family gets an empty answer.
  Client lookups consider both families together.
- Round-robin and weighted choices are made by the daemon, once per query,
  not per application process.
- `sticky` has no affinity key over DNS and works like `round-robin`.
- SRV answers list every candidate, with priority and weight set from
  `balance` ([chapter 7](07-dns.md#srv-records)).
- Resolver caching can return the same answer for up to the TTL.
