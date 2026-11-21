# 8. How a backend is chosen

[Contents](README.md) · Previous: [7. Plugins](07-plugins.md) · Next: [9. Client libraries](09-clients.md)

A lookup, whether by a client library, `emfor lookup` or DNS, goes in three
steps:

1. Read the service record and the heartbeat from the store (DNS uses the
   daemon's memory).
2. Find the **candidates**: the first non-empty [tier](#tiers).
3. Let the service's **balance** method choose one candidate.

`emfor lookup <service> --explain` prints every step. This chapter describes
each of them.

## Tiers

| # | Tier (as `--explain` prints it) | Used when |
| --- | --- | --- |
| 1 | `healthy primary backends whose last check passed` | always first |
| 2 | `healthy backup backends whose last check passed` | tier 1 is empty |
| 3 | `healthy primary backends including those whose last check failed` | tiers 1–2 are empty |
| 4 | `healthy backup backends including those whose last check failed` | tiers 1–3 are empty |
| 5 | `unknown backends whose last check passed` | tiers 1–4 are empty and `on_all_down = "open"` |
| 6 | `every unknown backend` | as 5, and tier 5 is empty |
| 7 | `every backend` | as 5, and tier 6 is empty |
| – | `none` | tiers 1–4 are empty and `on_all_down = "closed"`: the lookup fails with `ALL_DOWN` |

A **drained** backend is in no tier, not even tier 7. A service whose
backends are all drained therefore always gives `ALL_DOWN`, even with
`on_all_down = "open"`.

When the store is **stale** (the heartbeat is older than
`EMFOR_MAX_STALENESS`, 3 s by default, or 0 because the daemon stopped), the
client treats every backend as not healthy, so tiers 1–4 are empty: an `open`
service answers from tier 7 (every backend that is not drained), a `closed`
one fails with
`ALL_DOWN: service db.svc.emfor: the store is stale (emfor daemon not updating it), and on_all_down is closed`.

`--explain` names, for every backend that is not a candidate, why:

| Shown as | Meaning |
| --- | --- |
| `unhealthy` | Its state is `unhealthy`. |
| `unknown` | Its state is `unknown`. |
| `last check failed` | `healthy` (or `unknown` in tier 5) but its latest check did not pass, while others passed. |
| `backup held back` | A backup, while a primary is a candidate. |
| `drained` | `drain = true`. |
| `stale` | The store is stale. |
| `not in the pool` | `fastest`: not among the fastest backends. |
| `not at this site` | `locality`: another site, while a backend at this host's site is a candidate. |

## on_all_down

| Value | When tiers 1–4 are empty |
| --- | --- |
| `open` (default) | Answer from tiers 5–7. DNS answers too. |
| `closed` | The lookup fails with `ALL_DOWN`; DNS answers `SERVFAIL`. |

Pick `open` for stateless services where trying a backend beats an error (and
where your checks could be wrong), `closed` where a wrong backend does harm,
such as a database primary that may have been demoted.

## Balance methods

The balance method is chosen per service (or in `[defaults]`):

```toml
[services."api.svc.emfor"]
check = "http"
balance = "weighted"
backends = [
  { addr = "10.0.0.11:8080", weight = 3 },
  { addr = "10.0.0.12:8080", weight = 1 },
]
```

### round-robin (default)

Each process keeps one counter per service, starting at a random value, and
takes the candidates in turn. Different processes are not coordinated, so the
spread is even only over many processes or many lookups.

### sticky

`sticky` gives the same backend for the same **affinity key**, a string your
application passes (a user id, a session id, a tenant):

```python
addr = emfor.lookup("cache.svc.emfor", affinity_key="user:42")
```

The choice is a weighted **rendezvous hash** over the candidates: each
candidate gets the score `MD5(key + id)` (the first 4 bytes, big-endian;
`id` is the backend's `ip:port`), with `weight` scores per backend, and the
highest score wins. So:

- every process on every host picks the same backend for a key, with no
  shared state;
- when a backend leaves the candidates, only the keys that were on it move;
  when it comes back, they move back;
- a backend with `weight = 2` gets about twice the keys.

Without a key (or with an empty one), and in DNS, `sticky` works as
`round-robin`. Slow start does not change the choice for a key.

### failover

The **first candidate in config order**. List the preferred backend first:

```toml
[services."db.svc.emfor"]
check = "tcp"
balance = "failover"
on_all_down = "closed"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
```

When the first backend fails a check, it drops out of tier 1 at once, and the
second is chosen. When the first passes again and is `healthy`, it is chosen
again, so traffic **fails back** automatically. If you do not want that, use a
check that only passes on the current primary (`role = "primary"` of the
`pgsql`, `mysql`, `redis` or `mongodb` check, [chapter 7](07-plugins.md#roles-and-replication-lag)).

### weighted

Each candidate is chosen in proportion to its `weight` (1–100, default 1).

- A client library takes `counter mod (sum of weights)` and walks the
  candidates in config order: with weights 3 and 1, one process gives
  `a a a b a a a b ...`.
- DNS uses smooth weighted round-robin, which spreads them out:
  `a a b a a a b a ...`.

Only candidates count: when a heavy backend is unhealthy, the others share
the traffic by their weights.

### locality

Candidates whose `site` equals the host's `site` (set at the top of the
config) are chosen in turn; when there are none, all candidates are, in turn.

```toml
site = "dc1"

[services."api.svc.emfor"]
check = "http"
balance = "locality"
backends = [
  { addr = "10.1.0.11:8080", site = "dc1" },
  { addr = "10.1.0.12:8080", site = "dc1" },
  { addr = "10.2.0.11:8080", site = "dc2" },
]
```

Here a lookup on a `dc1` host gives the two `dc1` backends while either is a
candidate, and the `dc2` one only when both are out. Tiers come first: a
`dc1` backend whose last check failed is not preferred over a passing `dc2`
one. A host without `site`, or backends without `site`, give plain
round-robin.

### fastest

The daemon keeps, per service, a **pool** of the backends that answer their
checks fastest, and clients take the pool's candidates in turn.

- Each backend's latency is a moving average of its check times (each new
  sample counts 5%), for `healthy` backends only.
- A backend enters the pool when its latency is at most 1.5 × the lowest one
  (at least the lowest + 1 ms), and leaves it above 2 × the lowest (at least
  the lowest + 2 ms). The gap stops backends from going in and out.
- Primaries and backups have separate pools.
- When the pool has no candidate, or the store is stale, `fastest` works as
  `round-robin`.

`emfor status` marks the backends in the pool. The latency is that of the
**check**, measured on this host: a fast `tcp` connect says nothing about how
fast the application answers.

## Backup backends

```toml
[services."api.svc.emfor"]
check = "http"
backends = [
  "10.0.0.11:8080",
  "10.0.0.12:8080",
  { addr = "10.9.0.11:8080", backup = true },
]
```

A backup is a candidate only in tiers 2 and 4: while no primary passes. At
least one backend must not be a backup (`at least one backend must not be a
backup`). Backups are checked like any other backend.

## Slow start

```toml
[services."api.svc.emfor"]
check = "http"
slow_start = "30s"
backends = ["10.0.0.11:8080", "10.0.0.12:8080"]
```

When a backend goes from `unhealthy` to `healthy`, it gets 1/10 of its share
at first, and one tenth more every `slow_start / 10` (here every 3 seconds),
until it has its full share after 30 seconds. A backend that becomes healthy
from `unknown` (after a start or reload) gets its full share at once.

- `slow_start` is `0s` (off, the default) or from `1s` to `1h`.
- While a candidate ramps, every method except `failover` (and `sticky` with
  a key) chooses by the effective weights: `weight × step` for `weighted`,
  `step` for the others.
- `--explain` lists the candidates in slow start with their step and
  effective weight.
- A stale store turns slow start off for that lookup.

## Drain

```toml
[services."api.svc.emfor"]
check = "http"
backends = [
  "10.0.0.11:8080",
  { addr = "10.0.0.12:8080", drain = true },
]
```

A drained backend is never chosen, by a client or by DNS, but its checks go
on, so `emfor status` still shows how it is doing:

```text
  127.0.0.2:8080  drained    2.2s ago  2.4ms  (checks: healthy)
```

Changing `drain` is a reload, and does not reset the backend's state. In the
store, a drained backend is written as `"state": "unhealthy"` with the reason
`drained`, so that a client of an older release also skips it. Applications
that already hold a connection to it keep it: emfor is not in the data path.
[Chapter 12](12-operating.md#a-rolling-deploy-with-drain) has a complete
rolling deploy.

## DNS and the client libraries compared

| | Client library, `emfor lookup` | DNS |
| --- | --- | --- |
| Reads | The store | The daemon's memory |
| Answer | One `ip:port` | A/AAAA: one address, no port. SRV: every candidate. |
| `sticky` | By affinity key | As round-robin |
| `weighted` | Counter over cumulative weights | Smooth weighted round-robin; SRV carries the weights |
| `failover` | The first candidate | The first; in SRV, priorities 0, 1, 2 in config order |
| `locality`, `fastest` | Preferred candidates in turn | The same; in SRV, preferred ones priority 0, others 1 |
| Daemon stopped | Stale store: `on_all_down` decides | No answer at all |
| Caching | None: every lookup reads the store | By the resolver, for `ttl` (default 1 s) |

[Chapter 10](10-dns.md) has the details of the DNS answers.
