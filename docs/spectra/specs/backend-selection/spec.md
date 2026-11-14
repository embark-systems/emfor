# backend-selection Specification

## Purpose

Defines the `balance` methods and the rules for choosing one backend of a service, so that the DNS server, and later every language client, pick backends the same way. This capability covers selection without an affinity key.

## Requirements

### Requirement: Balance methods

Each service SHALL have a `balance` of `round-robin`, `sticky`, `failover`, `weighted`, `locality`, or `fastest`, taken from the service table, otherwise from `[defaults]`, otherwise `round-robin`.

#### Scenario: Default balance

- **WHEN** neither `[defaults]` nor the service sets `balance`
- **THEN** the service's `balance` is `round-robin`

#### Scenario: Service overrides default

- **WHEN** `[defaults]` sets `balance = "failover"` and a service sets `balance = "round-robin"`
- **THEN** that service's `balance` is `round-robin` and services without their own `balance` use `failover`

#### Scenario: New methods accepted

- **WHEN** one service sets `balance = "weighted"`, another sets `balance = "locality"`, and a third sets `balance = "fastest"`
- **THEN** the config is valid and the store records show `"weighted"`, `"locality"` and `"fastest"` respectively

#### Scenario: Unknown value rejected

- **WHEN** a service sets `balance = "random"`
- **THEN** the config is invalid and the error for `balance` lists `round-robin`, `sticky`, `failover`, `weighted`, `locality` and `fastest`


<!-- @trace
source: emfor-balance-fastest
updated: 2026-11-12
code:
  - crates/emfor-format/src/lib.rs
  - crates/emfor-daemon/src/select.rs
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/dns.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/config.rs
  - examples/emfor.toml
  - docs/deploy/dns-resolver.md
  - crates/emfor-daemon/src/checker/mod.rs
  - clients/rust/src/client.rs
  - .spectra.yaml
  - tests/conformance/v1/cases.json
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/schedule.rs
  - clients/rust/README.md
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Candidate backends per address family

To choose a backend for an address family (IPv4 or IPv6), the system SHALL use only the service's backends of that family, in config order. If there are none, the result SHALL be "no backend of this family". Otherwise the candidates SHALL be the passing backends of that family, where a backend is passing when its `state` is `healthy` and its `reason` is `null`; if none of them is passing, the candidates SHALL be the `healthy` backends of that family, including those whose `reason` is not `null` (failing backends); if none of them is `healthy` and `on_all_down` is `open`, the candidates SHALL be the backends of that family whose `state` is `unknown` and `reason` is `null` (passing unknown backends), or, when there are none, every `unknown` backend of that family, or, when there are none, all backends of that family; if none of them is `healthy` and `on_all_down` is `closed`, the result SHALL be "all down".

#### Scenario: Unhealthy backends are skipped

- **WHEN** a service has IPv4 backends 10.0.0.1 (healthy) and 10.0.0.2 (unhealthy) and an IPv4 backend is chosen
- **THEN** the only candidate is 10.0.0.1

#### Scenario: Failing backends are skipped while another passes

- **WHEN** a service has IPv4 backends 10.0.0.1 (healthy, reason `connect: Connection refused`) and 10.0.0.2 (healthy, reason `null`) and an IPv4 backend is chosen
- **THEN** the only candidate is 10.0.0.2

#### Scenario: Every healthy backend failing

- **WHEN** a service with `on_all_down = "closed"` has IPv4 backends 10.0.0.1 (healthy, reason `timeout`), 10.0.0.2 (healthy, reason `timeout`) and 10.0.0.3 (unhealthy) and an IPv4 backend is chosen
- **THEN** the candidates are 10.0.0.1 and 10.0.0.2

#### Scenario: Open service prefers unknown backends

- **WHEN** a service with `on_all_down = "open"` has IPv4 backends 10.0.0.1 (unhealthy), 10.0.0.2 (unknown, reason `timeout`) and 10.0.0.3 (unknown, reason `null`) and an IPv4 backend is chosen
- **THEN** the only candidate is 10.0.0.3

##### Example: candidate sets

