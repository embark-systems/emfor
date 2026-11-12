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

/// One backend inside a [`ServiceRecord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendRecord {
    /// Backend string as written in config (IPv6 as `[addr]:port`).
    pub id: String,
    pub ip: String,
    pub port: u16,
    pub state: HealthState,
    /// Unix milliseconds when the last check finished; `None` before the first check.
    pub checked_at: Option<u64>,
    /// Message of the last `down` or `error` result; `None` after an `up`.
    pub reason: Option<String>,
}

/// Value stored at `svc/<name>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRecord {
    pub name: String,
    pub on_all_down: OnAllDown,
    /// `true` when no backend is [`HealthState::Healthy`].
    pub all_down: bool,
    /// Backends in config order.
    pub backends: Vec<BackendRecord>,
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
            all_down: false,
            backends: vec![BackendRecord {
                id: "[2001:db8::5]:5432".into(),
                ip: "2001:db8::5".into(),
                port: 5432,
                state: HealthState::Healthy,
                checked_at: Some(1762905600123),
                reason: None,
            }],
        }
    }

    #[test]
    fn serializes_spec_record_example_exactly() {
        let json = serde_json::to_string(&db_example()).unwrap();
        assert_eq!(
            json,
            r#"{"name":"db.svc.emfor","on_all_down":"closed","all_down":false,"backends":[{"id":"[2001:db8::5]:5432","ip":"2001:db8::5","port":5432,"state":"healthy","checked_at":1762905600123,"reason":null}]}"#
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
    fn service_key_format() {
        assert_eq!(service_key("api.svc.emfor"), "svc/api.svc.emfor");
    }
}
