## MODIFIED Requirements

### Requirement: Daemon metrics

The body SHALL also contain `emfor_build_info{version="<emfor version>"} 1` (gauge), `emfor_start_time_seconds` (gauge) with the daemon start time in Unix seconds, and `emfor_store_commit_failures_total` (counter), the number of failed state store commits since the daemon started. After the first successful commit it SHALL contain `emfor_store_last_commit_timestamp_seconds` (gauge), the Unix time of the last successful commit, `emfor_store_used_bytes` (gauge), the bytes of the store up to its last used page, and `emfor_store_map_size_bytes` (gauge), the map size. For each plugin of the applied config it SHALL contain `emfor_plugin_up{plugin="<name>"}` (gauge), 1 while the plugin process runs and has completed the handshake and 0 otherwise, and `emfor_plugin_restarts_total{plugin="<name>"}` (counter), the number of times the plugin was started again after it exited. When `[peers]` is configured it SHALL contain `emfor_peer_up{peer="<ip:port>"}` (gauge) for every member other than this daemon, 1 while the member is live and 0 otherwise; `emfor_peer_messages_rejected_total{reason="<reason>"}` (counter) for each of the reasons `malformed`, `auth`, `sender`, `site`, `stale` and `replay`, starting at 0; and `emfor_peer_notifications_total{direction="sent"}` and `{direction="received"}` (counters), the `down` messages sent (one per receiving member) and the `down` messages accepted. With `[peers]` it SHALL also contain `emfor_peer_results_total{direction="sent"}` and `{direction="received"}` (counters), the `result` messages sent (one per receiving member) and the `result` messages accepted, and `emfor_peer_disagreements` (gauge), the number of backends that are currently a disagreement (spec peer-group). Without `[peers]` it SHALL contain no series whose name starts with `emfor_peer_`.

#### Scenario: Plugin restart counted

- **WHEN** a plugin exits during a check and is started again
- **THEN** `emfor_plugin_restarts_total{plugin="<name>"}` increases by 1

#### Scenario: Store commit failure counted

- **WHEN** one commit to the state store fails
- **THEN** `emfor_store_commit_failures_total` increases by 1

#### Scenario: Peer metrics

- **WHEN** a daemon with `members = ["127.0.0.1:5302", "127.0.0.1:5303"]` and `listen = "127.0.0.1:5302"` has accepted a heartbeat from 127.0.0.1:5303 within the last 3 seconds and has dropped one message with a wrong tag
- **THEN** the body contains `emfor_peer_up{peer="127.0.0.1:5303"} 1` and `emfor_peer_messages_rejected_total{reason="auth"} 1`

#### Scenario: Peer result metrics

- **WHEN** a daemon in a peer group of three is an assigned checker of one backend, has made 4 `up` checks of it, and has one disagreement
- **THEN** the body contains `emfor_peer_results_total{direction="sent"} 8` and `emfor_peer_disagreements 1`

