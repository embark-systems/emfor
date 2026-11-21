# 3. Concepts

[Contents](README.md) · Previous: [2. Quick start](02-quick-start.md) · Next: [4. Installing](04-installing.md)

This chapter explains the ideas the rest of the manual uses. Each section
ends with a pointer to the chapter that has the details.

## The parts

| Part | What it is |
| --- | --- |
| `emfor server` | The daemon. It reads the config, checks every backend, and writes the results to the state store. One per host. |
| The state store | An LMDB file, `/var/lib/emfor/state.mdb` by default. The daemon is its only writer; any number of processes on the host read it. |
| Client libraries | `libemfor` (C, built from the Rust crate `emfor-client`) and wrappers for Go, Python, Perl and PHP. A lookup reads the store and picks one backend. |
| DNS server | Optional. Answers A, AAAA and SRV queries for the services, from the daemon's memory. |
| `emfor-check` | A plugin program with checks for databases and network protocols. |
| `emfor status`, `emfor lookup`, `emfor check-config` | Commands to see what the daemon publishes, what a lookup would return, and whether a config is valid. |
| `/metrics` | Optional Prometheus endpoint. |

There is no central server. Each host has its own daemon, its own config and
its own view of the backends. Hosts of one data centre can optionally share
check results in a [peer group](#peer-groups).

## Services and backends

A **service** is a name, such as `db.svc.emfor`, with:

- a list of **backends**: the `ip:port` addresses that serve it;
- a **check**: how one backend is tested (`tcp`, `http`, `exec`, or a plugin
  such as `pgsql`), with its `check_options`;
- timing: `interval`, `timeout`, `rise`, `fall`, `jitter`;
- a **balance** method: how one healthy backend is chosen;
- an **on_all_down** policy: what a lookup returns when none is healthy.

```toml
[services."db.svc.emfor"]
check = "tcp"
balance = "failover"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
```

Backends are always IP addresses, never host names, so that emfor does not
depend on DNS to find them. The port belongs to each backend; backends of one
service may use different ports.

Service names look like host names: labels of `a-z`, `0-9` and `-`, separated
by dots, at most 253 characters. Case and one trailing dot are ignored, both
in the config and in a lookup.

## Check results

Every check of a backend ends in one of three **results**:

| Result | Meaning | Counts against the backend |
| --- | --- | --- |
| `up` | The backend passed. | No |
| `down` | The backend failed: refused, timed out, wrong answer. | Yes |
| `error` | emfor could not do the check, because of a problem on this host (an unreadable password file, a plugin that crashed, too many open files). | No |

A `down` or `error` comes with a **reason**, such as `timeout` or
`connect: Connection refused (os error 111)`. You see it in `emfor status`, in
the log and in the state history.

The difference between `down` and `error` matters: a local problem must not
mark every good backend down. An `error` changes no counter. Only when a
backend has had nothing but errors for longer than `fall × interval` does it
go back to `unknown`.

## Health states, rise and fall

Each backend has a **health state**:

| State | When |
| --- | --- |
| `unknown` | After the daemon starts, after the backend is added, after its check definition changes, and after a long run of `error` results. |
| `healthy` | After `rise` consecutive `up` results (default 2). |
| `unhealthy` | After `fall` consecutive `down` results (default 3). |

```
               rise × up                     fall × down
   unknown  ─────────────>  healthy  ─────────────────────>  unhealthy
      │                        ^                                 │
      │      fall × down       │            rise × up            │
      └───────────> unhealthy  └─────────────────────────────────┘
```

An `up` resets the failure count and a `down` resets the success count, so
the counts are of results in a row. The counters make one lost packet harmless. With the defaults
(`interval = "2s"`, `fall = 3`, `rise = 2`) a backend that stops is marked
unhealthy after about 6 seconds, and one that comes back is healthy again
after about 4 seconds.

While a backend is `unknown`, emfor checks it every 200 ms (or every
`interval`, if that is shorter), for at most `rise + fall` checks. So after a
start every backend is known within about half a second. Details:
[chapter 6](06-health-checks.md#scheduling).

## The state store and the heartbeat

The daemon writes one record per service into the store, and a **heartbeat**
(the current time) at least once a second. A client compares the heartbeat
with its own clock:

- heartbeat at most 3 seconds old (`EMFOR_MAX_STALENESS`): the store is
  **fresh**, and the health states count;
- older, or 0 (the daemon stopped cleanly): the store is **stale**, and the
  client treats every backend as not healthy. `on_all_down` then decides.

So an application never trusts health data from a daemon that hung or went
away, and an `open` service still gives it an address.

When the daemon starts, it rewrites every service with every backend
`unknown`. It never reuses the health states of an earlier run; only the
**state history** (the last 20 changes of each backend) survives a restart.
Details: [chapter 12](12-operating.md#the-store-on-disk).

## What a lookup returns: tiers

A lookup first picks a set of **candidates**, then lets `balance` choose one
of them. The candidates are the first of these **tiers** that is not empty:

1. healthy primary backends whose last check passed;
2. healthy backup backends whose last check passed;
3. healthy primary backends, including those whose last check failed;
4. healthy backup backends, including those whose last check failed.

"Last check failed" means the backend is still `healthy` (it has not had
`fall` failures yet), but its most recent check did not pass. Because of tier
1, a backend that starts failing is left out at its **first** failed check, as
long as another backend passes.

When none of the four tiers has a backend, the service is **all down**, and
`on_all_down` decides:

| `on_all_down` | Lookup | DNS |
| --- | --- | --- |
| `open` (default) | Chooses from: the `unknown` backends whose last check passed, else every `unknown` backend, else every backend | Answers the same way |
| `closed` | Fails with `ALL_DOWN` | `SERVFAIL` |

Use `open` when trying a backend is better than failing (the checks
themselves may be wrong). Use `closed` when sending traffic to a broken
backend is worse than failing fast.

`emfor lookup <service> --explain` prints the tier and why each other
backend is out. Details: [chapter 8](08-choosing-a-backend.md).

## Balance methods

| `balance` | Chooses |
| --- | --- |
| `round-robin` (default) | The candidates in turn |
| `sticky` | The same backend for the same affinity key |
| `failover` | The first candidate in config order |
| `weighted` | In proportion to each backend's `weight` |
| `locality` | Candidates in this host's `site` first |
| `fastest` | In turn among the backends whose checks answer fastest |

Details: [chapter 8](08-choosing-a-backend.md#balance-methods).

## Backup backends

A backend with `backup = true` is used only while no primary backend passes
its checks (tiers 2 and 4 above). Use it for a replica in another data centre
or a smaller standby machine.

## Sites

`site = "dc1"` at the top of the config names the data centre of this host;
`site = "dc2"` on a backend names the backend's. `balance = "locality"`
prefers backends of the host's own site, and a [peer group](#peer-groups)
only spans one site.

## Slow start

With `slow_start = "30s"`, a backend that goes from `unhealthy` to `healthy`
gets a growing share of the traffic over 30 seconds, in ten steps, instead of
its full share at once, so that cold caches and connection pools can warm up.
Details: [chapter 8](08-choosing-a-backend.md#slow-start).

## Drain

`drain = true` on a backend takes it out of every choice (lookups and DNS)
while its checks go on, for an upgrade or a reboot. emfor is not in the data
path, so connections that are already open are not touched; new lookups just
skip the backend. There is no run-time command: you edit the config and
reload. Details: [chapter 8](08-choosing-a-backend.md#drain).

## Peer groups

Without help, each host needs `fall` failed checks of its own to notice a
failure. In a **peer group**, the daemons of one `site` that list each other
in `[peers]` exchange authenticated, encrypted UDP messages:

- when one member sees a backend go from `healthy` to `unhealthy`, it tells
  the others, which recheck that backend at once;
- with three or more live members, three of them are **assigned checkers**
  for each backend; when two of the three see it down, it counts as down on
  every member;
- members that are not assigned checkers of a backend check it less often.

Lookups still read only the local store. A member that cannot reach the
others decides from its own checks, as without a group. Details:
[chapter 11](11-peer-groups.md).

## Files the daemon uses

| File | Purpose |
| --- | --- |
| `/etc/emfor/emfor.toml` | The config (another path with `--config`). |
| `/etc/emfor/conf.d/*.toml` | More config files, when `include_dir = "conf.d"` is set. |
| `<store>` (`/var/lib/emfor/state.mdb`) | The state store. Mode 0640 by default. |
| `<store>-lock` | LMDB's lock file. Every reader registers in it, so applications need write access. Mode 0660 by default. |
| `<store>.writer` | Locked by the running daemon, so only one daemon writes a store. Mode 0600. |
| `<store>.corrupt` | A damaged store, moved aside at start. |
| `<store>-peers` | With `[peers]`: fingerprints (never the secrets) of the peer secrets the daemon used. Mode 0600. |