| IPv4 backends (state) | IPv6 backends (state) | `on_all_down` | Family asked | Result |
| --- | --- | --- | --- | --- |
| 10.0.0.1 (healthy), 10.0.0.2 (unhealthy) | none | open | IPv4 | candidates: 10.0.0.1 |
| 10.0.0.1 (unhealthy), 10.0.0.2 (unknown) | none | open | IPv4 | candidates: 10.0.0.2 |
| 10.0.0.1 (unhealthy), 10.0.0.2 (unknown, reason "timeout"), 10.0.0.3 (unknown, reason "refused") | none | open | IPv4 | candidates: 10.0.0.2, 10.0.0.3 |
| 10.0.0.1 (unhealthy), 10.0.0.2 (unhealthy) | 2001:db8::1 (unknown) | open | IPv4 | candidates: 10.0.0.1, 10.0.0.2 |
| 10.0.0.1 (unhealthy), 10.0.0.2 (unknown) | none | closed | IPv4 | all down |
| 10.0.0.1 (healthy) | none | open | IPv6 | no backend of this family |
| 10.0.0.1 (unhealthy) | 2001:db8::1 (healthy) | closed | IPv4 | all down |
| 10.0.0.1 (unhealthy) | 2001:db8::1 (healthy) | closed | IPv6 | candidates: 2001:db8::1 |
| 10.0.0.1 (healthy, reason "refused"), 10.0.0.2 (healthy, reason null) | none | closed | IPv4 | candidates: 10.0.0.2 |
| 10.0.0.1 (healthy, reason "refused"), 10.0.0.2 (unhealthy) | 2001:db8::1 (healthy, reason null) | closed | IPv4 | candidates: 10.0.0.1 |


<!-- @trace
source: emfor-open-prefers-unknown
updated: 2026-11-14
code:
  - tests/conformance/v1/cases.json
  - docs/manual.zh_TW/05-backend-selection.md
  - docs/manual/07-dns.md
  - crates/emfor-format/src/rules.rs
  - docs/manual.zh_TW/01-concepts.md
  - crates/emfor-daemon/src/select.rs
  - clients/rust/src/select.rs
  - CHANGELOG.md
  - docs/manual/05-backend-selection.md
  - docs/deploy/dns-resolver.md
  - docs/manual.zh_TW/07-dns.md
  - docs/manual/01-concepts.md
-->

---
### Requirement: Choosing among candidates without an affinity key

Given a non-empty candidate list, the system SHALL choose exactly one backend: with `round-robin`, the candidates in turn, advancing one position per choice for that service and family; with `failover`, always the first candidate in config order; with `sticky`, the same as `round-robin`, because no affinity key is available.

#### Scenario: Round-robin alternates

- **WHEN** a `round-robin` service has healthy IPv4 backends 10.0.0.1 and 10.0.0.2 and four IPv4 choices are made in a row
- **THEN** the choices alternate between the two backends, each chosen twice

#### Scenario: Failover returns the first healthy backend

- **WHEN** a `failover` service lists 10.0.0.1, 10.0.0.2, 10.0.0.3 in that order and 10.0.0.1 is unhealthy while the others are healthy
- **THEN** every choice is 10.0.0.2

##### Example: failover order

| States of 10.0.0.1, 10.0.0.2, 10.0.0.3 | `on_all_down` | Every choice |
| --- | --- | --- |
| healthy, healthy, healthy | open | 10.0.0.1 |
| unhealthy, healthy, healthy | open | 10.0.0.2 |
| unhealthy, unhealthy, healthy | open | 10.0.0.3 |
| unhealthy, unhealthy, unhealthy | open | 10.0.0.1 |
| unhealthy, unhealthy, unhealthy | closed | none (all down) |

#### Scenario: Sticky without a key

- **WHEN** a `sticky` service has healthy IPv4 backends 10.0.0.1 and 10.0.0.2 and two IPv4 choices are made without an affinity key
- **THEN** the two choices are different backends

<!-- @trace
source: emfor-dns
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: Weighted choice in the daemon

For a `weighted` service, the daemon SHALL choose among the candidates of the requested family by smooth weighted round-robin, keeping one state per service and family: on each choice, add each candidate's `weight` to its current value, choose the candidate with the largest current value (the earlier one in config order on a tie), and subtract the sum of the candidates' weights W from the chosen candidate's current value. The state SHALL start at zero and SHALL be reset to zero whenever the candidate list differs from the one used for the previous choice. Over any W consecutive choices among an unchanged candidate list, each candidate SHALL be chosen exactly `weight` times.

#### Scenario: Proportions over one cycle

- **WHEN** a `weighted` service has healthy IPv4 backends 10.0.0.1 (weight 1), 10.0.0.2 (weight 3) and 10.0.0.3 (weight 1), and five A queries are made in a row
- **THEN** 10.0.0.2 is answered three times and 10.0.0.1 and 10.0.0.3 once each

