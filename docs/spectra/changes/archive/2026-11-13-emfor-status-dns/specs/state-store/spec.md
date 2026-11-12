## ADDED Requirements

### Requirement: DNS listen addresses in the store

When the config has a `[dns]` table, the daemon SHALL write `meta/dns_listen` on startup, in the same transaction that writes `meta/started_at`: a JSON array of strings holding the `[dns] listen` addresses in config order, IPv4 as `<ip>:<port>` and IPv6 as `[<ip>]:<port>`. When the config has no `[dns]` table, the daemon SHALL delete `meta/dns_listen` in that transaction. A reload SHALL NOT change the key, because `[dns] listen` cannot change without a restart. Clients and the DNS server SHALL NOT read the key, and `meta/schema_version` SHALL stay `1`.

#### Scenario: DNS table written at start

- **WHEN** the daemon starts with `[dns] listen = ["127.0.0.1:5300", "[::1]:5300"]`
- **THEN** key `meta/dns_listen` holds `["127.0.0.1:5300","[::1]:5300"]` once `meta/started_at` is written

#### Scenario: DNS key from an earlier run removed

- **WHEN** the store holds `meta/dns_listen` from an earlier run and the daemon starts with a config that has no `[dns]` table
- **THEN** key `meta/dns_listen` does not exist once `meta/started_at` is written
