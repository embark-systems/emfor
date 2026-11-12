//! The single writer task for the LMDB store.
//!
//! All store writes go through one task fed by one channel, so layout changes
//! (start, reload) and check results are applied in the order they were sent.
//! A health state change is committed immediately; `checked_at` / `reason`
//! changes and the heartbeat are committed once per second. A failed commit is
//! logged and retried at the next tick.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use emfor_format::{
    BackendRecord, Balance, HealthState, KEY_HEARTBEAT, KEY_SCHEMA_VERSION, KEY_STARTED_AT,
    OnAllDown, SCHEMA_VERSION, SERVICE_KEY_PREFIX, ServiceRecord, service_key,
};
use tokio::sync::{mpsc, oneshot};
use tokio::time::MissedTickBehavior;

use crate::config::Backend;
use crate::live::{LiveService, LiveState};
use crate::schedule::{BackendUpdate, unix_ms};
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

/// Like [`spawn`], also keeping `live` in line with the records.
pub fn spawn_live<S: Sink>(
    sink: S,
    rx: mpsc::UnboundedReceiver<WriterMsg>,
    live: LiveState,
) -> tokio::task::JoinHandle<()> {
    spawn_inner(sink, rx, Duration::from_secs(1), live)
}

pub fn spawn_with_tick<S: Sink>(
    sink: S,
    rx: mpsc::UnboundedReceiver<WriterMsg>,
    tick: Duration,
) -> tokio::task::JoinHandle<()> {
    spawn_inner(sink, rx, tick, LiveState::default())
}

fn spawn_inner<S: Sink>(
    sink: S,
    mut rx: mpsc::UnboundedReceiver<WriterMsg>,
    tick: Duration,
    live: LiveState,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut w = Writer {
            sink,
            records: BTreeMap::new(),
            generations: BTreeMap::new(),
            pending: Vec::new(),
            dirty: BTreeSet::new(),
            live,
        };
        // First tick one period from now: start-up already writes a heartbeat.
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + tick, tick);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(WriterMsg::Update(u)) => w.update(u),
                    Some(WriterMsg::SetLayout { fresh_start, services }) => w.set_layout(fresh_start, services),
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
        rec.refresh_all_down();
        self.live.set_record(rec);
        self.dirty.insert(u.service.clone());
        if u.transition.is_some() {
            self.flush(Vec::new());
        }
    }

    fn set_layout(&mut self, fresh_start: bool, services: Vec<ServiceLayout>) {
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
        let mut live = std::collections::HashMap::new();
        for s in services {
            // Config parsing normalizes names; the live view relies on it.
            debug_assert_eq!(&*s.name, s.name.to_ascii_lowercase());
            let old = if s.reset {
                None
            } else {
                self.records.remove(&s.name)
            };
            let backends = s
                .backends
                .iter()
                .map(|b| {
                    old.as_ref()
                        .and_then(|o| o.backends.iter().find(|ob| ob.id == b.id).cloned())
                        .unwrap_or_else(|| fresh_backend(b))
                })
                .collect();
            let mut rec = ServiceRecord {
                name: s.name.to_string(),
                on_all_down: s.on_all_down,
                balance: s.balance,
                all_down: true,
                backends,
            };
            rec.refresh_all_down();
            live.insert(s.name.to_string(), LiveService::new(rec.clone(), s.dns_ttl));
            self.generations.insert(s.name.clone(), s.generation);
            self.records.insert(s.name.clone(), rec);
            self.dirty.insert(s.name);
        }
        self.live.replace(live);
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
        if ops.is_empty() {
            return;
        }
        match self.sink.commit(&ops) {
            Ok(()) => self.dirty.clear(),
            Err(e) => {
                tracing::error!("state store commit failed, retrying at next heartbeat: {e}");
                // Keep everything except service puts (dirty records are re-rendered) and
                // stale heartbeats (a fresh one is written next tick).
                self.pending = ops
                    .into_iter()
                    .filter(|op| !matches!(op, Op::Put(k, _) if k.starts_with(SERVICE_KEY_PREFIX) || k == KEY_HEARTBEAT))
                    .collect();
            }
        }
    }
}

fn fresh_backend(b: &Backend) -> BackendRecord {
    BackendRecord {
        id: b.id.clone(),
        ip: b.addr.ip().to_string(),
        port: b.addr.port(),
        state: HealthState::Unknown,
        checked_at: None,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Transition;
    use crate::store::tests::cfg;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn be(id: &str) -> Backend {
        Backend {
            id: id.into(),
            addr: id.parse().unwrap(),
        }
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
    async fn heartbeat_advances() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        spawn(store.clone(), rx);
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
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
        spawn_inner(store.clone(), rx, Duration::from_secs(3600), live.clone());
        let mut l = layout("a", &["10.0.0.1:80"], 0, true);
        l.dns_ttl = Some(5);
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
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
}