##### Example: answer order

| Weights (in config order) | First answers from a reset state |
| --- | --- |
| 10.0.0.1 = 3, 10.0.0.2 = 1 | 10.0.0.1, 10.0.0.1, 10.0.0.2, 10.0.0.1 |
| 10.0.0.1 = 1, 10.0.0.2 = 3, 10.0.0.3 = 1 | 10.0.0.2, 10.0.0.1, 10.0.0.2, 10.0.0.3, 10.0.0.2 |
| 10.0.0.1 = 1, 10.0.0.2 = 1 | 10.0.0.1, 10.0.0.2, 10.0.0.1, 10.0.0.2 |

#### Scenario: Unhealthy heavy backend is skipped

- **WHEN** a `weighted` service has 10.0.0.1 (weight 3, unhealthy) and 10.0.0.2 (weight 1, healthy)
- **THEN** every A answer is 10.0.0.2


<!-- @trace
source: emfor-balance-2
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: Locality choice in the daemon

For a `locality` service, the daemon SHALL narrow the candidates of the requested family to those whose `site` equals the config's top-level `site`, when the config sets `site` and at least one candidate matches; otherwise it SHALL keep all candidates. It SHALL then choose among them by round-robin with the service's per-family counter.

#### Scenario: Local backends preferred

- **WHEN** the daemon has `site = "dc1"` and a `locality` service has healthy 10.0.0.1 (site dc1), 10.0.0.2 (site dc1) and 10.0.0.3 (site dc2), and four A queries are made
- **THEN** the answers alternate between 10.0.0.1 and 10.0.0.2 and never include 10.0.0.3

##### Example: locality outcomes

| Daemon `site` | Backends (site, state) | `on_all_down` | A answers come from |
| --- | --- | --- | --- |
| dc1 | 10.0.0.1 (dc1, healthy), 10.0.0.3 (dc2, healthy) | open | 10.0.0.1 |
| dc1 | 10.0.0.1 (dc1, unhealthy), 10.0.0.3 (dc2, healthy) | open | 10.0.0.3 |
| dc1 | 10.0.0.1 (dc1, unhealthy), 10.0.0.3 (dc2, unhealthy) | open | 10.0.0.1 |
| dc1 | 10.0.0.1 (dc1, unhealthy), 10.0.0.3 (dc2, unhealthy) | closed | none (SERVFAIL) |
| none | 10.0.0.1 (dc1, healthy), 10.0.0.3 (dc2, healthy) | open | 10.0.0.1 and 10.0.0.3 in turn |
| dc3 | 10.0.0.1 (dc1, healthy), 10.0.0.3 (dc2, healthy) | open | 10.0.0.1 and 10.0.0.3 in turn |

<!-- @trace
source: emfor-balance-2
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: Fast membership decided by the daemon

For every `fastest` service, the daemon SHALL keep a fast flag for each backend and SHALL recompute it for all backends of the service whenever a backend's state or latency changes and after a reload. Only backends that are `healthy` and have a latency, of either address family, SHALL be in scope; every other backend SHALL NOT be fast. With `min` the lowest latency in scope in microseconds, a backend in scope that is not fast SHALL become fast when its latency is at most the larger of `min * 3 / 2` (integer division) and `min + 1000`, and a fast backend in scope SHALL stay fast while its latency is at most the larger of `min * 2` and `min + 2000`, and SHALL become not fast otherwise. A backend that is added, reset to `unknown`, or becomes `unhealthy` SHALL NOT be fast. The service's pool SHALL be the ids of its fast backends in config order; the daemon SHALL write it to the store and SHALL use it for DNS answers. Readers SHALL use the pool as written and SHALL NOT recompute it.

#### Scenario: Nearly equal backends are fast

- **WHEN** a service has healthy backends with latencies 2000, 2800 and 5000 microseconds, none of them fast before
- **THEN** after the recomputation the first two are fast and the third is not

#### Scenario: A fast backend near the edge stays fast

- **WHEN** a fast backend's latency rises from 2900 to 3500 microseconds while the lowest latency in the service stays 2000
- **THEN** the backend stays fast, and it becomes not fast only when its latency exceeds 4000

#### Scenario: Missing latency is not fast

- **WHEN** a service has healthy 10.0.0.1 with latency 900 and healthy 10.0.0.2 with no latency
- **THEN** 10.0.0.1 is fast and 10.0.0.2 is not

