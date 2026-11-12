//! The single writer task for the LMDB store.
//!
//! All store writes go through one task fed by one channel, so layout changes
//! (start, reload) and check results are applied in the order they were sent.
//! A health state change is committed immediately; `checked_at` / `reason` /
//! `latency_us` / `pool` changes and the heartbeat are committed once per
//! second. For `fastest` services the writer recomputes `pool` on every
//! update and layout change, so the store and the live state agree. A failed commit is
//! logged and retried at the next tick.
//!
//! Every health state change is also appended to the service's
//! `hist/<name>` history in the same transaction. History survives restarts:
//! the caller hands the stored history to [`spawn_live`], and layout changes
//! prune it to the configured services and backends.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use emfor_format::{
    BackendRecord, Balance, HIST_KEY_PREFIX, HealthState, HistoryEntry, KEY_HEARTBEAT,
    KEY_SCHEMA_VERSION, KEY_SITE, KEY_STARTED_AT, OnAllDown, SCHEMA_VERSION, SERVICE_KEY_PREFIX,
    ServiceHistory, ServiceRecord, hist_key, service_key,
};
use tokio::sync::{mpsc, oneshot};
use tokio::time::MissedTickBehavior;

use crate::config::Backend;
use crate::live::{LiveService, LiveState};
use crate::schedule::{BackendUpdate, unix_ms};
use crate::state::fast_pool;
use crate::store::{Store, delete_prefix};

/// One store operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Put(String, String),
    Delete(String),
    DeletePrefix(String),
}

/// Where the writer commits. [`Store`] is the real one; tests wrap it to
/// inject failures.
pub trait Sink: Send + 'static {
    /// Apply all `ops` in one transaction.
    fn commit(&mut self, ops: &[Op]) -> Result<(), String>;
}

fn commit_to_store(store: &Store, ops: &[Op]) -> Result<(), String> {
    store
        .write(|txn, db| {
            for op in ops {
                match op {
                    Op::Put(k, v) => db.put(txn, k, v)?,
                    Op::Delete(k) => {
                        db.delete(txn, k)?;
                    }
                    Op::DeletePrefix(p) => delete_prefix(txn, db, p)?,
                }
            }
            Ok(())
        })
        .map_err(|e| e.to_string())
}

impl Sink for Store {
    fn commit(&mut self, ops: &[Op]) -> Result<(), String> {
        commit_to_store(self, ops)
    }
}

/// Shared store, so tests (and future readers in-process) can read while the writer runs.
impl Sink for Arc<Store> {
    fn commit(&mut self, ops: &[Op]) -> Result<(), String> {
        commit_to_store(self, ops)
    }
}

/// Desired shape of one service after start or reload.
#[derive(Debug, Clone)]
pub struct ServiceLayout {
    pub name: Arc<str>,
    pub on_all_down: OnAllDown,
    pub balance: Balance,
    /// Per-service override of `[dns] ttl`, in seconds.
    pub dns_ttl: Option<u32>,
    pub backends: Vec<Backend>,
    /// Updates from tasks of any other generation are dropped.
    pub generation: u64,
    /// Discard all backend state (new service or `check` / `check_options` changed).
    pub reset: bool,
}

#[derive(Debug)]
pub enum WriterMsg {
    /// Result of one check.
    Update(BackendUpdate),
    /// Start (`fresh_start`: delete every `svc/*` and write meta keys) or reload.
    /// Services not listed are deleted.
    SetLayout {
        fresh_start: bool,
        /// Top-level `site`: written to `meta/site`, which is deleted when `None`.
        site: Option<String>,
        services: Vec<ServiceLayout>,
    },
    /// Flush, write `meta/heartbeat = "0"`, and stop.
    Shutdown(oneshot::Sender<()>),
}

struct Writer<S: Sink> {
    sink: S,
    records: BTreeMap<Arc<str>, ServiceRecord>,
    generations: BTreeMap<Arc<str>, u64>,
    /// Ops not yet committed (from failed commits or pending layout changes).
    pending: Vec<Op>,
    /// Services whose record changed since the last commit.
    dirty: BTreeSet<Arc<str>>,
    /// Backend state history per service.
    histories: BTreeMap<Arc<str>, ServiceHistory>,
    /// Services whose history changed since the last commit.
    hist_dirty: BTreeSet<Arc<str>>,
    /// Updated before every commit attempt.
    live: LiveState,
}

