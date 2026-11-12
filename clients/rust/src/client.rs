//! One client: a store path, a staleness limit, the lazily opened store and
//! the round-robin counters.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use emfor_format::{SCHEMA_VERSION, ServiceRecord, service_key};

use crate::select::{candidates, is_stale, normalize, pick};
use crate::store::Env;
use crate::{Error, ErrorCode};

pub struct Client {
    path: PathBuf,
    max_staleness_ms: Result<u64, String>,
    /// `None` until an open succeeds; a failed open is retried next lookup.
    env: Mutex<Option<Arc<Env>>>,
    counters: Mutex<HashMap<String, Arc<AtomicU64>>>,
    /// Random per client: each service's counter starts at a random value.
    seed: RandomState,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Client {
    pub fn new(path: PathBuf, max_staleness_ms: Result<u64, String>) -> Self {
        Client {
            path,
            max_staleness_ms,
            env: Mutex::new(None),
            counters: Mutex::new(HashMap::new()),
            seed: RandomState::new(),
        }
    }

    pub fn from_env() -> Self {
        let (path, max) = crate::config::from_env();
        Client::new(path, max)
    }

    fn env(&self) -> Result<Arc<Env>, Error> {
        let mut slot = self.env.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(env) = slot.as_ref() {
            return Ok(env.clone());
        }
        let env = Arc::new(Env::open(&self.path)?);
        *slot = Some(env.clone());
        Ok(env)
    }

    fn next(&self, service: &str) -> u64 {
        let counter = {
            let mut m = self.counters.lock().unwrap_or_else(PoisonError::into_inner);
            m.entry(service.to_string())
                .or_insert_with(|| Arc::new(AtomicU64::new(self.seed.hash_one(service))))
                .clone()
        };
        counter.fetch_add(1, Ordering::Relaxed)
    }

    pub fn lookup(&self, name: &str, affinity_key: Option<&str>) -> Result<String, Error> {
        let max = self
            .max_staleness_ms
            .clone()
            .map_err(|m| Error::new(ErrorCode::Unavailable, m))?;
        let env = self.env()?;
        let service = normalize(name);
        let snap = env.snapshot(&service_key(&service))?;
        let path = self.path.display();
        let schema = snap.schema_version.ok_or_else(|| {
            Error::new(
                ErrorCode::Unavailable,
                format!("store {path} is not initialized: meta/schema_version is missing"),
            )
        })?;
        if schema != SCHEMA_VERSION.to_string() {
            return Err(Error::new(
                ErrorCode::Incompatible,
                format!(
                    "store {path} has format version {schema:?}; this client reads version {SCHEMA_VERSION}"
                ),
            ));
        }
        let heartbeat = snap.heartbeat.ok_or_else(|| {
            Error::new(
                ErrorCode::Incompatible,
                format!("store {path}: meta/heartbeat is missing"),
            )
        })?;
        let heartbeat: u64 = heartbeat.parse().map_err(|_| {
            Error::new(
                ErrorCode::Incompatible,
                format!("store {path}: meta/heartbeat {heartbeat:?} is not a decimal integer"),
            )
        })?;
        let record = snap.record.ok_or_else(|| {
            Error::new(ErrorCode::NotFound, format!("no service named {service:?}"))
        })?;
        let record: ServiceRecord = serde_json::from_str(&record).map_err(|e| {
            Error::new(
                ErrorCode::Incompatible,
                format!("record of service {service:?} cannot be parsed: {e}"),
            )
        })?;
        let stale = is_stale(heartbeat, now_ms(), max);
        let cands = candidates(&record, stale)?;
        Ok(pick(record.balance, &cands, affinity_key, || {
            self.next(&service)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write(dir: &Path, store: serde_json::Value) -> PathBuf {
        let path = dir.join("state.mdb");
        emfor_fixture::write_store(&store, 3000, emfor_fixture::now_ms(), &path).unwrap();
        path
    }

    fn svc(name: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut v = serde_json::json!({"name": name, "on_all_down": "closed", "balance": "failover", "all_down": false,
            "backends": [{"id": "10.0.0.11:8080", "ip": "10.0.0.11", "port": 8080, "state": "healthy", "checked_at": 1, "reason": null}]});
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        v
    }

    fn code(c: &Client, name: &str) -> ErrorCode {
        c.lookup(name, None).unwrap_err().code
    }

    /// What, store (`None`: no file), staleness, name, expected code.
    type Row = (
        &'static str,
        Option<serde_json::Value>,
        Result<u64, String>,
        &'static str,
        ErrorCode,
    );

    #[test]
    fn store_conditions() {
        use ErrorCode::*;
        let dir = tempfile::tempdir().unwrap();
        let ok = || svc("api.svc.emfor", serde_json::json!({}));
        let cases: Vec<Row> = vec![
            (
                "store file does not exist",
                None,
                Ok(3000),
                "api.svc.emfor",
                Unavailable,
            ),
            (
                "schema_version absent",
                Some(serde_json::json!({"heartbeat": "fresh", "services": [ok()]})),
                Ok(3000),
                "api.svc.emfor",
                Unavailable,
            ),
            (
                "EMFOR_MAX_STALENESS=fast",
                Some(serde_json::json!({"schema_version": "1", "services": [ok()]})),
                crate::config::parse_max_staleness("fast"),
                "api.svc.emfor",
                Unavailable,
            ),
            (
                "schema_version 2",
                Some(serde_json::json!({"schema_version": "2", "services": [ok()]})),
                Ok(3000),
                "api.svc.emfor",
                Incompatible,
            ),
            (
                "schema wins over missing service",
                Some(serde_json::json!({"schema_version": "2"})),
                Ok(3000),
                "nope.svc.emfor",
                Incompatible,
            ),
            (
                "heartbeat absent",
                Some(
                    serde_json::json!({"schema_version": "1", "heartbeat": "missing", "services": [ok()]}),
                ),
                Ok(3000),
                "api.svc.emfor",
                Incompatible,
            ),
            (
                "bad json",
                Some(
                    serde_json::json!({"schema_version": "1", "raw": {"svc/api.svc.emfor": "{not json"}}),
                ),
                Ok(3000),
                "api.svc.emfor",
                Incompatible,
            ),
            (
                "weighted",
                Some(
                    serde_json::json!({"schema_version": "1", "services": [svc("api.svc.emfor", serde_json::json!({"balance": "weighted"}))]}),
                ),
                Ok(3000),
                "api.svc.emfor",
                Incompatible,
            ),
            (
                "no such service",
                Some(serde_json::json!({"schema_version": "1", "services": [ok()]})),
                Ok(3000),
                "nope.svc.emfor",
                NotFound,
            ),
            (
                "all unhealthy, closed",
                Some(
                    serde_json::json!({"schema_version": "1", "services": [svc("api.svc.emfor", serde_json::json!({"backends": [{"id": "10.0.0.11:8080", "ip": "10.0.0.11", "port": 8080, "state": "unhealthy", "checked_at": 1, "reason": "refused"}]}))]}),
                ),
                Ok(3000),
                "api.svc.emfor",
                AllDown,
            ),
        ];
        for (i, (what, store, max, name, want)) in cases.into_iter().enumerate() {
            let sub = dir.path().join(i.to_string());
            std::fs::create_dir(&sub).unwrap();
            let path = match store {
                Some(s) => write(&sub, s),
                None => sub.join("state.mdb"),
            };
            let c = Client::new(path, max);
            let e = c.lookup(name, None).unwrap_err();
            assert_eq!(e.code, want, "{what}: {e}");
        }
    }

    #[test]
    fn found_and_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            serde_json::json!({"schema_version": "1", "services": [svc("api.svc.emfor", serde_json::json!({}))]}),
        );
        let c = Client::new(p, Ok(3000));
        for n in ["api.svc.emfor", "API.Svc.Emfor", "api.svc.emfor."] {
            assert_eq!(c.lookup(n, None).unwrap(), "10.0.0.11:8080", "{n}");
        }
        assert_eq!(code(&c, "api.svc.emfor.."), ErrorCode::NotFound);
    }

    #[test]
    fn missing_store_does_not_create_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = Client::new(dir.path().join("state.mdb"), Ok(3000));
        let e = c.lookup("api.svc.emfor", None).unwrap_err();
        assert!(e.message.contains("does not exist"), "{e}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn unreadable_directory_is_not_reported_as_missing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("emfor");
        std::fs::create_dir(&sub).unwrap();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o000)).unwrap();
        // root can search any directory; there is nothing to check then.
        let blocked = std::fs::metadata(sub.join("state.mdb"))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied);
        let e = Client::new(sub.join("state.mdb"), Ok(3000))
            .lookup("api.svc.emfor", None)
            .unwrap_err();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
        if blocked {
            assert_eq!(e.code, ErrorCode::Unavailable);
            assert!(e.message.contains("cannot open store"), "{e}");
            assert!(!e.message.contains("does not exist"), "{e}");
        }
    }

