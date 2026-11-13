## MODIFIED Requirements

### Requirement: Peer group state in the store

When the config has a `[peers]` table, the daemon SHALL keep the key `meta/peers` holding a JSON object with these fields:

- `listen`: this daemon's `[peers] listen`;
- `members`: an array of `{"addr":<member>,"live":<bool>}` for every member other than this daemon, in config order;
- `digest_mismatch`: an array of `{"service":<name>,"member":<member>}` for every current digest mismatch (spec peer-group), ordered by service name and then member;
- `disagreements`: an array of `{"service":<name>,"backend":<id>,"down":<d>,"fresh":<f>}` for every current disagreement (spec peer-group), ordered by service name and then backend id.
- `services_off`: an array of the names of this daemon's services whose effective `peers` setting is `false` (spec peer-group, "Services left out of the peer group"), in byte order of the name.

Members SHALL be written as in the config (IPv6 as `[addr]:port`). On startup the daemon SHALL write the key in the same transaction that writes `meta/started_at`, with every member `live` false, `digest_mismatch` and `disagreements` empty, and `services_off` from the config; when the config has no `[peers]` table it SHALL delete the key in that transaction. While running, the daemon SHALL compare the content with the last one handed to the writer once per second and, when it changed, write the new content in the next commit. A failed commit SHALL be retried like any other key. Clients and the DNS server SHALL NOT read the key, and `meta/schema_version` SHALL stay `1`.

#### Scenario: Peer state written at start

- **WHEN** the daemon starts with `[peers] listen = "127.0.0.1:5302"` and `members = ["127.0.0.1:5302", "127.0.0.1:5303"]`
- **THEN** once `meta/started_at` is written, key `meta/peers` holds `{"listen":"127.0.0.1:5302","members":[{"addr":"127.0.0.1:5303","live":false}],"digest_mismatch":[],"disagreements":[],"services_off":[]}`

#### Scenario: Peer key from an earlier run removed

- **WHEN** the store holds `meta/peers` from an earlier run and the daemon starts with a config that has no `[peers]` table
- **THEN** key `meta/peers` does not exist once `meta/started_at` is written

#### Scenario: Liveness and disagreement reach the store

- **WHEN** a daemon with `listen = "127.0.0.1:5302"` and `members = ["127.0.0.1:5302", "127.0.0.1:5303", "127.0.0.1:5304"]` sees both other members become live, and backend `127.0.0.1:8080` of `api.svc.emfor` becomes a disagreement with 2 of 3 votes down
- **THEN** within 2 seconds `meta/peers` holds `"members":[{"addr":"127.0.0.1:5303","live":true},{"addr":"127.0.0.1:5304","live":true}]` and `"disagreements":[{"service":"api.svc.emfor","backend":"127.0.0.1:8080","down":2,"fresh":3}]`

#### Scenario: Services left out are listed

- **WHEN** the daemon runs with `[peers]` and services `web.svc.emfor` and `db.svc.emfor` have `peers = false`, and a reload then sets `peers = true` for `web.svc.emfor`
- **THEN** `meta/peers` holds `"services_off":["db.svc.emfor","web.svc.emfor"]` at start and `"services_off":["db.svc.emfor"]` within 2 seconds after the reload

