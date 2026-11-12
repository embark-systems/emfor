# backend-selection Specification

## Purpose

Defines the `balance` methods and the rules for choosing one backend of a service, so that the DNS server, and later every language client, pick backends the same way. This capability covers selection without an affinity key.

## Requirements

### Requirement: Balance methods

Each service SHALL have a `balance` of `round-robin`, `sticky`, `failover`, `weighted`, or `locality`, taken from the service table, otherwise from `[defaults]`, otherwise `round-robin`.

#### Scenario: Default balance

- **WHEN** neither `[defaults]` nor the service sets `balance`
- **THEN** the service's `balance` is `round-robin`

#### Scenario: Service overrides default

- **WHEN** `[defaults]` sets `balance = "failover"` and a service sets `balance = "round-robin"`
- **THEN** that service's `balance` is `round-robin` and services without their own `balance` use `failover`

#### Scenario: New methods accepted

- **WHEN** one service sets `balance = "weighted"` and another sets `balance = "locality"`
- **THEN** the config is valid and the store records show `"weighted"` and `"locality"` respectively


<!-- @trace
source: emfor-balance-2
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
-->

---
### Requirement: Candidate backends per address family

To choose a backend for an address family (IPv4 or IPv6), the system SHALL consider only the service's backends of that family, in config order. If there are none, the result SHALL be "no backend of this family". Otherwise the candidates SHALL be the `healthy` backends of that family; if none of them is `healthy`, the candidates SHALL be all backends of that family when `on_all_down` is `open`, and the result SHALL be "all down" when `on_all_down` is `closed`.

#### Scenario: Unhealthy backends are skipped

- **WHEN** a service has IPv4 backends 10.0.0.1 (healthy) and 10.0.0.2 (unhealthy) and an IPv4 backend is chosen
- **THEN** the only candidate is 10.0.0.1

##### Example: candidate sets

| IPv4 backends (state) | IPv6 backends (state) | `on_all_down` | Family asked | Result |
| --- | --- | --- | --- | --- |
| 10.0.0.1 (healthy), 10.0.0.2 (unhealthy) | none | open | IPv4 | candidates: 10.0.0.1 |
| 10.0.0.1 (unhealthy), 10.0.0.2 (unknown) | none | open | IPv4 | candidates: 10.0.0.1, 10.0.0.2 |
| 10.0.0.1 (unhealthy), 10.0.0.2 (unknown) | none | closed | IPv4 | all down |
| 10.0.0.1 (healthy) | none | open | IPv6 | no backend of this family |
| 10.0.0.1 (unhealthy) | 2001:db8::1 (healthy) | closed | IPv4 | all down |
| 10.0.0.1 (unhealthy) | 2001:db8::1 (healthy) | closed | IPv6 | candidates: 2001:db8::1 |


<!-- @trace
source: emfor-dns
updated: 2026-11-12
code:
  - docs/deploy/dns-resolver.md
  - examples/rc.d/emfor
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