##### Example: fast membership

| Lowest latency in scope (µs) | Backend (state, latency µs, fast before) | Fast after (in the pool) |
| --- | --- | --- |
| 2000 | healthy, 2000, false | true |
| 2000 | healthy, 3000, false | true |
| 2000 | healthy, 3500, false | false |
| 2000 | healthy, 3500, true | true |
| 2000 | healthy, 4000, true | true |
| 2000 | healthy, 4001, true | false |
| 2000 | unknown, 100, false | false |
| 2000 | healthy, none, true | false |
| 200 | healthy, 1200, false | true |
| 200 | healthy, 1201, false | false |
| 200 | healthy, 2200, true | true |
| 200 | healthy, 2201, true | false |


<!-- @trace
source: emfor-balance-fastest
updated: 2026-11-12
code:
  - crates/emfor-format/src/lib.rs
  - crates/emfor-daemon/src/select.rs
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/dns.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/config.rs
  - examples/emfor.toml
  - docs/deploy/dns-resolver.md
  - crates/emfor-daemon/src/checker/mod.rs
  - clients/rust/src/client.rs
  - .spectra.yaml
  - tests/conformance/v1/cases.json
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/schedule.rs
  - clients/rust/README.md
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Fastest choice in the daemon

For a `fastest` service, the daemon SHALL take the backends of the service's pool that are of the requested family. When there are any, it SHALL choose among them by round-robin with the service's per-family counter; otherwise it SHALL choose as for `round-robin`, with the same counter, including the rules for no backend of the family and all down.

#### Scenario: Fast backends preferred

- **WHEN** a `fastest` service has healthy 10.0.0.1, 10.0.0.2 and 10.0.0.3 with pool [10.0.0.1, 10.0.0.2], and four A queries are made
- **THEN** the answers alternate between 10.0.0.1 and 10.0.0.2 and never include 10.0.0.3

#### Scenario: Empty pool

- **WHEN** a `fastest` service has healthy 10.0.0.1 and 10.0.0.2 with an empty pool (no latency yet), and two A queries are made
- **THEN** the two answers are different backends

#### Scenario: All down open

- **WHEN** every backend of a `fastest` service is unhealthy and `on_all_down = "open"`
- **THEN** the pool is empty and the A answers rotate over every backend

#### Scenario: Pool without the family

- **WHEN** a `fastest` service has healthy 10.0.0.1 and 2001:db8::1 and the pool is [2001:db8::1], and two A queries are made
- **THEN** both answers are 10.0.0.1

<!-- @trace
source: emfor-balance-fastest
updated: 2026-11-12
code:
  - crates/emfor-format/src/lib.rs
  - crates/emfor-daemon/src/select.rs
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/dns.rs
  - clients/c/README.md
  - crates/emfor-daemon/src/config.rs
  - examples/emfor.toml
  - docs/deploy/dns-resolver.md
  - crates/emfor-daemon/src/checker/mod.rs
  - clients/rust/src/client.rs
  - .spectra.yaml
  - tests/conformance/v1/cases.json
  - crates/emfor-daemon/src/state.rs
  - crates/emfor-daemon/src/checker/http.rs
  - crates/emfor-daemon/src/schedule.rs
  - clients/rust/README.md
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/writer.rs
tests:
  - crates/emfor-daemon/tests/dns.rs
  - tools/emfor-fixture/tests/conformance_data.rs
  - crates/emfor-daemon/tests/daemon.rs
-->

---
### Requirement: Slow start effective weight

A backend's slow start step SHALL be 10 when the store or the daemon's state gives it `recovered_at` `null`, when its service's `slow_start_ms` is 0, when the current Unix time in milliseconds is earlier than `recovered_at`, or when the time elapsed since `recovered_at` is at least `slow_start_ms`. Otherwise the step SHALL be `max(1, ceil(10 × elapsed / slow_start_ms))`. A backend is ramping when its step is below 10. Its effective weight SHALL be its base weight times its step, where the base weight is the backend's `weight` for `balance = "weighted"` and 1 for every other `balance`.

#### Scenario: Step during a 30 second slow start

- **WHEN** a backend's `recovered_at` is 1762905600000 and its service's `slow_start_ms` is 30000
- **THEN** its step is 1 at 1762905600000, 1 at 1762905603000, 4 at 1762905610000, 10 at 1762905630000 and 10 at 1762905640000

##### Example: step and effective weight

