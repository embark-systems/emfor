//! emfor state store format, version 2.
//!
//! The daemon writes these keys and records into a single-file LMDB
//! environment (`MDB_NOSUBDIR`, default database only). Keys and values are
//! UTF-8 strings; service records are JSON. Readers in every client language
//! parse this format directly, so any change here is a format change and must
//! bump [`SCHEMA_VERSION`].

pub mod rules;

use serde::{Deserialize, Serialize};

/// Current store format version, stored at [`KEY_SCHEMA_VERSION`]. Version
/// 2 added [`BackendRecord::backup`]; it is otherwise version 1.
pub const SCHEMA_VERSION: u32 = 2;

/// The format versions a reader can read, as a message names them.
pub const READABLE_SCHEMA_VERSIONS: &str = "1 and 2";

/// Whether a reader can read a store whose [`KEY_SCHEMA_VERSION`] is `v`:
/// version 2, or version 1, whose backends are all read as not backups.
pub fn readable_schema(v: &str) -> bool {
    v == "1" || v == SCHEMA_VERSION.to_string()
}

/// LMDB reader slots in the store's lock file, shared by every reading
/// process on the host. LMDB's default is 126. The process that first opens
/// the store while nobody else has it open sizes the lock file, so the daemon
/// and every client set the same value. A lookup holds a slot only for its
/// own read transaction, and finding a free slot scans only the slots used
/// so far, so a larger table does not slow lookups down.
pub const MAX_READERS: u32 = 1024;

/// Format version, decimal string.
pub const KEY_SCHEMA_VERSION: &str = "meta/schema_version";
/// Unix time in milliseconds as a decimal string; `"0"` after a clean shutdown.
pub const KEY_HEARTBEAT: &str = "meta/heartbeat";
/// Daemon start time in Unix milliseconds, decimal string.
pub const KEY_STARTED_AT: &str = "meta/started_at";
/// The host's site label (config top-level `site`); absent when not set.
pub const KEY_SITE: &str = "meta/site";
/// `[metrics] listen` of the running daemon as a JSON array of strings;
/// absent when the config has no `[metrics]` table.
pub const KEY_METRICS_LISTEN: &str = "meta/metrics_listen";
/// `[dns] listen` of the running daemon as a JSON array of strings; absent
/// when the config has no `[dns]` table.
pub const KEY_DNS_LISTEN: &str = "meta/dns_listen";
/// Peer group state of the running daemon as a JSON object (`listen`,
/// `members`, `digest_mismatch`, `disagreements`); absent when the config
/// has no `[peers]` table. `emfor status` reads it; clients ignore it.
pub const KEY_PEERS: &str = "meta/peers";
/// Prefix of service record keys: `svc/<service name>`.
pub const SERVICE_KEY_PREFIX: &str = "svc/";

/// Store key for a service record.
pub fn service_key(name: &str) -> String {
    format!("{SERVICE_KEY_PREFIX}{name}")
}

/// Prefix of backend state history keys: `hist/<service name>`. Only
/// `emfor status` reads them; clients ignore them like any unknown key.
pub const HIST_KEY_PREFIX: &str = "hist/";
/// Most transitions kept per backend.
pub const HISTORY_LIMIT: usize = 20;

/// Store key for a service's backend state history.
pub fn hist_key(name: &str) -> String {
    format!("{HIST_KEY_PREFIX}{name}")
}

/// Health state of a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthState {
    Unknown,
    Healthy,
    Unhealthy,
}

impl HealthState {
    pub fn as_str(self) -> &'static str {
        match self {
            HealthState::Unknown => "unknown",
            HealthState::Healthy => "healthy",
            HealthState::Unhealthy => "unhealthy",
        }
    }
}

impl std::fmt::Display for HealthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What readers do when no backend of a service is healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnAllDown {
    /// Pick from all backends anyway.
    #[default]
    Open,
    /// Return a failure.
    Closed,
}