    #[test]
    fn non_lmdb_file_names_the_release_rule() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.mdb");
        std::fs::write(&path, vec![0x5a; 16384]).unwrap();
        let path_text = path.display().to_string();
        let e = Client::new(path, Ok(3000))
            .lookup("api.svc.emfor", None)
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Unavailable);
        assert!(
            e.message
                .contains("install libemfor from the same emfor release as the daemon"),
            "{e}"
        );
        assert!(e.message.contains(&path_text), "{e}");
    }

    /// Spec "Unhealthy backend is never returned": four lookups skip the
    /// unhealthy backend and give each healthy one twice.
    #[test]
    fn round_robin_skips_unhealthy_evenly() {
        let dir = tempfile::tempdir().unwrap();
        let be = |id: &str, state: &str| {
            let (ip, port) = id.split_once(':').unwrap();
            serde_json::json!({"id": id, "ip": ip, "port": port.parse::<u16>().unwrap(),
                               "state": state, "checked_at": 1, "reason": null})
        };
        let store = serde_json::json!({"schema_version": "1", "heartbeat": "fresh", "raw": {},
            "services": [svc("api.svc.emfor", serde_json::json!({"balance": "round-robin", "backends": [
                be("10.0.0.11:8080", "healthy"), be("10.0.0.12:8080", "unhealthy"), be("10.0.0.13:8080", "healthy")]}))]});
        let c = Client::new(write(dir.path(), store), Ok(3000));
        let mut counts = std::collections::BTreeMap::new();
        for _ in 0..4 {
            *counts
                .entry(c.lookup("api.svc.emfor", None).unwrap())
                .or_insert(0) += 1;
        }
        let want: std::collections::BTreeMap<String, i32> = [
            ("10.0.0.11:8080".to_string(), 2),
            ("10.0.0.13:8080".to_string(), 2),
        ]
        .into();
        assert_eq!(counts, want);
    }
}
