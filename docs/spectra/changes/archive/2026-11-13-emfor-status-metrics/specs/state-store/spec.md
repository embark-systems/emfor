## ADDED Requirements

### Requirement: Metrics listen addresses in the store

When the config has a `[metrics]` table, the daemon SHALL write `meta/metrics_listen` on startup, in the same transaction that writes `meta/started_at`: a JSON array of strings holding the `[metrics] listen` addresses in config order, IPv4 as `<ip>:<port>` and IPv6 as `[<ip>]:<port>`. When the config has no `[metrics]` table, the daemon SHALL delete `meta/metrics_listen` in that transaction. A reload SHALL NOT change the key, because `[metrics] listen` cannot change without a restart. Clients and the DNS server SHALL NOT read the key, and `meta/schema_version` SHALL stay `1`.

#### Scenario: Metrics table written at start

- **WHEN** the daemon starts with `[metrics] listen = ["127.0.0.1:5301", "[::1]:5301"]`
- **THEN** key `meta/metrics_listen` holds `["127.0.0.1:5301","[::1]:5301"]` once `meta/started_at` is written

#### Scenario: Key from an earlier run removed

- **WHEN** the store holds `meta/metrics_listen` from an earlier run and the daemon starts with a config that has no `[metrics]` table
- **THEN** key `meta/metrics_listen` does not exist once `meta/started_at` is written
