//! One client: a store path, a staleness limit, the lazily opened store and
//! the round-robin counters.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use emfor_format::{READABLE_SCHEMA_VERSIONS, ServiceRecord, readable_schema, service_key};

use crate::select::{candidates, is_stale, normalize, pick_at, pool_candidates};
use crate::store::{Env, Snapshot, SnapshotError};
use crate::{Error, ErrorCode};

pub struct Client {
    path: PathBuf,
    max_staleness_ms: Result<u64, String>,
    /// `None` until an open succeeds; a failed open is retried by a later
    /// lookup (see [`OPEN_RETRY`]).
    /// Lookups hold the read lock for their read transaction, so a reopen
    /// (write lock) closes the old environment only when none uses it, and
    /// before it opens the new one: LMDB must not have two environments on
    /// one lock file in a process.
    env: RwLock<Option<Env>>,
    /// Read-locked on each round-robin lookup; written only the first time
    /// a service is looked up.
    counters: RwLock<HashMap<String, Arc<AtomicU64>>>,
    /// Unix ms of the last check for a replaced store file, and the least
    /// time between two checks ([`REPLACE_CHECK_MS`]).
    last_replace_check: AtomicU64,
    replace_check_ms: u64,
    /// The last failed open of a store file that exists, and when it
    /// failed: until [`OPEN_RETRY`] has passed, lookups fail with its error
    /// without opening the file again.
    open_failure: Mutex<Option<(Instant, Error)>>,
    open_retry: Duration,
    /// Random per client: each service's counter starts at a random value.
    seed: RandomState,
}

/// Least time between two checks for a replaced store file. A check costs
/// a `stat`, and a lookup that finds the store stale or fails would
/// otherwise pay it every time, just when applications retry the most.
pub(crate) const REPLACE_CHECK_MS: u64 = 100;