/// Spawn the writer task. It runs until it receives [`WriterMsg::Shutdown`]
/// or every sender is dropped.
pub fn spawn<S: Sink>(
    sink: S,
    rx: mpsc::UnboundedReceiver<WriterMsg>,
) -> tokio::task::JoinHandle<()> {
    spawn_with_tick(sink, rx, Duration::from_secs(1))
}

/// Like [`spawn`], also keeping `live` in line with the records and
/// starting from `history`, the backend state history read from the store.
pub fn spawn_live<S: Sink>(
    sink: S,
    rx: mpsc::UnboundedReceiver<WriterMsg>,
    live: LiveState,
    history: BTreeMap<String, ServiceHistory>,
) -> tokio::task::JoinHandle<()> {
    spawn_inner(sink, rx, Duration::from_secs(1), live, history)
}

pub fn spawn_with_tick<S: Sink>(
    sink: S,
    rx: mpsc::UnboundedReceiver<WriterMsg>,
    tick: Duration,
) -> tokio::task::JoinHandle<()> {
    spawn_inner(sink, rx, tick, LiveState::default(), BTreeMap::new())
}

fn spawn_inner<S: Sink>(
    sink: S,
    mut rx: mpsc::UnboundedReceiver<WriterMsg>,
    tick: Duration,
    live: LiveState,
    history: BTreeMap<String, ServiceHistory>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut w = Writer {
            sink,
            records: BTreeMap::new(),
            generations: BTreeMap::new(),
            pending: Vec::new(),
            dirty: BTreeSet::new(),
            live,
            histories: history
                .into_iter()
                .map(|(name, h)| (Arc::from(name), h))
                .collect(),
            hist_dirty: BTreeSet::new(),
        };
        // First tick one period from now: start-up already writes a heartbeat.
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + tick, tick);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(WriterMsg::Update(u)) => w.update(u),
                    Some(WriterMsg::SetLayout { fresh_start, site, services }) => w.set_layout(fresh_start, site, services),
                    Some(WriterMsg::Shutdown(done)) => {
                        w.shutdown();
                        let _ = done.send(());
                        return;
                    }
                    None => return,
                },
                _ = ticker.tick() => w.heartbeat(),
            }
        }
    })
}

impl<S: Sink> Writer<S> {
    fn update(&mut self, u: BackendUpdate) {
        if self.generations.get(&u.service) != Some(&u.generation) {
            return; // from a task replaced by a reload
        }
        let Some(rec) = self.records.get_mut(&u.service) else {
            return;
        };
        let Some(b) = rec.backends.iter_mut().find(|b| b.id == u.backend_id) else {
            return;
        };
        b.state = u.state;
        b.checked_at = Some(u.checked_at);
        b.reason = u.reason;
        b.latency_us = u.latency_us;
        let reason = b.reason.clone();
        rec.refresh_all_down();
        refresh_pool(rec);
        self.live.set_record(rec);
        self.dirty.insert(u.service.clone());
        if let Some(t) = u.transition {
            let entry = HistoryEntry {
                at: u.checked_at,
                from: t.from,
                to: t.to,
                reason,
            };
            self.histories
                .entry(u.service.clone())
                .or_default()
                .push(&u.backend_id, entry);
            self.hist_dirty.insert(u.service.clone());
            self.flush(Vec::new());
        }
    }

