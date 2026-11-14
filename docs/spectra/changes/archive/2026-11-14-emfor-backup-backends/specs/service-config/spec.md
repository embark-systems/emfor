## MODIFIED Requirements

### Requirement: Host site and backend weight and site

The config SHALL accept an optional top-level `site`, the label of the data centre this host is in. Each `backends` entry SHALL be either a backend string or a table with the key `addr` (a backend string) and the optional keys `weight`, `site` and `backup`. `weight` SHALL default to 1, `site` to none and `backup` to `false`; `backup` SHALL be a boolean. A service whose every backend has `backup = true` SHALL be a config error at the service's `backends` path with the message `at least one backend must not be a backup`. A string entry SHALL be equivalent to a table with only `addr`. `weight` and `site` SHALL be accepted under every `balance` value; only `weighted` uses `weight` and only `locality` uses `site`. Site labels SHALL be ASCII-lowercased when read. `backup` SHALL be accepted under every `balance` value. A change of the top-level `site`, a backend's `weight`, a backend's `site`, or a backend's `backup` SHALL take effect on reload, and the backend SHALL keep its health state.

#### Scenario: Mixed backend forms

- **WHEN** a service lists `backends = ["10.0.0.10:8080", { addr = "10.0.0.11:8080", weight = 3, site = "DC1" }]`
- **THEN** the service has backend `10.0.0.10:8080` with weight 1 and no site, and backend `10.0.0.11:8080` with weight 3 and site `dc1`

#### Scenario: Weight change applies on reload

- **WHEN** a `weighted` service changes a backend from `weight = 1` to `weight = 4` and the daemon receives `SIGHUP`
- **THEN** the store record for that backend shows `weight` 4 and the DNS answers follow the new weight

#### Scenario: Backup backends

- **WHEN** a service lists `backends = ["10.0.0.10:8080", { addr = "10.0.1.10:8080", backup = true }]`
- **THEN** the service has backend `10.0.0.10:8080` with `backup` false and backend `10.0.1.10:8080` with `backup` true

#### Scenario: Only backup backends

- **WHEN** service `api.svc.emfor` lists only `{ addr = "10.0.1.10:8080", backup = true }`
- **THEN** `emfor check-config` reports `services."api.svc.emfor".backends: at least one backend must not be a backup` and exits 2

##### Example: backup values

| `backup` value in a backend table | Result |
| --- | --- |
| absent | not a backup |
| `false` | not a backup |
| `true` | backup |
| `"yes"` | config error: `backup must be true or false` |