/// Least time between two attempts to open a store file that exists but
/// could not be opened (another LMDB format, no permission, not a store).
/// An attempt maps the file and sets up its lock file under the client's
/// write lock, which stops every other lookup of the process meanwhile; a
/// cause like these needs an operator, so trying once a second is enough,
/// and a store the daemon is still creating is read at most a second later
/// (less than the default `EMFOR_MAX_STALENESS`). A missing file is tried
/// on every lookup: that costs one `stat`, and the daemon may create it at
/// any moment.
pub(crate) const OPEN_RETRY: Duration = Duration::from_secs(1);

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
            env: RwLock::new(None),
            counters: RwLock::new(HashMap::new()),
            last_replace_check: AtomicU64::new(0),
            replace_check_ms: REPLACE_CHECK_MS,
            open_failure: Mutex::new(None),
            open_retry: OPEN_RETRY,
            seed: RandomState::new(),
        }
    }

    pub fn from_env() -> Self {
        let (path, max) = crate::config::from_env();
        Client::new(path, max)
    }

    /// One read transaction on the store, opening it first if needed.
    fn snapshot(&self, key: &str) -> Result<Snapshot, Error> {
        {
            let slot = self.env.read().unwrap_or_else(PoisonError::into_inner);
            if let Some(env) = slot.as_ref() {
                match env.snapshot(key) {
                    // Adopting the new size needs the write lock.
                    Err(SnapshotError::MapResized) => {}
                    r => return r.map_err(|e| e.into_error(&self.path)),
                }
            } else {
                self.check_open_failure()?;
            }
        }
        // The write lock waits until no lookup has a read transaction open.
        let mut slot = self.env.write().unwrap_or_else(PoisonError::into_inner);
        let mut env = match slot.take() {
            Some(env) => env, // another thread opened it meanwhile
            None => self.open()?,
        };
        let snapshot = match env.snapshot(key) {
            Err(SnapshotError::MapResized) => env
                .adopt_map_size()
                .map_err(SnapshotError::Failed)
                .and_then(|()| env.snapshot(key)),
            r => r,
        };
        *slot = Some(env);
        snapshot.map_err(|e| e.into_error(&self.path))
    }

    /// The error of the last failed open while [`OPEN_RETRY`] has not
    /// passed since.
    fn check_open_failure(&self) -> Result<(), Error> {
        match &*self
            .open_failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            Some((at, e)) if at.elapsed() < self.open_retry => Err(e.clone()),
            _ => Ok(()),
        }
    }

    /// Open the store, unless an open failed less than [`OPEN_RETRY`] ago.
    /// Called with the write lock of `env` held.
    fn open(&self) -> Result<Env, Error> {
        self.check_open_failure()?;
        let r = Env::open(&self.path);
        let mut failure = self
            .open_failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *failure = match &r {
            // A missing file (or one that cannot be looked at) cost only a
            // `stat`: no pause.
            Err(e) if self.path.exists() => Some((Instant::now(), e.clone())),
            _ => None,
        };
        r
    }

    /// When the store path now holds another file, switch to it, so a
    /// running client follows a store the daemon replaced without a
    /// restart. Called for a lookup that found the store stale or failed;
    /// checks at most once per [`REPLACE_CHECK_MS`] in the process. True
    /// when the next read sees the new file.
    fn reopen_if_replaced(&self) -> bool {
        let now = now_ms();
        let last = self.last_replace_check.load(Ordering::Relaxed);
        // A clock that went back allows a check.
        if now >= last && now - last < self.replace_check_ms {
            return false;
        }
        if self
            .last_replace_check
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return false; // another thread checks now
        }
        // Under the read lock, so lookups go on while the file is checked.
        let replaced = self
            .env
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(Env::replaced);
        if !replaced {
            return false;
        }
        // The write lock waits until no lookup uses the open store.
        let mut slot = self.env.write().unwrap_or_else(PoisonError::into_inner);
        let Some(old) = slot.as_ref() else {
            return false;
        };
        if !old.replaced() {
            return true; // switched by another thread meanwhile
        }
        if old.lock_file_replaced() {
            // Another lock file: open the new store first and keep the old
            // one if that fails (a file still being written, say).
            match Env::open(&self.path) {
                Ok(env) => {
                    *slot = Some(env); // closes the old environment
                    true
                }
                Err(_) => false,
            }
        } else {
            // The same lock file: the old environment must be closed first.
            *slot = None;
            match Env::open(&self.path) {
                Ok(env) => {
                    *slot = Some(env);
                    true
                }
                Err(_) => false, // opened again on the next lookup
            }
        }
    }

    fn next(&self, service: &str) -> u64 {
        let known = self
            .counters
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(service)
            .cloned();
        let counter = known.unwrap_or_else(|| {
            let mut m = self
                .counters
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            m.entry(service.to_string())
                .or_insert_with(|| Arc::new(AtomicU64::new(self.seed.hash_one(service))))
                .clone()
        });
        counter.fetch_add(1, Ordering::Relaxed)
    }

    pub fn lookup(&self, name: &str, affinity_key: Option<&str>) -> Result<String, Error> {
        match self.lookup_once(name, affinity_key, true) {
            // A store that was set aside as damaged may fail to read, and a
            // service added since is missing from it: errors look too. Not
            // ALL_DOWN: a fresh store answers it from the daemon's checks,
            // and a stale one was looked at already.
            Err(e) if e.code != ErrorCode::AllDown && self.reopen_if_replaced() => {
                self.lookup_once(name, affinity_key, false)
            }
            r => r,
        }
    }

    fn lookup_once(
        &self,
        name: &str,
        affinity_key: Option<&str>,
        may_reopen: bool,
    ) -> Result<String, Error> {
        let max = self
            .max_staleness_ms
            .clone()
            .map_err(|m| Error::new(ErrorCode::Unavailable, m))?;
        let service = normalize(name);
        let snap = self.snapshot(&service_key(&service))?;
        let path = self.path.display();
        let schema = snap.schema_version.ok_or_else(|| {
            Error::new(
                ErrorCode::Unavailable,
                format!("store {path} is not initialized: meta/schema_version is missing"),
            )
        })?;
        if !readable_schema(&schema) {
            return Err(Error::new(
                ErrorCode::Incompatible,
                format!(
                    "store {path} has format version {schema:?}; this client reads versions {READABLE_SCHEMA_VERSIONS}"
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
        let now = now_ms();
        let stale = is_stale(heartbeat, now, max);
        if stale && may_reopen && self.reopen_if_replaced() {
            return self.lookup_once(name, affinity_key, false);
        }
        let record = snap.record.ok_or_else(|| {
            Error::new(ErrorCode::NotFound, format!("no service named {service:?}"))
        })?;
        let record: ServiceRecord = serde_json::from_str(&record).map_err(|e| {
            Error::new(
                ErrorCode::Incompatible,
                format!("record of service {service:?} cannot be parsed: {e}"),
            )
        })?;
        let cands = pool_candidates(&record, candidates(&record, stale)?, stale);
        // A stale store has no slow start: its recovery times may be old.
        let slow_start_ms = if stale { 0 } else { record.slow_start_ms };
        Ok(pick_at(
            record.balance,
            &cands,
            affinity_key,
            snap.site.as_deref(),
            slow_start_ms,
            now,
            || self.next(&service),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Staleness for tests that need a fresh store but do not test
    /// staleness: a host that stalls for a few seconds between writing the
    /// store and the lookup must not make it stale.
    const FRESH_FOR_TESTS_MS: u64 = 60_000;

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
                "schema_version 3",
                Some(serde_json::json!({"schema_version": "3", "services": [ok()]})),
                Ok(3000),
                "api.svc.emfor",
                Incompatible,
            ),
            (
                "schema wins over missing service",
                Some(serde_json::json!({"schema_version": "3"})),
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
                "balance is not a string",
                Some(
                    serde_json::json!({"schema_version": "1", "services": [svc("api.svc.emfor", serde_json::json!({"balance": 5}))]}),
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
        let c = Client::new(p, Ok(FRESH_FOR_TESTS_MS));
        for n in ["api.svc.emfor", "API.Svc.Emfor", "api.svc.emfor."] {
            assert_eq!(c.lookup(n, None).unwrap(), "10.0.0.11:8080", "{n}");
        }
        assert_eq!(code(&c, "api.svc.emfor.."), ErrorCode::NotFound);
    }

    /// Put a new store file at `dir/state.mdb` the way the daemon replaces a
    /// damaged one: the old file moves away (here: is unlinked by the
    /// rename) and the lock file is removed.
    fn replace(dir: &Path, store: serde_json::Value) {
        let next = dir.join("next.mdb");
        emfor_fixture::write_store(&store, 3000, emfor_fixture::now_ms(), &next).unwrap();
        std::fs::rename(&next, dir.join("state.mdb")).unwrap();
        let _ = std::fs::remove_file(dir.join("next.mdb-lock"));
        let _ = std::fs::remove_file(dir.join("state.mdb-lock"));
    }

    /// No least time between replacement checks, so a test can replace the
    /// store and look up again at once.
    fn unthrottled(mut c: Client) -> Client {
        c.replace_check_ms = 0;
        c
    }

    fn backend(ip: &str) -> serde_json::Value {
        // `open`: a stale store still answers, with any backend.
        serde_json::json!({"on_all_down": "open", "backends": [{"id": format!("{ip}:8080"), "ip": ip, "port": 8080, "state": "healthy", "checked_at": 1, "reason": null}]})
    }

    #[test]
    fn stale_store_is_followed_to_its_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            serde_json::json!({"schema_version": "1", "heartbeat": "stale",
                "services": [svc("api.svc.emfor", backend("10.0.0.11"))]}),
        );
        let c = unthrottled(Client::new(p, Ok(3000)));
        assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.11:8080");
        replace(
            dir.path(),
            serde_json::json!({"schema_version": "1",
                "services": [svc("api.svc.emfor", backend("10.0.0.22"))]}),
        );
        assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.22:8080");
    }

    #[test]
    fn service_only_in_the_replacement_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            serde_json::json!({"schema_version": "1",
                "services": [svc("api.svc.emfor", backend("10.0.0.11"))]}),
        );
        let c = unthrottled(Client::new(p, Ok(3000)));
        assert_eq!(code(&c, "db.svc.emfor"), ErrorCode::NotFound);
        replace(
            dir.path(),
            serde_json::json!({"schema_version": "1",
                "services": [svc("db.svc.emfor", backend("10.0.0.33"))]}),
        );
        assert_eq!(c.lookup("db.svc.emfor", None).unwrap(), "10.0.0.33:8080");
    }

    fn stale_store(dir: &Path, ip: &str) -> PathBuf {
        write(
            dir,
            serde_json::json!({"schema_version": "1", "heartbeat": "stale",
                "services": [svc("api.svc.emfor", backend(ip))]}),
        )
    }

    /// Replacement checks are at most `REPLACE_CHECK_MS` apart: a lookup
    /// right after one keeps the open store, a later one follows.
    #[test]
    fn replacement_checks_are_spaced() {
        let dir = tempfile::tempdir().unwrap();
        let c = Client::new(stale_store(dir.path(), "10.0.0.11"), Ok(3000));
        assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.11:8080");
        replace(
            dir.path(),
            serde_json::json!({"schema_version": "1",
                "services": [svc("api.svc.emfor", backend("10.0.0.22"))]}),
        );
        assert_eq!(
            c.lookup("api.svc.emfor", None).unwrap(),
            "10.0.0.11:8080",
            "checked less than {REPLACE_CHECK_MS}ms ago"
        );
        std::thread::sleep(std::time::Duration::from_millis(REPLACE_CHECK_MS + 20));
        assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.22:8080");
    }

    /// ALL_DOWN from a fresh store is the daemon's answer: no check.
    #[test]
    fn all_down_in_a_fresh_store_does_not_check() {
        let dir = tempfile::tempdir().unwrap();
        let down = serde_json::json!({"all_down": true, "backends": [{"id": "10.0.0.11:8080", "ip": "10.0.0.11", "port": 8080, "state": "unhealthy", "checked_at": 1, "reason": "refused"}]});
        let p = write(
            dir.path(),
            serde_json::json!({"schema_version": "1", "services": [svc("api.svc.emfor", down)]}),
        );
        let c = Client::new(p, Ok(FRESH_FOR_TESTS_MS));
        assert_eq!(code(&c, "api.svc.emfor"), ErrorCode::AllDown);
        assert_eq!(c.last_replace_check.load(Ordering::Relaxed), 0);
        assert_eq!(code(&c, "db.svc.emfor"), ErrorCode::NotFound);
        assert_ne!(
            c.last_replace_check.load(Ordering::Relaxed),
            0,
            "other errors check"
        );
    }

    /// A new file that cannot be opened leaves the open store in use.
    #[test]
    fn unreadable_replacement_keeps_the_open_store() {
        let dir = tempfile::tempdir().unwrap();
        let p = stale_store(dir.path(), "10.0.0.11");
        let c = unthrottled(Client::new(p.clone(), Ok(3000)));
        assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.11:8080");
        // As the daemon does: the old file moves away, the lock file goes,
        // and a new file appears that is not (yet) a store.
        std::fs::rename(&p, dir.path().join("state.mdb.corrupt")).unwrap();
        std::fs::remove_file(dir.path().join("state.mdb-lock")).unwrap();
        std::fs::write(&p, b"not lmdb yet").unwrap();
        for _ in 0..2 {
            // The second lookup reads the store again after the failed open.
            assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.11:8080");
        }
        // Once it is a store, the client follows it.
        std::fs::remove_file(&p).unwrap();
        let _ = std::fs::remove_file(dir.path().join("state.mdb-lock"));
        replace(
            dir.path(),
            serde_json::json!({"schema_version": "1",
                "services": [svc("api.svc.emfor", backend("10.0.0.22"))]}),
        );
        assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.22:8080");
    }

    /// A data file replaced under the same lock file is followed too, by
    /// closing the open store first.
    #[test]
    fn replacement_with_the_same_lock_file_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let c = unthrottled(Client::new(stale_store(dir.path(), "10.0.0.11"), Ok(3000)));
        assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.11:8080");
        let next = dir.path().join("next.mdb");
        emfor_fixture::write_store(
            &serde_json::json!({"schema_version": "1",
                "services": [svc("api.svc.emfor", backend("10.0.0.22"))]}),
            3000,
            emfor_fixture::now_ms(),
            &next,
        )
        .unwrap();
        std::fs::rename(&next, dir.path().join("state.mdb")).unwrap();
        let _ = std::fs::remove_file(dir.path().join("next.mdb-lock"));
        assert!(dir.path().join("state.mdb-lock").exists(), "lock file kept");
        assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.22:8080");
    }

    #[test]
    fn deleted_store_keeps_the_open_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            serde_json::json!({"schema_version": "1", "heartbeat": "stale",
                "services": [svc("api.svc.emfor", backend("10.0.0.11"))]}),
        );
        let c = Client::new(p.clone(), Ok(3000));
        assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.11:8080");
        std::fs::remove_file(&p).unwrap();
        assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.11:8080");
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

    /// A store file that cannot be opened is tried again only after
    /// `open_retry`: lookups meanwhile fail with the same error.
    #[test]
    fn unopenable_store_is_retried_after_a_pause() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.mdb");
        std::fs::write(&path, vec![0x5a; 16384]).unwrap();
        let mut c = Client::new(path.clone(), Ok(FRESH_FOR_TESTS_MS));
        // Long enough that a slow host cannot reach it while the store is
        // written below; set to zero afterwards instead of sleeping.
        c.open_retry = Duration::from_secs(600);
        let first = c.lookup("api.svc.emfor", None).unwrap_err();
        assert_eq!(first.code, ErrorCode::Unavailable, "{first}");
        // Now a store, but the pause has not passed.
        std::fs::remove_file(&path).unwrap();
        let _ = std::fs::remove_file(dir.path().join("state.mdb-lock"));
        write(
            dir.path(),
            serde_json::json!({"schema_version": "1", "services": [svc("api.svc.emfor", serde_json::json!({}))]}),
        );
        assert_eq!(
            c.lookup("api.svc.emfor", None).unwrap_err(),
            first,
            "opened again within the pause"
        );
        // The pause is over: the next lookup opens the store.
        c.open_retry = Duration::ZERO;
        assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.11:8080");
    }

    /// A missing store has no pause: it is opened as soon as it exists.
    #[test]
    fn missing_store_is_retried_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let c = Client::new(dir.path().join("state.mdb"), Ok(FRESH_FOR_TESTS_MS));
        assert_eq!(code(&c, "api.svc.emfor"), ErrorCode::Unavailable);
        write(
            dir.path(),
            serde_json::json!({"schema_version": "1", "services": [svc("api.svc.emfor", serde_json::json!({}))]}),
        );
        assert_eq!(c.lookup("api.svc.emfor", None).unwrap(), "10.0.0.11:8080");
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
        let c = Client::new(write(dir.path(), store), Ok(FRESH_FOR_TESTS_MS));
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

    /// Lookups of `api.svc.emfor` from one client, counted per result
    /// (errors as their code name).
    fn counts(
        dir: &Path,
        site: Option<&str>,
        on_all_down: &str,
        backends: &[(&str, Option<&str>, &str)],
        n: usize,
    ) -> std::collections::BTreeMap<String, usize> {
        let bes: Vec<serde_json::Value> = backends
            .iter()
            .map(|(id, site, state)| {
                let (ip, port) = id.split_once(':').unwrap();
                serde_json::json!({"id": id, "ip": ip, "port": port.parse::<u16>().unwrap(),
                                   "site": site, "state": state, "checked_at": 1, "reason": null})
            })
            .collect();
        let raw = match site {
            Some(s) => serde_json::json!({"meta/site": s}),
            None => serde_json::json!({}),
        };
        let store = serde_json::json!({"schema_version": "1", "heartbeat": "fresh", "raw": raw,
            "services": [svc("api.svc.emfor", serde_json::json!({"balance": "locality",
                "on_all_down": on_all_down, "backends": bes}))]});
        let path = dir.join(format!("locality-{}.mdb", rand_suffix()));
        emfor_fixture::write_store(&store, 3000, emfor_fixture::now_ms(), &path).unwrap();
        let c = Client::new(path, Ok(3000));
        let mut out = std::collections::BTreeMap::new();
        for _ in 0..n {
            let k = match c.lookup("api.svc.emfor", Some("user-42")) {
                Ok(a) => a,
                Err(e) => e.code.as_str().to_string(),
            };
            *out.entry(k).or_insert(0) += 1;
        }
        out
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::AtomicU64;
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }

    /// Spec "Locality prefers backends in the host's site", every row of
    /// the locality-outcomes example plus the local-site-healthy scenario.
    #[test]
    fn locality_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, c) = ("10.0.0.11:8080", "10.0.0.12:8080", "10.0.0.13:8080");
        let m = |pairs: &[(&str, usize)]| -> std::collections::BTreeMap<String, usize> {
            pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
        };
        assert_eq!(
            counts(
                dir.path(),
                Some("dc1"),
                "open",
                &[
                    (a, Some("dc1"), "healthy"),
                    (b, Some("dc1"), "healthy"),
                    (c, Some("dc2"), "healthy")
                ],
                4
            ),
            m(&[(a, 2), (b, 2)])
        );
        assert_eq!(
            counts(
                dir.path(),
                Some("dc1"),
                "open",
                &[(a, Some("dc1"), "unhealthy"), (c, Some("dc2"), "healthy")],
                2
            ),
            m(&[(c, 2)])
        );
        assert_eq!(
            counts(
                dir.path(),
                Some("dc1"),
                "open",
                &[(a, Some("dc1"), "unhealthy"), (c, Some("dc2"), "unhealthy")],
                2
            ),
            m(&[(a, 2)])
        );
        assert_eq!(
            counts(
                dir.path(),
                Some("dc1"),
                "closed",
                &[(a, Some("dc1"), "unhealthy"), (c, Some("dc2"), "unhealthy")],
                1
            ),
            m(&[("ALL_DOWN", 1)])
        );
        assert_eq!(
            counts(
                dir.path(),
                None,
                "open",
                &[(a, Some("dc1"), "healthy"), (c, Some("dc2"), "healthy")],
                2
            ),
            m(&[(a, 1), (c, 1)])
        );
        assert_eq!(
            counts(
                dir.path(),
                Some("dc3"),
                "open",
                &[(a, Some("dc1"), "healthy"), (c, None, "healthy")],
                2
            ),
            m(&[(a, 1), (c, 1)])
        );
    }
}
