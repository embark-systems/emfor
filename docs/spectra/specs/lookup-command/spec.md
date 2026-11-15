# lookup-command Specification

## Purpose

Defines `emfor lookup`, the command that answers from the shell which backend an application would get for a service, and with `--explain` why: which tier of candidates it came from and why each backend is in or out. It exists so that operators and cron jobs need no client library to find out where a lookup lands and why.

## Requirements

### Requirement: Lookup command answers like the client

`emfor lookup <service>` SHALL read the state store at the `[store] path` of the config file given with `--config` (the same file and rules as `emfor status`), judge staleness with `EMFOR_MAX_STALENESS` (default 3 seconds) as the client library does, and print the address of one backend, `ip:port` (`[ip]:port` for IPv6), followed by a newline, on standard output. It SHALL use the same steps as `emfor_client::lookup`: service name normalization, store format check, stale heartbeat, candidates, `balance`. `--key <k>` SHALL be the `affinity_key` of the lookup, and an empty key SHALL count as no key. The command SHALL NOT write the store.

For `failover` and for `sticky` with a key the printed address is the one the client library returns on the same store. For `round-robin`, `weighted`, `locality`, `fastest` and `sticky` without a key the printed address SHALL be one of the candidates; the rotation of a new process starts at a random point, as the client's does.

#### Scenario: Failover returns the first healthy backend

- **WHEN** the store holds `api.svc.emfor` with `balance` `failover`, backend `10.0.0.11:8080` unhealthy and backends `10.0.0.12:8080` and `10.0.0.13:8080` healthy without a `reason`, and an operator runs `emfor lookup api.svc.emfor`
- **THEN** standard output is `10.0.0.12:8080` followed by a newline and the exit status is 0

#### Scenario: Sticky with a key gives the same backend

- **WHEN** the store holds `api.svc.emfor` with `balance` `sticky` and two healthy backends, and an operator runs `emfor lookup api.svc.emfor --key user-42` twice
- **THEN** both runs print the same address, and it is the address `emfor_client::lookup("api.svc.emfor", Some("user-42"))` returns on that store

#### Scenario: Round-robin prints one candidate

- **WHEN** the store holds `api.svc.emfor` with `balance` `round-robin` and healthy backends `10.0.0.11:8080` and `10.0.0.12:8080`, and an operator runs `emfor lookup api.svc.emfor`
- **THEN** standard output is `10.0.0.11:8080` or `10.0.0.12:8080`

#### Scenario: Lookup does not write the store

- **WHEN** an operator runs `emfor lookup api.svc.emfor` as a user who can only read the store
- **THEN** the command succeeds and the store files are unchanged

---
### Requirement: Explain shows the tier and the reason of every backend

`emfor lookup <service> --explain` SHALL print, in addition to the answer, which store it read and whether the store is stale (heartbeat age and `EMFOR_MAX_STALENESS`), the `balance` and `on_all_down` of the service, the tier the candidates came from, every backend with whether it is a candidate and, when it is not, the reason, and how `balance` chooses among the candidates.

The tiers, in order, SHALL be those of `rules::healthy_or_all`: healthy primary backends whose last check passed (no `reason`); healthy backup backends whose last check passed; healthy primary backends including those whose last check failed; healthy backup backends including those; and, when `on_all_down` is `open`, `unknown` backends whose last check passed, every `unknown` backend, and every backend. The tier of the candidates is the first one that is not empty. In a stale store no backend counts as healthy or `unknown`, so an `open` service uses every backend and a `closed` service has no candidates.

The reason a backend is not a candidate SHALL be one of: `stale` (the store is stale, so no backend counts as healthy or `unknown`; this happens for a `closed` service, since an `open` service then uses every backend); `unhealthy`; `unknown` (a better tier exists); `last check failed` (a passing backend of the same kind exists); `backup held back` (a primary backend is a candidate); `not in the pool` (`fastest`); `not at this site` (`locality`). A candidate in slow start SHALL show its slow start step and effective weight.