/// How one backend of a service is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Balance {
    /// Candidates in turn.
    #[default]
    RoundRobin,
    /// Same backend for the same affinity key; round-robin without a key.
    Sticky,
    /// First candidate in config order.
    Failover,
    /// In proportion to backend weights.
    Weighted,
    /// Candidates in the host's site first, then in turn.
    Locality,
    /// Candidates in the daemon's `pool` (the fast backends) in turn; all
    /// candidates in turn when the pool has none of them.
    Fastest,
    /// A value this reader does not know (written by a newer daemon). Readers
    /// treat it as round-robin; the daemon never writes it.
    #[serde(other)]
    Unknown,
}

impl Balance {
    pub fn as_str(self) -> &'static str {
        match self {
            Balance::RoundRobin => "round-robin",
            Balance::Sticky => "sticky",
            Balance::Failover => "failover",
            Balance::Weighted => "weighted",
            Balance::Locality => "locality",
            Balance::Fastest => "fastest",
            Balance::Unknown => "unknown",
        }
    }
}

/// One backend inside a [`ServiceRecord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendRecord {
    /// Backend string as written in config (IPv6 as `[addr]:port`).
    pub id: String,
    pub ip: String,
    pub port: u16,
    /// Weight for `weighted`, 1 to 100; missing in records from older daemons.
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// Site label for `locality`; `None` when the backend has none.
    #[serde(default)]
    pub site: Option<String>,
    /// `true` for a backup backend, used only while no primary backend
    /// passes its checks; missing (read as `false`) in version 1 stores.
    #[serde(default)]
    pub backup: bool,
    pub state: HealthState,
    /// Unix milliseconds when the last check finished; `None` before the first check.
    pub checked_at: Option<u64>,
    /// Message of the last `down` or `error` result; `None` after an `up`.
    pub reason: Option<String>,
    /// Smoothed duration of up checks in microseconds; `None` before the
    /// first up check, after the backend turned unhealthy, and in records
    /// from older daemons.
    #[serde(default)]
    pub latency_us: Option<u64>,
    /// Unix milliseconds of the check that last turned the backend from
    /// unhealthy to healthy; `None` after any other transition, before the
    /// first one, and in records from older daemons. Slow start counts from it.
    #[serde(default)]
    pub recovered_at: Option<u64>,
}

fn default_weight() -> u32 {
    1
}

/// Value stored at `svc/<name>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRecord {
    pub name: String,
    pub on_all_down: OnAllDown,
    /// Missing in records written before this field existed; reads as round-robin.
    #[serde(default)]
    pub balance: Balance,
    /// `true` when no backend is [`HealthState::Healthy`].
    pub all_down: bool,
    /// The service's `slow_start` in milliseconds; 0 when slow start is off
    /// and in records from older daemons.
    #[serde(default)]
    pub slow_start_ms: u64,
    /// Backends in config order.
    pub backends: Vec<BackendRecord>,
    /// Ids of the fast backends in config order, decided by the daemon.
    /// Written only for `fastest` services; a missing pool reads as empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<Vec<String>>,
}

impl ServiceRecord {
    /// Recompute [`ServiceRecord::all_down`] from backend states.
    pub fn refresh_all_down(&mut self) {
        self.all_down = !self
            .backends
            .iter()
            .any(|b| b.state == HealthState::Healthy);
    }
}

/// One health state transition caused by a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// `checked_at` of the check that caused the transition.
    pub at: u64,
    pub from: HealthState,
    pub to: HealthState,
    /// The backend's `reason` after that check.
    pub reason: Option<String>,
}

/// Value stored at `hist/<name>`: per backend id, transitions oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceHistory {
    pub backends: std::collections::BTreeMap<String, Vec<HistoryEntry>>,
}

impl ServiceHistory {
    /// Append `entry` to backend `id`, dropping the oldest past [`HISTORY_LIMIT`].
    pub fn push(&mut self, id: &str, entry: HistoryEntry) {
        let entries = self.backends.entry(id.to_owned()).or_default();
        entries.push(entry);
        if entries.len() > HISTORY_LIMIT {
            let excess = entries.len() - HISTORY_LIMIT;
            entries.drain(..excess);
        }
    }

