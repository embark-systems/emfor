//! emfor state store format, version 1.
//!
//! The daemon writes these keys and records into a single-file LMDB
//! environment (`MDB_NOSUBDIR`, default database only). Keys and values are
//! UTF-8 strings; service records are JSON. Readers in every client language
//! parse this format directly, so any change here is a format change and must
//! bump [`SCHEMA_VERSION`].

use serde::{Deserialize, Serialize};

/// Current store format version, stored at [`KEY_SCHEMA_VERSION`].
pub const SCHEMA_VERSION: u32 = 1;

/// Format version, decimal string.
pub const KEY_SCHEMA_VERSION: &str = "meta/schema_version";
/// Unix time in milliseconds as a decimal string; `"0"` after a clean shutdown.
pub const KEY_HEARTBEAT: &str = "meta/heartbeat";
/// Daemon start time in Unix milliseconds, decimal string.
pub const KEY_STARTED_AT: &str = "meta/started_at";
/// The host's site label (config top-level `site`); absent when not set.
pub const KEY_SITE: &str = "meta/site";
/// Prefix of service record keys: `svc/<service name>`.
pub const SERVICE_KEY_PREFIX: &str = "svc/";

/// Store key for a service record.
pub fn service_key(name: &str) -> String {
    format!("{SERVICE_KEY_PREFIX}{name}")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn db_example() -> ServiceRecord {
        ServiceRecord {
            name: "db.svc.emfor".into(),
            on_all_down: OnAllDown::Closed,
            balance: Balance::RoundRobin,
            all_down: false,
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
            }],
            pool: None,
        }
    }

    #[test]
    fn serializes_spec_record_example_exactly() {
        let json = serde_json::to_string(&db_example()).unwrap();
        assert_eq!(
            json,
            r#"{"name":"db.svc.emfor","on_all_down":"closed","balance":"round-robin","all_down":false,"backends":[{"id":"[2001:db8::5]:5432","ip":"2001:db8::5","port":5432,"weight":1,"site":null,"state":"healthy","checked_at":1762905600123,"reason":null,"latency_us":1834}]}"#
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

    #[test]
    fn pool_written_only_when_set() {
        let mut rec = db_example();
        assert!(!serde_json::to_string(&rec).unwrap().contains("pool"));
        rec.balance = Balance::Fastest;
        rec.pool = Some(vec!["[2001:db8::5]:5432".into()]);
        let json = serde_json::to_string(&rec).unwrap();
        assert!(
            json.ends_with(r#""latency_us":1834}],"pool":["[2001:db8::5]:5432"]}"#),
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
    }
}
