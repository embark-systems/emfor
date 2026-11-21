# 19. Glossary

[Contents](README.md) · Previous: [18. Configuration reference](18-config-reference.md)

| Term | Meaning |
| --- | --- |
| **affinity key** | A string an application passes to `lookup` so that `sticky` gives the same backend for it. |
| **all down** | No backend of a service is in tiers 1–4 (none is healthy). `on_all_down` decides what a lookup returns. |
| **assigned checkers** | In a peer group, the three members whose votes decide a backend's state. Chosen by hash, the same on every member. |
| **backend** | One `ip:port` that serves a service. |
| **backup** | A backend used only while no primary backend passes its checks. |
| **balance** | How one candidate is chosen: `round-robin`, `sticky`, `failover`, `weighted`, `locality`, `fastest`. |
| **candidates** | The backends a lookup may choose from: the first non-empty tier. |
| **check** | One test of one backend, by `tcp`, `http`, `exec` or a plugin. |
| **check definition digest** | A hash of a service's `check`, `check_options`, `timeout` and plugin command, compared between peers. |
| **client library** | `libemfor` and its wrappers; reads the store and returns one backend. |
| **disagreement** | A backend that is down by the peers' votes but up in this host's own check. |
| **drain** | `drain = true`: the backend is never chosen, but still checked. |
| **`emfor-check`** | The plugin program that ships with emfor, with checks for databases and network protocols. |
| **`EMP1`, `EMP2`, `EMP3`** | The peer message formats: signed; encrypted with ChaCha20-Poly1305; encrypted with XChaCha20-Poly1305. |
| **`error` (result)** | A check that could not be made because of a problem on this host. It does not count against the backend. |
| **fall** | How many `down` results in a row make a backend `unhealthy`. |
| **fast pool** | For `fastest`: the backends whose check latency is close to the lowest. |
| **fast recheck run** | Checks every 200 ms (at most `fall`) started when a peer reports a backend down. |
| **flapping** | Five or more state changes of a backend within ten minutes. |
| **heartbeat** | The time the daemon writes to the store every second; clients use it to tell whether the store is fresh. |
| **history** | The last 20 state changes of each backend, kept in the store across restarts. |
| **interval** | Time between two checks of one backend. |
| **jitter** | A random part added to each `interval`, so checks do not all run at once. |
| **live (member)** | A peer from which a valid message arrived in the last 3 seconds. |
| **lock file** | `<store>-lock`, LMDB's reader table; every reader needs write access. |
| **lookup** | Asking which backend to use for a service, from a client library, `emfor lookup` or DNS. |
| **on_all_down** | `open` (answer anyway) or `closed` (fail with `ALL_DOWN`) when a service is all down. |
| **peer group** | Daemons of one site that share failure notices and vote on backends. |
| **plugin** | A long-running program the daemon sends check requests to, speaking protocol 1. |
| **reason** | The text that says why a check failed. |
| **reload** | Re-reading the config on `SIGHUP`, without a restart. |
| **rise** | How many `up` results in a row make a backend `healthy`. |
| **service** | A name with a list of backends, a check and a balance method. |
| **site** | A data centre label, of the host and of backends. |
| **slow start** | A recovered backend's share growing over `slow_start` in ten steps. |
| **SRV record** | A DNS record with a port, priority and weight; emfor answers one per candidate. |
| **stale (store)** | A store whose heartbeat is older than `EMFOR_MAX_STALENESS` (3 s), or 0: every backend is treated as not healthy. |
| **state** | `unknown`, `healthy` or `unhealthy`. |
| **store** | The LMDB file the daemon writes and clients read, `/var/lib/emfor/state.mdb` by default. |
| **tier** | One of the ordered sets of backends a lookup tries, from "healthy primaries whose last check passed" to "every backend". |
| **zone** | The DNS domain emfor answers, `svc.emfor` by default. |
