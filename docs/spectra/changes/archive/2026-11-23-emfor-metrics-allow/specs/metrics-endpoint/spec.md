## ADDED Requirements

### Requirement: Metrics source allow-list

When `[metrics] allow` is set, the metrics server SHALL compare the address of the peer of every connection with the CIDR entries of `allow` (an IPv4-mapped IPv6 peer address SHALL be compared as the IPv4 address it maps), and SHALL answer a connection whose address is in no entry with status 403 `Forbidden`, an empty body, `Content-Length: 0` and `Connection: close`, without reading the request and without logging. A connection from an address in an entry SHALL be served as without `allow`. Loopback addresses SHALL get no exception: they are served only when an entry contains them. Without `allow` every source SHALL be served. When `[metrics] listen` has an address that is not a loopback address and `allow` is not set, the daemon SHALL log one warning per such address at startup and again after each accepted reload, saying that every host that can reach it can read every service name, backend address and check failure reason, and that `allow` or a firewall limits who can; it SHALL log no such warning when `allow` is set. A reload that changes `[metrics] allow` SHALL be rejected with the error `metrics allow settings require restart` on `metrics.allow`.

#### Scenario: Source inside an allowed network

- **WHEN** the daemon runs with `[metrics] listen = ["127.0.0.1:5301"]` and `allow = ["127.0.0.0/8"]`, and a client at 127.0.0.1 sends `GET /metrics`
- **THEN** the answer is status 200 with the metrics body

#### Scenario: Source outside every allowed network

- **WHEN** the daemon runs with `allow = ["10.0.0.0/8"]` and a client at 127.0.0.1 sends `GET /metrics`
- **THEN** the answer is status 403 with an empty body, and the body holds no metric

#### Scenario: Source matching

- **WHEN** `allow` holds the entries listed and a connection arrives from the address listed
- **THEN** the connection is served or refused as listed

##### Example: allow and peer address

| `allow` | Peer address | Answer |
| --- | --- | --- |
| `["10.0.0.0/8"]` | `10.20.30.40` | served |
| `["10.0.0.0/8"]` | `11.0.0.1` | 403 |
| `["10.0.0.5/32"]` | `10.0.0.5` | served |
| `["10.0.0.5/32"]` | `10.0.0.6` | 403 |
| `["fd00::/8"]` | `fd12::1` | served |
| `["10.0.0.0/8"]` | `::ffff:10.1.2.3` | served |
| `["0.0.0.0/0"]` | `192.0.2.1` | served |
| `["0.0.0.0/0"]` | `2001:db8::1` | 403 |

#### Scenario: Not set

- **WHEN** `[metrics]` has no `allow`
- **THEN** every source is served, as before

#### Scenario: Warning for a non-loopback address without allow

- **WHEN** the config has `[metrics] listen = ["10.0.0.5:5301"]` and no `allow`, and the daemon starts, and again after a reload
- **THEN** each of the two times one warning names `10.0.0.5:5301`, and with `allow` set or with only loopback addresses there is no such warning

#### Scenario: Allow change requires restart

- **WHEN** the daemon runs with `[metrics] allow = ["10.0.0.0/8"]`, the config file is changed to `["10.0.0.0/8", "192.0.2.0/24"]`, and the daemon receives `SIGHUP`
- **THEN** the reload is rejected with the error "metrics allow settings require restart" and the metrics server keeps using the old list