For `failover` and for `sticky` with a key, the explanation SHALL name the backend that is chosen and say that the choice is fixed. For `round-robin`, `weighted`, `locality`, `fastest`, `sticky` without a key, and a `balance` the client does not know, it SHALL list the candidates and the rule (the weights as a ratio for `weighted`, the site subset for `locality`, the pool for `fastest`) and SHALL say that the actual choice depends on the rotation start of the process, which is random in a new process.

#### Scenario: Primary and backup backends

- **WHEN** the store is fresh and `api.svc.emfor` has `balance` `round-robin` and `on_all_down` `open`, with backends `10.0.0.11:8080` (healthy, no `reason`), `10.0.0.12:8080` (healthy, `reason` `connect: Connection refused`), `10.0.0.13:8080` (`backup`, healthy, no `reason`) and `10.0.0.14:8080` (unhealthy), and an operator runs `emfor lookup api.svc.emfor --explain`
- **THEN** the tier is "healthy primary backends whose last check passed", the candidates are `10.0.0.11:8080` only, `10.0.0.12:8080` is excluded as `last check failed`, `10.0.0.13:8080` as `backup held back`, `10.0.0.14:8080` as `unhealthy`, and the printed address is `10.0.0.11:8080`

#### Scenario: Stale store

- **WHEN** the heartbeat is older than `EMFOR_MAX_STALENESS` and `api.svc.emfor` has `on_all_down` `open` and two healthy backends, and an operator runs `emfor lookup api.svc.emfor --explain`
- **THEN** the output says the store is stale, the tier is "every backend" and both backends are candidates

#### Scenario: Closed service with no healthy backend

- **WHEN** `api.svc.emfor` has `on_all_down` `closed` and no healthy backend, and an operator runs `emfor lookup api.svc.emfor --explain`
- **THEN** the output lists every backend with its reason, says no backend can be chosen because `on_all_down` is `closed`, prints no address on standard output, and the exit status is 1

##### Example: tier of the candidates

| Backends | `on_all_down` | Tier |
| --- | --- | --- |
| one healthy primary, no `reason` | `open` | healthy primary backends whose last check passed |
| only healthy primaries with a `reason`, and a healthy backup without one | `open` | healthy backup backends whose last check passed |
| only healthy primaries, all with a `reason` | `open` | healthy primary backends including those whose last check failed |
| no healthy backend, one `unknown` without `reason` | `open` | `unknown` backends whose last check passed |
| no healthy backend, none `unknown` | `open` | every backend |
| no healthy backend | `closed` | none |

---
### Requirement: Explain does not change any state

`emfor lookup` with or without `--explain` SHALL NOT advance any rotation counter of the process in a way that a later lookup could observe, SHALL NOT write the store, and SHALL NOT start a daemon. Calling `Client::explain` of `emfor-client` SHALL leave the rotation sequence of later `lookup` calls on the same client unchanged.

#### Scenario: Explain between two lookups

- **WHEN** a client looks up a `round-robin` service three times, and in a second client with the same rotation start `explain` is called between the first and the second lookup
- **THEN** both clients return the same three addresses in the same order

---
### Requirement: DNS mode explains the answer of the DNS server

`emfor lookup <service> --dns A` and `--dns AAAA` imply `--explain` and SHALL explain what the DNS server of the daemon would answer for an A or an AAAA query, from the store: only backends whose address is of that family are taken into account before the tiers of "Explain shows the tier and the reason of every backend" are applied; the store counts as fresh whatever its heartbeat; `sticky` has no key and chooses as `round-robin`; `weighted` is explained as the daemon chooses (smooth weighted round-robin). When the service has no backend of the family the command SHALL say so and exit 1. The output SHALL state that it reads the store, which the daemon updates up to about one second after it decides a `reason`, so that for that second it can differ from the live DNS answer. It SHALL list candidates and rules and SHALL NOT say which candidate the next query gets, because the rotation counters of the daemon are in its memory. SRV answers are not explained.