    fn set_layout(
        &mut self,
        fresh_start: bool,
        site: Option<String>,
        services: Vec<ServiceLayout>,
    ) {
        let mut ops = Vec::new();
        if fresh_start {
            self.records.clear();
            self.generations.clear();
            self.pending.clear();
            self.dirty.clear();
            ops.push(Op::Put(
                KEY_SCHEMA_VERSION.into(),
                SCHEMA_VERSION.to_string(),
            ));
            ops.push(Op::Put(KEY_STARTED_AT.into(), unix_ms().to_string()));
            ops.push(Op::Put(KEY_HEARTBEAT.into(), unix_ms().to_string()));
            ops.push(Op::DeletePrefix(SERVICE_KEY_PREFIX.into()));
            // History is pruned key by key below, never by prefix: a key the
            // caller could not read is left alone rather than lost.
        }
        let wanted: BTreeSet<Arc<str>> = services.iter().map(|s| s.name.clone()).collect();
        let removed: Vec<Arc<str>> = self
            .records
            .keys()
            .filter(|n| !wanted.contains(*n))
            .cloned()
            .collect();
        for name in removed {
            self.records.remove(&name);
            self.generations.remove(&name);
            self.dirty.remove(&name);
            ops.push(Op::Delete(service_key(&name)));
        }
        let unconfigured: Vec<Arc<str>> = self
            .histories
            .keys()
            .filter(|n| !wanted.contains(*n))
            .cloned()
            .collect();
        for name in unconfigured {
            self.histories.remove(&name);
            self.hist_dirty.remove(&name);
            ops.push(Op::Delete(hist_key(&name)));
        }
        let mut live = std::collections::HashMap::new();
        for s in services {
            // Config parsing normalizes names; the live view relies on it.
            debug_assert_eq!(&*s.name, s.name.to_ascii_lowercase());
            let old = if s.reset {
                None
            } else {
                self.records.remove(&s.name)
            };
            if let Some(h) = self.histories.get_mut(&s.name) {
                let pruned = h.retain(|id| s.backends.iter().any(|b| b.id == id));
                // At start, also rewrite (or delete, when empty) every loaded key.
                if pruned || fresh_start {
                    self.hist_dirty.insert(s.name.clone());
                }
            }
            let backends = s
                .backends
                .iter()
                .map(|b| {
                    let mut rec = old
                        .as_ref()
                        .and_then(|o| o.backends.iter().find(|ob| ob.id == b.id).cloned())
                        .unwrap_or_else(|| fresh_backend(b));
                    // Health state carries over; weight and site follow the config.
                    rec.weight = b.weight;
                    rec.site = b.site.clone();
                    rec
                })
                .collect();
            let mut rec = ServiceRecord {
                name: s.name.to_string(),
                on_all_down: s.on_all_down,
                balance: s.balance,
                all_down: true,
                backends,
                pool: old.and_then(|o| o.pool),
            };
            rec.refresh_all_down();
            refresh_pool(&mut rec);
            live.insert(s.name.to_string(), LiveService::new(rec.clone(), s.dns_ttl));
            self.generations.insert(s.name.clone(), s.generation);
            self.records.insert(s.name.clone(), rec);
            self.dirty.insert(s.name);
        }
        ops.push(match &site {
            Some(s) => Op::Put(KEY_SITE.into(), s.clone()),
            None => Op::Delete(KEY_SITE.into()),
        });
        self.live.replace(live, site);
        self.flush(ops);
    }

    fn heartbeat(&mut self) {
        self.flush(vec![Op::Put(KEY_HEARTBEAT.into(), unix_ms().to_string())]);
    }

    fn shutdown(&mut self) {
        self.flush(vec![Op::Put(KEY_HEARTBEAT.into(), "0".into())]);
    }

    /// Commit pending ops, `extra`, and every dirty service record in one transaction.
    fn flush(&mut self, extra: Vec<Op>) {
        let mut ops = std::mem::take(&mut self.pending);
        ops.extend(extra);
        for name in &self.dirty {
            if let Some(rec) = self.records.get(name) {
                let json = serde_json::to_string(rec).expect("service record serializes");
                ops.push(Op::Put(service_key(name), json));
            }
        }
        for name in &self.hist_dirty {
            match self.histories.get(name).filter(|h| !h.is_empty()) {
                Some(h) => {
                    let json = serde_json::to_string(h).expect("history serializes");
                    ops.push(Op::Put(hist_key(name), json));
                }
                None => ops.push(Op::Delete(hist_key(name))),
            }
        }
        if ops.is_empty() {
            return;
        }
        match self.sink.commit(&ops) {
            Ok(()) => {
                self.dirty.clear();
                self.hist_dirty.clear();
            }
            Err(e) => {
                tracing::error!("state store commit failed, retrying at next heartbeat: {e}");
                // Keep everything except service and history puts (dirty records and
                // histories are re-rendered) and stale heartbeats (a fresh one is
                // written next tick).
                self.pending = ops
                    .into_iter()
                    .filter(|op| {
                        !matches!(op, Op::Put(k, _) if k.starts_with(SERVICE_KEY_PREFIX)
                            || k.starts_with(HIST_KEY_PREFIX)
                            || k == KEY_HEARTBEAT)
                    })
                    .collect();
            }
        }
    }
}

/// Recompute `pool` of a `fastest` service from its backends and the
/// previous pool (ids no longer configured drop out); other services have none.
fn refresh_pool(rec: &mut ServiceRecord) {
    rec.pool = (rec.balance == Balance::Fastest)
        .then(|| fast_pool(&rec.backends, rec.pool.as_deref().unwrap_or_default()));
}

