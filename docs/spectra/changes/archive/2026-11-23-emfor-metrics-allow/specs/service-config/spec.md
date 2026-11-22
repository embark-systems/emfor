## MODIFIED Requirements

### Requirement: Config validation rules

The daemon SHALL reject a config that violates any of these rules and SHALL report every violation, each prefixed with its TOML path:

- `interval` from 100ms to 24h; `timeout` at least 10ms and not greater than `interval` (equal is allowed, because checks of one backend run one after another and cannot overlap); `rise` at least 1; `fall` at least 1; `jitter` from 0% to 50%; `on_all_down` is `open` or `closed`; `balance` is `round-robin`, `sticky`, `failover`, `weighted`, `locality`, or `fastest`; `dns_ttl` is a whole number of seconds from 0s to 3600s; `peers` is a boolean.
- `backends` is non-empty; every entry is a string or a table; a string, and a table's `addr`, is `IPv4:port` or `[IPv6]:port` with port 1–65535; a table has `addr` and no keys other than `addr`, `weight`, `site`, `backup`; `weight` is an integer from 1 to 100; no `addr` repeats within a service.
- The top-level `site` and every backend `site` are, after ASCII lowercasing, 1 to 63 characters of `[a-z0-9-]` that do not start or end with `-`.
- `check` names a registered checker and `check_options` passes that checker's validation.
- `[dns] listen` is a non-empty array of `IPv4:port` or `[IPv6]:port` with port 1–65535 and no repeats; `[dns] zone` follows the service name rules after the same normalization and is at most 242 characters, so that the SOA name `hostmaster.<zone>` fits in 253; `[dns] ttl` is a whole number of seconds from 0s to 3600s; `[dns] rate_limit` is an integer from 0 to 1000000.
- With `[dns]` present, for every service whose name is in the zone, the SRV target name of each backend (`<ip label>.<service>`, spec dns-server "SRV target names") is at most 253 characters; otherwise the error is on that backend, `services."<name>".backends[<i>]`. A 230-character service name fits an IPv4 backend but not an IPv6 one.
- `[metrics] listen` is a non-empty array of `IPv4:port` or `[IPv6]:port` with port 1–65535 and no repeats; `[metrics] allow`, when present, is a non-empty array of CIDR strings (an IPv4 or IPv6 address, `/`, and a prefix length of 0–32 or 0–128) with no repeated entry; `[metrics]` has no keys other than `listen` and `allow`.
- `[peers]` follows the peer group configuration rules (spec peer-group), and the top-level `site` is set when `[peers]` is present.
- No unknown keys anywhere in the file.

#### Scenario: Multiple errors reported together

- **WHEN** a service sets `timeout = "3s"` with `interval = "2s"` and also lists backend `10.0.0.1:70000`
- **THEN** validation reports both the timeout error and the port error

##### Example: validation outcomes

| Setting | Value | Result |
| --- | --- | --- |
| `interval` | `"50ms"` | error: interval below 100ms |
| `interval` | `"25h"` | error: interval above 24h |
| `timeout` | `"9ms"` | error: timeout below 10ms |
| `interval` / `timeout` | `"1s"` / `"1s"` | ok |
| `interval` / `timeout` | `"1s"` / `"2s"` | error: timeout must not exceed interval |
| `jitter` | `"60%"` | error: jitter above 50% |
| `backends` | `["db.local:5432"]` | error: backend must be an IP literal with port |
| `backends` | `["[2001:db8::1]:8080"]` | ok |
| `backends` | `[{ addr = "10.0.0.1:80", weight = 3, site = "dc1" }]` | ok |
| `backends` | `[{ weight = 3 }]` | error: backend table needs addr |
| `backends` | `[{ addr = "10.0.0.1:80", wieght = 3 }]` | error: unknown key |
| `backends` | `[{ addr = "10.0.0.1:80", weight = 0 }]` | error: weight must be from 1 to 100 |
| `backends` | `[{ addr = "10.0.0.1:80", weight = 101 }]` | error: weight must be from 1 to 100 |
| `backends` | `["10.0.0.1:80", { addr = "10.0.0.1:80" }]` | error: duplicate backend |
| `backends` | `[{ addr = "10.0.0.1:80", site = "dc_1" }]` | error: invalid site label |
| `site` (top level) | `"DC-1"` | ok, normalized to `dc-1` |
| `site` (top level) | `"-dc1"` | error: invalid site label |
| `chekc` (typo key) | `"tcp"` | error: unknown key |
| `check` | `"redis"` | error: unknown checker `redis` |
| `balance` | `"weighted"` | ok |
| `balance` | `"fastest"` | ok |
| `balance` | `"least-conn"` | error: balance must be round-robin, sticky, failover, weighted, locality, or fastest |
| `dns_ttl` | `"1500ms"` | error: dns_ttl must be whole seconds |
| `[dns] ttl` | `"2h"` | error: ttl above 3600s |
| `[dns] listen` | `[]` | error: listen must not be empty |
| `[dns] rate_limit` | `-1` | error: rate_limit must be 0 to 1000000 |
| `[dns] listen` | `["localhost:5300"]` | error: listen address must be an IP literal with port |
| `[dns] zone` | `"svc..emfor"` | error: invalid zone name |
| `[dns] zone` | a valid name of 243 characters | error: zone name longer than 242 characters |
| `[dns] zone` | `"SVC.Emfor."` | ok, normalized to `svc.emfor` |
| `backends` of a service named in the zone, 230 characters | `["[2001:db8::1]:80"]` | error: service name too long for this backend's SRV target name |
| `[metrics] listen` | `[]` | error: listen must not be empty |
| `[metrics] listen` | `["localhost:5301"]` | error: listen address must be an IP literal with port |
| `[metrics] listen` | `["127.0.0.1:5301", "127.0.0.1:5301"]` | error: duplicate listen address |
| `[metrics] allow` | `[]` | error: allow must not be empty |
| `[metrics] allow` | `["10.0.0.1"]` | error: allow entry must be an address with a prefix length, such as "10.0.0.0/8" |
| `[metrics] allow` | `["10.0.0.0/33"]` | error: allow entry must be an address with a prefix length, such as "10.0.0.0/8" |
| `[metrics] allow` | `["10.0.0.0/8", "10.0.0.0/8"]` | error: duplicate allow entry |
| `[metrics] allow` | `["10.0.0.0/8", "fd00::/8"]` | valid |
| `[metrics] path` | `"/m"` | error: unknown key |
| `[peers]` | present, no top-level `site` | error: site is required when [peers] is set |
| `[peers] members` | `["10.0.0.5:5302"]` | error: members must list 2 to 64 addresses |
| `peers` | `false` | ok |
| `peers` | `"no"` | error: peers must be true or false |
| `peers` (no `[peers]` table) | `false` | ok |