| `recovered_at` | `slow_start_ms` | Now | `balance` | `weight` | Step | Effective weight |
| --- | --- | --- | --- | --- | --- | --- |
| 1762905600000 | 30000 | 1762905600000 | round-robin | 1 | 1 | 1 |
| 1762905600000 | 30000 | 1762905601000 | round-robin | 1 | 1 | 1 |
| 1762905600000 | 30000 | 1762905607500 | round-robin | 1 | 3 | 3 |
| 1762905600000 | 30000 | 1762905629900 | weighted | 4 | 10 | 40 |
| 1762905600000 | 30000 | 1762905630000 | weighted | 4 | 10 | 40 |
| 1762905600000 | 30000 | 1762905607500 | weighted | 4 | 3 | 12 |
| null | 30000 | 1762905607500 | weighted | 4 | 10 | 40 |
| 1762905600000 | 0 | 1762905607500 | round-robin | 1 | 10 | 10 |
| 1762905600000 | 30000 | 1762905500000 | round-robin | 1 | 10 | 10 |


<!-- @trace
source: emfor-slow-start
updated: 2026-11-14
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/state.rs
  - tools/emfor-fixture/src/lib.rs
  - docs/manual.zh_TW/07-dns.md
  - tests/conformance/v1/README.md
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-format/src/rules.rs
  - tests/conformance/v1/cases.json
  - crates/emfor-format/src/lib.rs
  - docs/manual/07-dns.md
  - crates/emfor-daemon/src/select.rs
  - crates/emfor-daemon/src/writer.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/supervisor.rs
  - docs/manual.zh_TW/05-backend-selection.md
  - crates/emfor-daemon/src/metrics.rs
  - CHANGELOG.md
  - docs/manual/05-backend-selection.md
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/dns.rs
  - docs/manual.zh_TW/03-configuration.md
  - clients/rust/src/client.rs
-->

---
### Requirement: Slow start choice in the daemon

When choosing an A or AAAA answer, after the candidates are found and, for `locality` and `fastest`, narrowed to the preferred ones, the daemon SHALL check whether any remaining candidate is ramping. If none is, the choice SHALL be made exactly as without slow start. If at least one is and `balance` is `round-robin`, `weighted`, `locality`, `fastest`, `sticky` or a value the daemon does not recognize, the daemon SHALL choose by smooth weighted round-robin over the candidates' effective weights, keeping its state per service and family and resetting it whenever the list of candidates and their effective weights differs from the previous choice. `failover` SHALL always return the first candidate.

#### Scenario: Ramping backend gets a smaller share

- **WHEN** a `round-robin` service has healthy backends 10.0.0.1 (not ramping) and 10.0.0.2 (step 3), and 13 IPv4 answers are chosen while the step stays 3
- **THEN** 10.0.0.1 is chosen 10 times and 10.0.0.2 3 times

#### Scenario: Failover ignores slow start

- **WHEN** a `failover` service has healthy backends 10.0.0.1 (step 1) and 10.0.0.2 (not ramping) in config order
- **THEN** every IPv4 answer is 10.0.0.1

#### Scenario: Weighted ramping backend

- **WHEN** a `weighted` service has healthy backends 10.0.0.1 (weight 3, step 2) and 10.0.0.2 (weight 1, not ramping), and 16 IPv4 answers are chosen while the step stays 2
- **THEN** 10.0.0.1 is chosen 6 times and 10.0.0.2 10 times

<!-- @trace
source: emfor-slow-start
updated: 2026-11-14
code:
  - examples/emfor.toml
  - crates/emfor-daemon/src/state.rs
  - tools/emfor-fixture/src/lib.rs
  - docs/manual.zh_TW/07-dns.md
  - tests/conformance/v1/README.md
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-format/src/rules.rs
  - tests/conformance/v1/cases.json
  - crates/emfor-format/src/lib.rs
  - docs/manual/07-dns.md
  - crates/emfor-daemon/src/select.rs
  - crates/emfor-daemon/src/writer.rs
  - docs/manual/03-configuration.md
  - crates/emfor-daemon/src/supervisor.rs
  - docs/manual.zh_TW/05-backend-selection.md
  - crates/emfor-daemon/src/metrics.rs
  - CHANGELOG.md
  - docs/manual/05-backend-selection.md
  - clients/rust/src/select.rs
  - crates/emfor-daemon/src/live.rs
  - crates/emfor-daemon/src/dns.rs
  - docs/manual.zh_TW/03-configuration.md
  - clients/rust/src/client.rs
-->