fn fresh_backend(b: &Backend) -> BackendRecord {
    BackendRecord {
        id: b.id.clone(),
        ip: b.addr.ip().to_string(),
        port: b.addr.port(),
        weight: b.weight,
        site: b.site.clone(),
        state: HealthState::Unknown,
        checked_at: None,
        reason: None,
        latency_us: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Transition;
    use crate::store::tests::cfg;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn be(id: &str) -> Backend {
        Backend::new(id.parse().unwrap())
    }

    fn layout(name: &str, ids: &[&str], generation: u64, reset: bool) -> ServiceLayout {
        ServiceLayout {
            name: name.into(),
            on_all_down: OnAllDown::Open,
            balance: Balance::RoundRobin,
            dns_ttl: None,
            backends: ids.iter().map(|i| be(i)).collect(),
            generation,
            reset,
        }
    }

    fn upd(service: &str, id: &str, state: HealthState, transition: bool) -> WriterMsg {
        WriterMsg::Update(BackendUpdate {
            service: service.into(),
            generation: 0,
            backend_id: id.into(),
            state,
            checked_at: 1,
            reason: None,
            latency_us: None,
            transition: transition.then_some(Transition {
                from: HealthState::Unknown,
                to: state,
            }),
        })
    }

    struct Failing {
        inner: Arc<Store>,
        fail_next: Arc<AtomicUsize>,
    }
    impl Sink for Failing {
        fn commit(&mut self, ops: &[Op]) -> Result<(), String> {
            if self.fail_next.load(Ordering::SeqCst) > 0 {
                self.fail_next.fetch_sub(1, Ordering::SeqCst);
                return Err("MDB_MAP_FULL: Environment mapsize limit reached".into());
            }
            self.inner.commit(ops)
        }
    }

    fn record(store: &Store, name: &str) -> ServiceRecord {
        serde_json::from_str(
            &store
                .get(&service_key(name))
                .unwrap()
                .expect("record exists"),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn host_site_written_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_with_tick(store.clone(), rx, Duration::from_secs(3600));
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: Some("dc1".into()),
            services: vec![layout("a", &["10.0.0.1:80"], 0, true)],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(store.get(KEY_SITE).unwrap().as_deref(), Some("dc1"));
        tx.send(WriterMsg::SetLayout {
            fresh_start: false,
            site: None,
            services: vec![layout("a", &["10.0.0.1:80"], 0, false)],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(store.get(KEY_SITE).unwrap(), None);
    }

    #[tokio::test]
    async fn weight_and_site_written_and_reloaded() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_with_tick(store.clone(), rx, Duration::from_secs(3600));
        let mut l = layout("api.svc.emfor", &["10.0.0.11:8080"], 0, true);
        l.balance = Balance::Weighted;
        l.backends[0].weight = 3;
        l.backends[0].site = Some("dc1".into());
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![l.clone()],
        })
        .unwrap();
        tx.send(upd(
            "api.svc.emfor",
            "10.0.0.11:8080",
            HealthState::Healthy,
            true,
        ))
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let json = store.get(&service_key("api.svc.emfor")).unwrap().unwrap();
        assert!(json.contains(r#""weight":3,"site":"dc1""#), "{json}");
        // Reload changing the weight: the record follows, health carries over.
        l.reset = false;
        l.backends[0].weight = 4;
        tx.send(WriterMsg::SetLayout {
            fresh_start: false,
            site: None,
            services: vec![l],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let rec = record(&store, "api.svc.emfor");
        assert_eq!(rec.backends[0].weight, 4);
        assert_eq!(rec.backends[0].state, HealthState::Healthy);
    }

    #[tokio::test]
    async fn heartbeat_advances() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        spawn(store.clone(), rx);
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![layout("a", &["10.0.0.1:80"], 0, true)],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let hb1: u64 = store.get(KEY_HEARTBEAT).unwrap().unwrap().parse().unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let hb2: u64 = store.get(KEY_HEARTBEAT).unwrap().unwrap().parse().unwrap();
        assert!(hb2 > hb1, "{hb1} -> {hb2}");
        assert_eq!(store.get(KEY_SCHEMA_VERSION).unwrap().as_deref(), Some("1"));
        assert!(store.get(KEY_STARTED_AT).unwrap().is_some());
    }

    #[tokio::test]
    async fn state_change_committed_before_next_tick() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_with_tick(store.clone(), rx, Duration::from_secs(3600));
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![layout("a", &["10.0.0.1:80"], 0, true)],
        })
        .unwrap();
        // Without a transition, the change waits for the (hour-long) tick.
        tx.send(upd("a", "10.0.0.1:80", HealthState::Unknown, false))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(record(&store, "a").backends[0].checked_at, None);
        tx.send(upd("a", "10.0.0.1:80", HealthState::Healthy, true))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let rec = record(&store, "a");
        assert_eq!(rec.backends[0].state, HealthState::Healthy);
        assert!(!rec.all_down);
    }

    #[tokio::test]
    async fn all_down_computation_table() {
        use HealthState::*;
        // Rows of the "all_down computation" example.
        let rows: &[(&[HealthState], bool)] = &[
            (&[Healthy, Unhealthy], false),
            (&[Unhealthy, Unhealthy], true),
            (&[Unknown, Unhealthy], true),
            (&[Unknown, Healthy], false),
        ];
        for (states, expected) in rows {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
            let (tx, rx) = mpsc::unbounded_channel();
            spawn_with_tick(store.clone(), rx, Duration::from_secs(3600));
            let ids = ["10.0.0.1:80", "10.0.0.2:80"];
            tx.send(WriterMsg::SetLayout {
                fresh_start: true,
                site: None,
                services: vec![layout("a", &ids, 0, true)],
            })
            .unwrap();
            for (id, st) in ids.iter().zip(states.iter()) {
                tx.send(upd("a", id, *st, true)).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(record(&store, "a").all_down, *expected, "{states:?}");
        }
    }

    fn trans(
        service: &str,
        id: &str,
        from: HealthState,
        to: HealthState,
        at: u64,
        reason: Option<&str>,
    ) -> WriterMsg {
        WriterMsg::Update(BackendUpdate {
            service: service.into(),
            generation: 0,
            backend_id: id.into(),
            state: to,
            checked_at: at,
            reason: reason.map(str::to_owned),
            latency_us: None,
            transition: Some(Transition { from, to }),
        })
    }

    fn history(store: &Store, name: &str) -> Option<ServiceHistory> {
        store
            .get(&hist_key(name))
            .unwrap()
            .map(|v| serde_json::from_str(&v).unwrap())
    }

    fn ats(h: &ServiceHistory, id: &str) -> Vec<u64> {
        h.backends[id].iter().map(|e| e.at).collect()
    }

    /// Records every committed batch, then commits it to the store.
    struct Recording {
        inner: Arc<Store>,
        commits: Arc<std::sync::Mutex<Vec<Vec<Op>>>>,
    }
    impl Sink for Recording {
        fn commit(&mut self, ops: &[Op]) -> Result<(), String> {
            self.commits.lock().unwrap().push(ops.to_vec());
            self.inner.commit(ops)
        }
    }

    #[tokio::test]
    async fn transition_recorded_with_its_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let commits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Recording {
            inner: store.clone(),
            commits: commits.clone(),
        };
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_with_tick(sink, rx, Duration::from_secs(3600));
        let ids = ["10.0.0.11:8080", "10.0.0.12:8080"];
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![layout("api.svc.emfor", &ids, 0, true)],
        })
        .unwrap();
        tx.send(trans(
            "api.svc.emfor",
            "10.0.0.12:8080",
            HealthState::Healthy,
            HealthState::Unhealthy,
            1762905600123,
            Some("connect: Connection refused"),
        ))
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let last = commits.lock().unwrap().last().cloned().unwrap();
        let hist = last
            .iter()
            .find_map(|op| match op {
                Op::Put(k, v) if k == "hist/api.svc.emfor" => Some(v.clone()),
                _ => None,
            })
            .expect("history written");
        assert!(
            last.iter()
                .any(|op| matches!(op, Op::Put(k, _) if k == "svc/api.svc.emfor")),
            "same commit as the record"
        );
        assert_eq!(
            hist,
            r#"{"backends":{"10.0.0.12:8080":[{"at":1762905600123,"from":"healthy","to":"unhealthy","reason":"connect: Connection refused"}]}}"#
        );

        // An update without a transition, and a reset, add no entry.
        let n = commits.lock().unwrap().len();
        tx.send(upd(
            "api.svc.emfor",
            "10.0.0.12:8080",
            HealthState::Unhealthy,
            false,
        ))
        .unwrap();
        tx.send(WriterMsg::SetLayout {
            fresh_start: false,
            site: None,
            services: vec![layout("api.svc.emfor", &ids, 0, true)],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let commits = commits.lock().unwrap();
        assert!(
            commits[n..].iter().flatten().all(
                |op| !matches!(op, Op::Put(k, _) | Op::Delete(k) if k.starts_with(HIST_KEY_PREFIX))
            ),
            "{:?}",
            &commits[n..]
        );
        let h = history(&store, "api.svc.emfor").unwrap();
        assert_eq!(ats(&h, "10.0.0.12:8080"), [1762905600123]);
    }

    #[tokio::test]
    async fn failed_commit_retries_latest_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let fail_next = Arc::new(AtomicUsize::new(0));
        let sink = Failing {
            inner: store.clone(),
            fail_next: fail_next.clone(),
        };
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_with_tick(sink, rx, Duration::from_millis(300));
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![layout("a", &["10.0.0.1:80"], 0, true)],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        fail_next.store(1, Ordering::SeqCst);
        let (h, u) = (HealthState::Healthy, HealthState::Unhealthy);
        tx.send(trans("a", "10.0.0.1:80", HealthState::Unknown, h, 10, None))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(history(&store, "a"), None, "first commit failed");
        tx.send(trans("a", "10.0.0.1:80", h, u, 20, Some("refused")))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(ats(&history(&store, "a").unwrap(), "10.0.0.1:80"), [10, 20]);
        fail_next.store(1, Ordering::SeqCst);
        tx.send(trans("a", "10.0.0.1:80", u, h, 30, None)).unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            ats(&history(&store, "a").unwrap(), "10.0.0.1:80"),
            [10, 20, 30],
            "retried at next tick"
        );
    }

    /// History the caller could not read is not loaded and stays in the
    /// store; history loaded as empty (it did not parse) is deleted.
    #[tokio::test]
    async fn unread_history_left_alone_and_empty_history_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let mut kept = ServiceHistory::default();
        kept.push(
            "10.0.0.11:8080",
            HistoryEntry {
                at: 1,
                from: HealthState::Healthy,
                to: HealthState::Unhealthy,
                reason: None,
            },
        );
        let kept_json = serde_json::to_string(&kept).unwrap();
        commit_to_store(
            &store,
            &[
                Op::Put("hist/api.svc.emfor".into(), kept_json.clone()),
                Op::Put("hist/bad.svc.emfor".into(), "not json".into()),
                Op::Put("hist/gone.svc.emfor".into(), "not json".into()),
            ],
        )
        .unwrap();
        // As cli loads them: api unread, the two others did not parse.
        let loaded = BTreeMap::from([
            ("bad.svc.emfor".to_owned(), ServiceHistory::default()),
            ("gone.svc.emfor".to_owned(), ServiceHistory::default()),
        ]);
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_live(store.clone(), rx, LiveState::default(), loaded);
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![
                layout("api.svc.emfor", &["10.0.0.11:8080"], 0, true),
                layout("bad.svc.emfor", &["10.0.0.2:80"], 0, true),
            ],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(store.get("hist/api.svc.emfor").unwrap(), Some(kept_json));
        assert_eq!(store.get("hist/bad.svc.emfor").unwrap(), None);
        assert_eq!(store.get("hist/gone.svc.emfor").unwrap(), None);
    }

    #[tokio::test]
    async fn history_kept_and_pruned_on_start_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let entry = |at| HistoryEntry {
            at,
            from: HealthState::Healthy,
            to: HealthState::Unhealthy,
            reason: None,
        };
        let mut api = ServiceHistory::default();
        for at in 1..=3 {
            api.push("10.0.0.11:8080", entry(at));
        }
        api.push("10.0.0.99:8080", entry(4));
        let mut old = ServiceHistory::default();
        old.push("10.0.0.1:80", entry(5));
        // As left by the previous run.
        commit_to_store(
            &store,
            &[
                Op::Put(
                    "hist/api.svc.emfor".into(),
                    serde_json::to_string(&api).unwrap(),
                ),
                Op::Put(
                    "hist/old.svc.emfor".into(),
                    serde_json::to_string(&old).unwrap(),
                ),
            ],
        )
        .unwrap();
        let loaded = BTreeMap::from([
            ("api.svc.emfor".to_owned(), api),
            ("old.svc.emfor".to_owned(), old),
        ]);
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_live(store.clone(), rx, LiveState::default(), loaded);
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![
                layout(
                    "api.svc.emfor",
                    &["10.0.0.11:8080", "10.0.0.12:8080"],
                    0,
                    true,
                ),
                layout("db.svc.emfor", &["10.0.0.2:5432"], 0, true),
            ],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(history(&store, "old.svc.emfor"), None);
        let h = history(&store, "api.svc.emfor").unwrap();
        assert_eq!(h.backends.keys().collect::<Vec<_>>(), ["10.0.0.11:8080"]);
        assert_eq!(ats(&h, "10.0.0.11:8080"), [1, 2, 3]);
        assert!(
            record(&store, "api.svc.emfor")
                .backends
                .iter()
                .all(|b| b.state == HealthState::Unknown)
        );

        // Reload: a removed backend's entries and a removed service's key go.
        tx.send(trans(
            "db.svc.emfor",
            "10.0.0.2:5432",
            HealthState::Unknown,
            HealthState::Healthy,
            6,
            None,
        ))
        .unwrap();
        tx.send(trans(
            "api.svc.emfor",
            "10.0.0.12:8080",
            HealthState::Unknown,
            HealthState::Healthy,
            7,
            None,
        ))
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(history(&store, "db.svc.emfor").is_some());
        tx.send(WriterMsg::SetLayout {
            fresh_start: false,
            site: None,
            services: vec![layout("api.svc.emfor", &["10.0.0.12:8080"], 0, false)],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(history(&store, "db.svc.emfor"), None);
        let h = history(&store, "api.svc.emfor").unwrap();
        assert_eq!(h.backends.keys().collect::<Vec<_>>(), ["10.0.0.12:8080"]);

        // Removing the last backend with history deletes the key.
        tx.send(WriterMsg::SetLayout {
            fresh_start: false,
            site: None,
            services: vec![layout("api.svc.emfor", &["10.0.0.13:8080"], 0, false)],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(history(&store, "api.svc.emfor"), None);
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn failed_commit_is_logged_and_retried() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let fail_next = Arc::new(AtomicUsize::new(0));
        let sink = Failing {
            inner: store.clone(),
            fail_next: fail_next.clone(),
        };
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_with_tick(sink, rx, Duration::from_millis(300));
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![layout("a", &["10.0.0.1:80"], 0, true)],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        fail_next.store(1, Ordering::SeqCst);
        tx.send(upd("a", "10.0.0.1:80", HealthState::Healthy, true))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            record(&store, "a").backends[0].state,
            HealthState::Unknown,
            "first commit failed"
        );
        assert!(logs_contain("state store commit failed"));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            record(&store, "a").backends[0].state,
            HealthState::Healthy,
            "retried at next tick"
        );
    }

    #[tokio::test]
    async fn live_state_updated_before_commit() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let live = LiveState::default();
        let (tx, rx) = mpsc::unbounded_channel();
        // Hour-long tick: an update without a transition is not committed.
        spawn_inner(
            store.clone(),
            rx,
            Duration::from_secs(3600),
            live.clone(),
            BTreeMap::new(),
        );
        let mut l = layout("a", &["10.0.0.1:80"], 0, true);
        l.dns_ttl = Some(5);
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![l],
        })
        .unwrap();
        tx.send(upd("a", "10.0.0.1:80", HealthState::Healthy, false))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        {
            let v = live.read();
            let s = &v.services["a"];
            assert_eq!(s.record.backends[0].state, HealthState::Healthy);
            assert!(!s.record.all_down);
            assert_eq!(s.dns_ttl, Some(5));
            assert_eq!(record(&store, "a").backends[0].state, HealthState::Unknown);
        }
        tx.send(WriterMsg::SetLayout {
            fresh_start: false,
            site: None,
            services: vec![],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            live.read().services.is_empty(),
            "removed service leaves live state"
        );
    }

    #[tokio::test]
    async fn stale_generation_dropped_and_shutdown_zeroes_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        let h = spawn_with_tick(store.clone(), rx, Duration::from_secs(3600));
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![layout("a", &["10.0.0.1:80"], 5, true)],
        })
        .unwrap();
        tx.send(upd("a", "10.0.0.1:80", HealthState::Healthy, true))
            .unwrap(); // generation 0 != 5
        let (done_tx, done_rx) = oneshot::channel();
        tx.send(WriterMsg::Shutdown(done_tx)).unwrap();
        done_rx.await.unwrap();
        h.await.unwrap();
        assert_eq!(record(&store, "a").backends[0].state, HealthState::Unknown);
        assert_eq!(store.get(KEY_HEARTBEAT).unwrap().as_deref(), Some("0"));
    }

    fn updl(id: &str, state: HealthState, latency_us: Option<u64>, transition: bool) -> WriterMsg {
        let WriterMsg::Update(mut u) = upd("f", id, state, transition) else {
            unreachable!()
        };
        u.latency_us = latency_us;
        WriterMsg::Update(u)
    }

    fn fastest(ids: &[&str]) -> ServiceLayout {
        let mut l = layout("f", ids, 0, true);
        l.balance = Balance::Fastest;
        l
    }

    /// Writer with an hour-long tick; returns the sender and the live state.
    fn start_fastest(
        store: Arc<Store>,
        ids: &[&str],
    ) -> (mpsc::UnboundedSender<WriterMsg>, LiveState) {
        let live = LiveState::default();
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_inner(
            store,
            rx,
            Duration::from_secs(3600),
            live.clone(),
            BTreeMap::new(),
        );
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![fastest(ids)],
        })
        .unwrap();
        (tx, live)
    }

    fn live_pool(live: &LiveState) -> Vec<String> {
        live.read().services["f"]
            .record
            .pool
            .clone()
            .expect("fastest has a pool")
    }

    #[tokio::test]
    async fn nearly_equal_backends_are_fast() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let ids = ["10.0.0.1:80", "10.0.0.2:80", "10.0.0.3:80"];
        let (tx, _live) = start_fastest(store.clone(), &ids);
        for (id, l) in ids.iter().zip([2000, 2800, 5000]) {
            tx.send(updl(id, HealthState::Healthy, Some(l), true))
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            record(&store, "f").pool.unwrap(),
            ["10.0.0.1:80", "10.0.0.2:80"]
        );
    }

    #[tokio::test]
    async fn fast_backend_near_the_edge_stays_fast() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let ids = ["10.0.0.1:80", "10.0.0.2:80"];
        let (tx, live) = start_fastest(store.clone(), &ids);
        tx.send(updl(ids[0], HealthState::Healthy, Some(2000), true))
            .unwrap();
        tx.send(updl(ids[1], HealthState::Healthy, Some(2900), true))
            .unwrap();
        tx.send(updl(ids[1], HealthState::Healthy, Some(3500), false))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(live_pool(&live), ids);
        // A reload keeping the backends keeps the membership.
        let mut l = fastest(&ids);
        l.reset = false;
        tx.send(WriterMsg::SetLayout {
            fresh_start: false,
            site: None,
            services: vec![l],
        })
        .unwrap();
        tx.send(updl(ids[1], HealthState::Healthy, Some(4000), false))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(live_pool(&live), ids);
        tx.send(updl(ids[1], HealthState::Healthy, Some(4001), false))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(live_pool(&live), [ids[0]]);
        // Pool-only changes wait for the heartbeat; the store still has both.
        assert_eq!(record(&store, "f").pool.unwrap(), ids);
    }

    #[tokio::test]
    async fn missing_latency_is_not_fast() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let ids = ["10.0.0.1:80", "10.0.0.2:80"];
        let (tx, live) = start_fastest(store, &ids);
        tx.send(updl(ids[0], HealthState::Healthy, Some(900), true))
            .unwrap();
        tx.send(updl(ids[1], HealthState::Healthy, None, true))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(live_pool(&live), [ids[0]]);
    }

    #[tokio::test]
    async fn pool_written_for_a_fastest_service() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_with_tick(store.clone(), rx, Duration::from_secs(3600));
        let mut f = layout("api.svc.emfor", &["10.0.0.11:8080"], 0, true);
        f.balance = Balance::Fastest;
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![f, layout("rr.svc.emfor", &["10.0.0.12:8080"], 0, true)],
        })
        .unwrap();
        let send = |state, latency_us, from| {
            tx.send(WriterMsg::Update(BackendUpdate {
                service: "api.svc.emfor".into(),
                generation: 0,
                backend_id: "10.0.0.11:8080".into(),
                state,
                checked_at: 1,
                reason: None,
                latency_us,
                transition: Some(Transition { from, to: state }),
            }))
            .unwrap();
        };
        send(HealthState::Healthy, Some(1500), HealthState::Unknown);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let json = store.get(&service_key("api.svc.emfor")).unwrap().unwrap();
        assert!(json.ends_with(r#""pool":["10.0.0.11:8080"]}"#), "{json}");
        send(HealthState::Unhealthy, None, HealthState::Healthy);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let json = store.get(&service_key("api.svc.emfor")).unwrap().unwrap();
        assert!(json.ends_with(r#""pool":[]}"#), "{json}");
        let rr = store.get(&service_key("rr.svc.emfor")).unwrap().unwrap();
        assert!(!rr.contains("pool"), "{rr}");
    }
}
