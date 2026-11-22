## MODIFIED Requirements

### Requirement: Explain shows the tier and the reason of every backend

`emfor lookup <service> --explain` SHALL print, in addition to the answer, which store it read and whether the store is stale (heartbeat age and `EMFOR_MAX_STALENESS`; a heartbeat later than the current time is shown as `heartbeat <age> in the future`, and a store that is stale for that reason is said to have a heartbeat more than `EMFOR_MAX_STALENESS` ahead of the clock), the `balance` and `on_all_down` of the service, the tier the candidates came from, every backend with whether it is a candidate and, when it is not, the reason, and how `balance` chooses among the candidates.

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

#### Scenario: Heartbeat in the future

- **WHEN** the clock of the host was set back, so that the heartbeat is 60 seconds later than the current time, `EMFOR_MAX_STALENESS` is 3 seconds and an operator runs `emfor lookup api.svc.emfor --explain`
- **THEN** the output shows `heartbeat 1m00s in the future` and `stale: the heartbeat is more than 3.0s ahead of the clock`, and it does not say the heartbeat is older than anything

##### Example: heartbeat later than the current time

| Heartbeat is | Store line contains |
| --- | --- |
| 1 s in the future | `heartbeat 1.0s in the future` and `fresh` |
| 3 s in the future | `heartbeat 3.0s in the future` and `fresh` |
| 60 s in the future | `heartbeat 1m00s in the future` and `stale: the heartbeat is more than 3.0s ahead of the clock` |

### Requirement: JSON output is stable and only grows

`emfor lookup <service> --json` SHALL print one JSON object on standard output with the fields `service`, `via` (`"client"` or `"dns"`), `family` (`null`, `"A"` or `"AAAA"`), `store` (an object with `path`, `heartbeat_age_ms`, an integer number of milliseconds that is negative when the heartbeat is later than the current time, and `stale`), `balance`, `on_all_down` and `result`, an object with `status` (`"ok"` or `"error"`) and either `address` or `error` (an object with `code`, one of the four client error codes, and `message`). With `--explain` it SHALL add `explain`, an object with `tier`, `candidates` and `excluded` (arrays of objects with `id`, and for `excluded` also `reason`), and `selection` (an object with `rule`, `deterministic` and `chosen`, where `chosen` is `null` when `deterministic` is false). Later releases SHALL add fields only. The text output without `--json` is not a stable interface.

#### Scenario: JSON for a fixed choice

- **WHEN** an operator runs `emfor lookup api.svc.emfor --explain --json` for a `failover` service whose first healthy backend is `10.0.0.12:8080`
- **THEN** the object has `via` `client`, `result` with `status` `ok` and `address` `10.0.0.12:8080`, and `explain.selection` with `deterministic` true and `chosen` `10.0.0.12:8080`

#### Scenario: JSON for a rotating choice

- **WHEN** an operator runs `emfor lookup api.svc.emfor --explain --json` for a `round-robin` service
- **THEN** `explain.selection.deterministic` is false and `explain.selection.chosen` is `null`

#### Scenario: Heartbeat in the future in JSON output

- **WHEN** the heartbeat is 60 seconds later than the current time, `EMFOR_MAX_STALENESS` is 3 seconds and an operator runs `emfor lookup api.svc.emfor --explain --json`
- **THEN** `store.heartbeat_age_ms` is a negative number close to -60000 and `store.stale` is true