#### Scenario: A and AAAA have different tiers

- **WHEN** `api.svc.emfor` has `on_all_down` `open` and backends `10.0.0.11:8080` (healthy, no `reason`) and `10.0.0.12:8080` (unhealthy) of IPv4, and `[2001:db8::11]:8080` (healthy, `reason` `connect: Connection refused`) of IPv6, and an operator runs `emfor lookup api.svc.emfor --explain --dns A` and then `--dns AAAA`
- **THEN** for A the tier is "healthy primary backends whose last check passed" with candidate `10.0.0.11:8080`; for AAAA the tier is "healthy primary backends including those whose last check failed" with candidate `[2001:db8::11]:8080`

#### Scenario: No backend of the family

- **WHEN** `api.svc.emfor` has only IPv4 backends and an operator runs `emfor lookup api.svc.emfor --dns AAAA`
- **THEN** the output says the service has no AAAA backend, standard output has no address and the exit status is 1

#### Scenario: Unknown family

- **WHEN** an operator runs `emfor lookup api.svc.emfor --dns SRV`
- **THEN** the command prints a usage error and exits 2

---
### Requirement: JSON output is stable and only grows

`emfor lookup <service> --json` SHALL print one JSON object on standard output with the fields `service`, `via` (`"client"` or `"dns"`), `family` (`null`, `"A"` or `"AAAA"`), `store` (an object with `path`, `heartbeat_age_ms` and `stale`), `balance`, `on_all_down` and `result`, an object with `status` (`"ok"` or `"error"`) and either `address` or `error` (an object with `code`, one of the four client error codes, and `message`). With `--explain` it SHALL add `explain`, an object with `tier`, `candidates` and `excluded` (arrays of objects with `id`, and for `excluded` also `reason`), and `selection` (an object with `rule`, `deterministic` and `chosen`, where `chosen` is `null` when `deterministic` is false). Later releases SHALL add fields only. The text output without `--json` is not a stable interface.

#### Scenario: JSON for a fixed choice

- **WHEN** an operator runs `emfor lookup api.svc.emfor --explain --json` for a `failover` service whose first healthy backend is `10.0.0.12:8080`
- **THEN** the object has `via` `client`, `result` with `status` `ok` and `address` `10.0.0.12:8080`, and `explain.selection` with `deterministic` true and `chosen` `10.0.0.12:8080`

#### Scenario: JSON for a rotating choice

- **WHEN** an operator runs `emfor lookup api.svc.emfor --explain --json` for a `round-robin` service
- **THEN** `explain.selection.deterministic` is false and `explain.selection.chosen` is `null`

---
### Requirement: Lookup command exit status

`emfor lookup` SHALL exit 0 when it printed an address, or, with `--dns`, when the service has candidates for the family (a rotating choice has no single address to print), 1 when the service has no backend to choose (`ALL_DOWN`, or no backend of the family with `--dns`), 2 when the service does not exist (`NOT_FOUND`), the command line is invalid or the config file cannot be read, and 3 when the store cannot be used (`UNAVAILABLE`) or has a format this command does not read (`INCOMPATIBLE`). `--explain` SHALL NOT change the exit status, and when the lookup fails it SHALL still print the explanation that is known.

#### Scenario: Exit statuses

- **WHEN** an operator runs `emfor lookup` in each situation of the table below
- **THEN** the exit status is the one in the table

##### Example: exit status table

| Situation | Exit status |
| --- | --- |
| An address was printed | 0 |
| `closed` service with no healthy backend | 1 |
| `--dns AAAA` for a service with only IPv4 backends | 1 |
| No service of that name | 2 |
| `--dns SRV` or an unknown option | 2 |
| The store file does not exist | 3 |
| The store has a format version this command does not read | 3 |