    /// Keep only backends for which `keep` returns true and that have
    /// entries. Returns whether anything was removed.
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) -> bool {
        let before = self.backends.len();
        self.backends
            .retain(|id, entries| !entries.is_empty() && keep(id));
        self.backends.len() != before
    }

    pub fn is_empty(&self) -> bool {
        self.backends.values().all(Vec::is_empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_example() -> ServiceRecord {
        ServiceRecord {
            name: "db.svc.emfor".into(),
            on_all_down: OnAllDown::Closed,
            balance: Balance::RoundRobin,
            all_down: false,
            slow_start_ms: 0,
            backends: vec![BackendRecord {
                id: "[2001:db8::5]:5432".into(),
                ip: "2001:db8::5".into(),
                port: 5432,
                weight: 1,
                site: None,
                state: HealthState::Healthy,
                checked_at: Some(1762905600123),
                reason: None,
                latency_us: Some(1834),
                backup: false,
                recovered_at: None,
            }],
            pool: None,
        }
    }

    #[test]
    fn serializes_spec_record_example_exactly() {
        let json = serde_json::to_string(&db_example()).unwrap();
        assert_eq!(
            json,
            r#"{"name":"db.svc.emfor","on_all_down":"closed","balance":"round-robin","all_down":false,"slow_start_ms":0,"backends":[{"id":"[2001:db8::5]:5432","ip":"2001:db8::5","port":5432,"weight":1,"site":null,"backup":false,"state":"healthy","checked_at":1762905600123,"reason":null,"latency_us":1834,"recovered_at":null}]}"#
        );
    }

    #[test]
    fn round_trips() {
        let rec = db_example();
        let back: ServiceRecord =
            serde_json::from_str(&serde_json::to_string(&rec).unwrap()).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn unknown_state_before_first_check() {
        let json = r#"{"name":"a","on_all_down":"open","all_down":true,"backends":[{"id":"10.0.0.1:80","ip":"10.0.0.1","port":80,"state":"unknown","checked_at":null,"reason":null}]}"#;
        let rec: ServiceRecord = serde_json::from_str(json).unwrap();
        assert_eq!(rec.backends[0].state, HealthState::Unknown);
        assert_eq!(rec.backends[0].checked_at, None);
    }

    #[test]
    fn missing_balance_reads_as_round_robin() {
        let json = r#"{"name":"a","on_all_down":"open","all_down":true,"backends":[]}"#;
        let rec: ServiceRecord = serde_json::from_str(json).unwrap();
        assert_eq!(rec.balance, Balance::RoundRobin);
    }

    #[test]
    fn balance_values_serialize() {
        for (b, s) in [
            (Balance::RoundRobin, "\"round-robin\""),
            (Balance::Sticky, "\"sticky\""),
            (Balance::Failover, "\"failover\""),
            (Balance::Weighted, "\"weighted\""),
            (Balance::Locality, "\"locality\""),
            (Balance::Fastest, "\"fastest\""),
        ] {
            assert_eq!(serde_json::to_string(&b).unwrap(), s);
            assert_eq!(format!("\"{}\"", b.as_str()), s);
        }
    }

    #[test]
    fn unknown_balance_parses() {
        let b: Balance = serde_json::from_str("\"some-future-method\"").unwrap();
        assert_eq!(b, Balance::Unknown);
    }

    #[test]
    fn older_backend_object_reads_weight_1_and_no_site() {
        let json = r#"{"id":"10.0.0.1:80","ip":"10.0.0.1","port":80,"state":"healthy","checked_at":null,"reason":null,"zone_hint":"x"}"#;
        let b: BackendRecord = serde_json::from_str(json).unwrap();
        assert_eq!((b.weight, b.site, b.latency_us), (1, None, None));
    }

    #[test]
    fn weight_and_site_serialize() {
        let mut rec = db_example();
        rec.backends[0].weight = 3;
        rec.backends[0].site = Some("dc1".into());
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains(r#""weight":3,"site":"dc1""#), "{json}");
    }

    /// Spec "Store record format version 1", example "slow start fields
    /// written".
    #[test]
    fn slow_start_fields_written() {
        let mut rec = db_example();
        rec.slow_start_ms = 30000;
        rec.backends[0].recovered_at = Some(1762905600123);
        let json = serde_json::to_string(&rec).unwrap();
        assert!(
            json.contains(r#""all_down":false,"slow_start_ms":30000,"#),
            "{json}"
        );
        assert!(
            json.contains(r#""latency_us":1834,"recovered_at":1762905600123}"#),
            "{json}"
        );
    }

    /// Records from older daemons have neither field.
    #[test]
    fn missing_slow_start_fields_read_as_off() {
        let json = r#"{"name":"a","on_all_down":"open","all_down":false,"backends":[{"id":"10.0.0.1:80","ip":"10.0.0.1","port":80,"state":"healthy","checked_at":1,"reason":null}]}"#;
        let rec: ServiceRecord = serde_json::from_str(json).unwrap();
        assert_eq!(rec.slow_start_ms, 0);
        assert_eq!(rec.backends[0].recovered_at, None);
        // A version 1 record has no `backup`: not a backup.
        assert!(!rec.backends[0].backup);
    }

    /// Spec "Store record format version 2", example "backup written".
    #[test]
    fn backup_written() {
        let mut rec = db_example();
        rec.backends[0].backup = true;
        let json = serde_json::to_string(&rec).unwrap();
        assert!(
            json.contains(r#""site":null,"backup":true,"state""#),
            "{json}"
        );
        let back: ServiceRecord = serde_json::from_str(&json).unwrap();
        assert!(back.backends[0].backup);
    }

    #[test]
    fn readable_schema_versions() {
        assert_eq!(SCHEMA_VERSION, 2);
        for v in ["1", "2"] {
            assert!(readable_schema(v), "{v}");
        }
        for v in ["0", "3", "", "01", " 2"] {
            assert!(!readable_schema(v), "{v:?}");
        }
    }

    #[test]
    fn pool_written_only_when_set() {
        let mut rec = db_example();
        assert!(!serde_json::to_string(&rec).unwrap().contains("pool"));
        rec.balance = Balance::Fastest;
        rec.pool = Some(vec!["[2001:db8::5]:5432".into()]);
        let json = serde_json::to_string(&rec).unwrap();
        assert!(
            json.ends_with(r#""recovered_at":null}],"pool":["[2001:db8::5]:5432"]}"#),
            "{json}"
        );
        let missing: ServiceRecord = serde_json::from_str(
            r#"{"name":"a","on_all_down":"open","balance":"fastest","all_down":true,"backends":[]}"#,
        )
        .unwrap();
        assert_eq!(missing.pool, None);
    }

    #[test]
    fn service_key_format() {
        assert_eq!(service_key("api.svc.emfor"), "svc/api.svc.emfor");
        assert_eq!(hist_key("api.svc.emfor"), "hist/api.svc.emfor");
    }

    fn entry(at: u64) -> HistoryEntry {
        HistoryEntry {
            at,
            from: HealthState::Healthy,
            to: HealthState::Unhealthy,
            reason: Some("connect: Connection refused".into()),
        }
    }

    #[test]
    fn history_serializes_spec_shape() {
        let mut h = ServiceHistory::default();
        h.push("10.0.0.12:8080", entry(1762905600123));
        assert_eq!(
            serde_json::to_string(&h).unwrap(),
            r#"{"backends":{"10.0.0.12:8080":[{"at":1762905600123,"from":"healthy","to":"unhealthy","reason":"connect: Connection refused"}]}}"#
        );
        let back: ServiceHistory =
            serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(back, h);
    }

    #[test]
    fn history_capped_at_20() {
        let mut h = ServiceHistory::default();
        for at in 1..=21 {
            h.push("10.0.0.11:8080", entry(at));
        }
        let ats: Vec<u64> = h.backends["10.0.0.11:8080"].iter().map(|e| e.at).collect();
        assert_eq!(ats, (2..=21).collect::<Vec<_>>());
    }

    #[test]
    fn history_retain_drops_unlisted_and_empty_backends() {
        let mut h = ServiceHistory::default();
        h.push("10.0.0.11:8080", entry(1));
        h.push("10.0.0.99:8080", entry(2));
        h.backends.insert("10.0.0.12:8080".into(), Vec::new());
        assert!(h.retain(|id| id != "10.0.0.99:8080"));
        assert_eq!(h.backends.keys().collect::<Vec<_>>(), ["10.0.0.11:8080"]);
        assert!(!h.retain(|_| true));
        assert!(!h.is_empty());
    }
}
