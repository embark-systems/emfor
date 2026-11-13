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
    BackendRecord, Balance, HIST_KEY_PREFIX, HealthState, HistoryEntry, KEY_DNS_LISTEN,
    KEY_HEARTBEAT, KEY_METRICS_LISTEN, KEY_PEERS, KEY_SCHEMA_VERSION, KEY_SITE, KEY_STARTED_AT,
    OnAllDown, SCHEMA_VERSION, SERVICE_KEY_PREFIX, ServiceHistory, ServiceRecord, hist_key,
    service_key,
};
use tokio::sync::{mpsc, oneshot};
use tokio::time::MissedTickBehavior;

use crate::config::Backend;
#[cfg(test)]
use crate::live::BackendCounters;
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

    /// Free reader slots of processes that died during a lookup
    /// ([`Store::clear_stale_readers`]); returns how many were freed.
    fn clear_stale_readers(&mut self) -> Result<usize, String>;

    /// [`Store::usage`], for `/metrics`; `None` when unknown.
    fn usage(&self) -> Option<(u64, u64)> {
        None
    }
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

    fn clear_stale_readers(&mut self) -> Result<usize, String> {
        Store::log_stale_readers(self).map_err(|e| e.to_string())
    }

    fn usage(&self) -> Option<(u64, u64)> {
        Some(Store::usage(self))
    }
}

/// Shared store, so tests (and future readers in-process) can read while the writer runs.
impl Sink for Arc<Store> {
    fn commit(&mut self, ops: &[Op]) -> Result<(), String> {
        commit_to_store(self, ops)
    }

    fn clear_stale_readers(&mut self) -> Result<usize, String> {
        self.log_stale_readers().map_err(|e| e.to_string())
    }

    fn usage(&self) -> Option<(u64, u64)> {
        Some(Store::usage(self))
    }
}

/// Heartbeats between checks for reader slots left by dead processes:
/// about once a minute at the 1-second heartbeat. A failed commit checks
/// at once, since a full store is what those slots lead to.
const READER_CHECK_TICKS: u32 = 60;

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
    /// New value of `meta/peers`, written with the next commit.
    PeerState(String),
    /// Flush, write `meta/heartbeat = "0"`, and stop.
    Shutdown(oneshot::Sender<()>),
    /// Answered once every earlier message is handled (and committed, when
    /// handling commits), so tests wait for the writer instead of sleeping.
    #[cfg(test)]
    Barrier(oneshot::Sender<()>),
}

struct Writer<S: Sink> {
    /// Taken while a commit runs on a blocking thread.
    sink: Option<S>,
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
    /// `meta/dns_listen`, `meta/metrics_listen` and `meta/peers` with their
    /// JSON values, written at a fresh start; a key is deleted when its
    /// value is `None`.
    listen: [(&'static str, Option<String>); 3],
    /// Updated before every commit attempt.
    live: LiveState,
    /// Heartbeats since the last reader slot check.
    ticks_since_reader_check: u32,
}

/// The listen addresses of the running daemon, written to `meta/dns_listen`
/// and `meta/metrics_listen` at a fresh start, and the first value of
/// `meta/peers`. `None`: the config has no such table, and the key is
/// deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListenKeys {
    pub dns: Option<Vec<String>>,
    pub metrics: Option<Vec<String>>,
    /// JSON object for `meta/peers`.
    pub peers: Option<String>,
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
    listen: ListenKeys,
) -> tokio::task::JoinHandle<()> {
    spawn_inner(sink, rx, Duration::from_secs(1), live, history, listen)
}

pub fn spawn_with_tick<S: Sink>(
    sink: S,
    rx: mpsc::UnboundedReceiver<WriterMsg>,
    tick: Duration,
) -> tokio::task::JoinHandle<()> {
    spawn_inner(
        sink,
        rx,
        tick,
        LiveState::default(),
        BTreeMap::new(),
        ListenKeys::default(),
    )
}

fn spawn_inner<S: Sink>(
    sink: S,
    mut rx: mpsc::UnboundedReceiver<WriterMsg>,
    tick: Duration,
    live: LiveState,
    history: BTreeMap<String, ServiceHistory>,
    listen: ListenKeys,
) -> tokio::task::JoinHandle<()> {
    let json = |l: Option<Vec<String>>| {
        l.map(|l| serde_json::to_string(&l).expect("a list of strings always serializes to JSON"))
    };
    let listen = [
        (KEY_DNS_LISTEN, json(listen.dns)),
        (KEY_METRICS_LISTEN, json(listen.metrics)),
        (KEY_PEERS, listen.peers),
    ];
    tokio::spawn(async move {
        let mut w = Writer {
            sink: Some(sink),
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
            listen,
            ticks_since_reader_check: 0,
        };
        // First tick one period from now: start-up already writes a heartbeat.
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + tick, tick);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(WriterMsg::Update(u)) => w.update(u).await,
                    Some(WriterMsg::SetLayout { fresh_start, site, services }) => w.set_layout(fresh_start, site, services).await,
                    Some(WriterMsg::PeerState(json)) => w.peer_state(json),
                    Some(WriterMsg::Shutdown(done)) => {
                        w.shutdown().await;
                        let _ = done.send(());
                        return;
                    }
                    #[cfg(test)]
                    Some(WriterMsg::Barrier(done)) => {
                        let _ = done.send(());
                    }
                    None => return,
                },
                _ = ticker.tick() => w.heartbeat().await,
            }
        }
    })
}

impl<S: Sink> Writer<S> {
    async fn update(&mut self, u: BackendUpdate) {
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
        self.live
            .update_backend(rec, &u.backend_id, u.outcome, u.transition);
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
            self.flush(Vec::new()).await;
        }
    }

    /// Queue the new `meta/peers` value and remember it, so a later fresh
    /// start writes this value and not the one from when the daemon began.
    fn peer_state(&mut self, json: String) {
        for (key, value) in &mut self.listen {
            if *key == KEY_PEERS {
                *value = Some(json.clone());
            }
        }
        self.pending.push(Op::Put(KEY_PEERS.into(), json));
    }

    async fn set_layout(
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
            let started = unix_ms();
            self.live.set_started_at(started);
            ops.push(Op::Put(KEY_STARTED_AT.into(), started.to_string()));
            for (key, value) in &self.listen {
                ops.push(match value {
                    Some(v) => Op::Put((*key).into(), v.clone()),
                    None => Op::Delete((*key).into()),
                });
            }
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
        self.flush(ops).await;
    }

    async fn heartbeat(&mut self) {
        self.ticks_since_reader_check += 1;
        if self.ticks_since_reader_check >= READER_CHECK_TICKS {
            self.clear_stale_readers().await;
        }
        self.flush(vec![Op::Put(KEY_HEARTBEAT.into(), unix_ms().to_string())])
            .await;
    }

    async fn clear_stale_readers(&mut self) {
        self.ticks_since_reader_check = 0;
        // On a blocking thread, like a commit: it takes the reader table's
        // lock in the lock file, which any process using the store can hold.
        let mut sink = self.sink.take().expect(
            "the sink is taken only while a commit or reader check runs and is put back before it returns",
        );
        let (sink, result) = tokio::task::spawn_blocking(move || {
            let result = sink.clear_stale_readers();
            (sink, result)
        })
        .await
        .expect(
            "a store reader check panicked; the writer stops, and the daemon exits with status 70",
        );
        self.sink = Some(sink);
        if let Err(e) = result {
            tracing::error!("cannot check the state store's reader slots: {e}");
        }
    }

    async fn shutdown(&mut self) {
        self.flush(vec![Op::Put(KEY_HEARTBEAT.into(), "0".into())])
            .await;
    }

    /// Commit pending ops, `extra`, and every dirty service record in one transaction.
    async fn flush(&mut self, extra: Vec<Op>) {
        let mut ops = std::mem::take(&mut self.pending);
        ops.extend(extra);
        for name in &self.dirty {
            if let Some(rec) = self.records.get(name) {
                let json = serde_json::to_string(rec).expect("a ServiceRecord serializes: it holds only strings, numbers, booleans and lists");
                ops.push(Op::Put(service_key(name), json));
            }
        }
        for name in &self.hist_dirty {
            match self.histories.get(name).filter(|h| !h.is_empty()) {
                Some(h) => {
                    let json = serde_json::to_string(h)
                        .expect("a ServiceHistory serializes: its maps are keyed by strings");
                    ops.push(Op::Put(hist_key(name), json));
                }
                None => ops.push(Op::Delete(hist_key(name))),
            }
        }
        if ops.is_empty() {
            return;
        }
        // On a blocking thread: a commit can wait for the disk (`[store]
        // sync = true`, or the kernel writing back), and must not hold up a
        // worker that serves DNS, metrics and checks meanwhile.
        let mut sink = self.sink.take().expect(
            "the sink is taken only while a commit or reader check runs and is put back before it returns",
        );
        let (sink, ops, result, usage) = tokio::task::spawn_blocking(move || {
            #[cfg(feature = "test-hooks")]
            commit_panic_hook();
            let result = sink.commit(&ops);
            let usage = result.as_ref().ok().and_then(|()| sink.usage());
            (sink, ops, result, usage)
        })
        .await
        .expect("a store commit panicked; the writer stops, and the daemon exits with status 70");
        self.sink = Some(sink);
        match result {
            Ok(()) => {
                self.dirty.clear();
                self.hist_dirty.clear();
                self.live.record_commit(unix_ms(), usage);
            }
            Err(e) => {
                tracing::error!("state store commit failed, retrying at next heartbeat: {e}");
                self.live.count_commit_failure();
                self.clear_stale_readers().await;
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

/// Fault injection for emfor's own tests, compiled in only with the
/// `test-hooks` feature: a commit panics while the file named by
/// `EMFOR_TEST_COMMIT_PANIC_FILE` exists, so a test can watch the daemon
/// handle a writer that stops.
#[cfg(feature = "test-hooks")]
fn commit_panic_hook() {
    if let Some(path) = std::env::var_os("EMFOR_TEST_COMMIT_PANIC_FILE")
        && std::path::Path::new(&path).exists()
    {
        panic!(
            "test hook: a commit panics while {} exists",
            std::path::Path::new(&path).display()
        );
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

    /// Poll `cond` every 20ms until it holds, failing after 5s: for what
    /// waits on the real-time heartbeat tick.
    async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Wait until the writer has handled every message sent before.
    async fn settled(tx: &mpsc::UnboundedSender<WriterMsg>) {
        let (done, wait) = oneshot::channel();
        tx.send(WriterMsg::Barrier(done)).unwrap();
        wait.await.unwrap();
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
            outcome: crate::schedule::CheckOutcome::Up,
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
        fn clear_stale_readers(&mut self) -> Result<usize, String> {
            self.inner.clear_stale_readers()
        }
    }

    /// Counts reader slot checks; commits fail while `fail` is set.
    struct ReaderChecks {
        checks: Arc<AtomicUsize>,
        fail: Arc<std::sync::atomic::AtomicBool>,
    }
    impl Sink for ReaderChecks {
        fn commit(&mut self, _ops: &[Op]) -> Result<(), String> {
            if self.fail.load(Ordering::SeqCst) {
                return Err("MDB_MAP_FULL: Environment mapsize limit reached".into());
            }
            Ok(())
        }
        fn clear_stale_readers(&mut self) -> Result<usize, String> {
            self.checks.fetch_add(1, Ordering::SeqCst);
            Ok(0)
        }
    }

    /// Records the thread each commit runs on.
    struct Threads(Arc<std::sync::Mutex<Vec<std::thread::ThreadId>>>);
    impl Sink for Threads {
        fn commit(&mut self, _ops: &[Op]) -> Result<(), String> {
            self.0.lock().unwrap().push(std::thread::current().id());
            Ok(())
        }
        fn clear_stale_readers(&mut self) -> Result<usize, String> {
            Ok(0)
        }
    }

    /// `#[tokio::test]` runs every task on the test's own thread, so a
    /// commit made inside the writer task would run there too.
    #[tokio::test]
    async fn commits_run_off_the_async_thread() {
        let threads = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::unbounded_channel();
        let writer = spawn_with_tick(Threads(threads.clone()), rx, Duration::from_secs(3600));
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![layout("a", &["10.0.0.1:80"], 0, true)],
        })
        .unwrap();
        let (done_tx, done_rx) = oneshot::channel();
        tx.send(WriterMsg::Shutdown(done_tx)).unwrap();
        done_rx.await.unwrap();
        writer.await.unwrap();
        let threads = threads.lock().unwrap();
        assert_eq!(threads.len(), 2, "layout and shutdown commits");
        let here = std::thread::current().id();
        assert!(
            threads.iter().all(|t| *t != here),
            "{threads:?} vs {here:?}"
        );
    }

    /// Fails every commit and records the thread of each reader check.
    struct ReaderCheckThreads(Arc<std::sync::Mutex<Vec<std::thread::ThreadId>>>);
    impl Sink for ReaderCheckThreads {
        fn commit(&mut self, _ops: &[Op]) -> Result<(), String> {
            Err("MDB_MAP_FULL: Environment mapsize limit reached".into())
        }
        fn clear_stale_readers(&mut self) -> Result<usize, String> {
            self.0.lock().unwrap().push(std::thread::current().id());
            Ok(0)
        }
    }

    /// Reader checks take a lock other processes can hold, so like commits
    /// they run off the writer's async thread.
    #[tokio::test]
    async fn reader_checks_run_off_the_async_thread() {
        let threads = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::unbounded_channel();
        let writer = spawn_with_tick(
            ReaderCheckThreads(threads.clone()),
            rx,
            Duration::from_secs(3600),
        );
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![layout("a", &["10.0.0.1:80"], 0, true)],
        })
        .unwrap();
        let (done_tx, done_rx) = oneshot::channel();
        tx.send(WriterMsg::Shutdown(done_tx)).unwrap();
        done_rx.await.unwrap();
        writer.await.unwrap();
        let threads = threads.lock().unwrap();
        assert_eq!(threads.len(), 2, "one check per failed commit");
        let here = std::thread::current().id();
        assert!(
            threads.iter().all(|t| *t != here),
            "{threads:?} vs {here:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reader_slots_checked_every_minute_of_heartbeats() {
        let checks = Arc::new(AtomicUsize::new(0));
        let sink = ReaderChecks {
            checks: checks.clone(),
            fail: Arc::default(),
        };
        let (_tx, rx) = mpsc::unbounded_channel();
        spawn_with_tick(sink, rx, Duration::from_secs(1));
        tokio::time::sleep(Duration::from_millis(59_500)).await;
        assert_eq!(checks.load(Ordering::SeqCst), 0, "59 heartbeats");
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(checks.load(Ordering::SeqCst), 1, "60 heartbeats");
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(checks.load(Ordering::SeqCst), 2, "120 heartbeats");
    }

    #[tokio::test(start_paused = true)]
    async fn reader_slots_checked_after_failed_commit() {
        let checks = Arc::new(AtomicUsize::new(0));
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let sink = ReaderChecks {
            checks: checks.clone(),
            fail: fail.clone(),
        };
        let (_tx, rx) = mpsc::unbounded_channel();
        spawn_with_tick(sink, rx, Duration::from_secs(1));
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert_eq!(checks.load(Ordering::SeqCst), 1, "first heartbeat failed");
        fail.store(false, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(59)).await;
        assert_eq!(checks.load(Ordering::SeqCst), 1, "the count restarted");
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(checks.load(Ordering::SeqCst), 2);
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
        settled(&tx).await;
        assert_eq!(store.get(KEY_SITE).unwrap().as_deref(), Some("dc1"));
        tx.send(WriterMsg::SetLayout {
            fresh_start: false,
            site: None,
            services: vec![layout("a", &["10.0.0.1:80"], 0, false)],
        })
        .unwrap();
        settled(&tx).await;
        assert_eq!(store.get(KEY_SITE).unwrap(), None);
    }

    /// State-store "DNS listen addresses in the store" and "Metrics listen
    /// addresses in the store": written at a fresh start, left alone by a
    /// reload, deleted at a start without the table.
    #[tokio::test]
    async fn listen_keys_written_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let listen = ListenKeys {
            dns: Some(vec!["127.0.0.1:5300".into(), "[::1]:5300".into()]),
            metrics: Some(vec!["127.0.0.1:5301".into(), "[::1]:5301".into()]),
            peers: Some(PEERS_AT_START.into()),
        };
        let start = |tx: &mpsc::UnboundedSender<WriterMsg>, fresh_start| {
            tx.send(WriterMsg::SetLayout {
                fresh_start,
                site: None,
                services: vec![layout("a", &["10.0.0.1:80"], 0, fresh_start)],
            })
            .unwrap();
        };
        let keys = |store: &Store| {
            (
                store.get(KEY_DNS_LISTEN).unwrap(),
                store.get(KEY_METRICS_LISTEN).unwrap(),
                store.get(KEY_PEERS).unwrap(),
            )
        };
        let want = (
            Some(r#"["127.0.0.1:5300","[::1]:5300"]"#.to_owned()),
            Some(r#"["127.0.0.1:5301","[::1]:5301"]"#.to_owned()),
            Some(PEERS_AT_START.to_owned()),
        );
        let (tx, rx) = mpsc::unbounded_channel();
        let w = spawn_inner(
            store.clone(),
            rx,
            Duration::from_secs(3600),
            LiveState::default(),
            BTreeMap::new(),
            listen,
        );
        start(&tx, true);
        settled(&tx).await;
        assert_eq!(keys(&store), want);
        start(&tx, false);
        settled(&tx).await;
        assert_eq!(keys(&store), want);
        drop(tx);
        w.await.unwrap();

        // Next run without [dns], [metrics] or [peers]: the earlier values
        // are gone.
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_with_tick(store.clone(), rx, Duration::from_secs(3600));
        start(&tx, true);
        settled(&tx).await;
        assert_eq!(keys(&store), (None, None, None));
    }

    /// State-store "Peer state written at start".
    const PEERS_AT_START: &str = r#"{"listen":"127.0.0.1:5302","members":[{"addr":"127.0.0.1:5303","live":false}],"digest_mismatch":[],"disagreements":[],"services_off":[]}"#;

    /// A new `meta/peers` value is written with the next commit (here the
    /// next heartbeat).
    #[tokio::test]
    async fn peer_state_written_with_the_next_commit() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_inner(
            store.clone(),
            rx,
            Duration::from_millis(50),
            LiveState::default(),
            BTreeMap::new(),
            ListenKeys {
                peers: Some(PEERS_AT_START.into()),
                ..ListenKeys::default()
            },
        );
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![layout("a", &["10.0.0.1:80"], 0, true)],
        })
        .unwrap();
        settled(&tx).await;
        assert_eq!(
            store.get(KEY_PEERS).unwrap().as_deref(),
            Some(PEERS_AT_START)
        );
        let live = PEERS_AT_START.replace("false", "true");
        tx.send(WriterMsg::PeerState(live.clone())).unwrap();
        settled(&tx).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(store.get(KEY_PEERS).unwrap(), Some(live));
    }

    /// A peer state that arrives before the start transaction is not
    /// overwritten by the value from when the daemon began.
    #[tokio::test]
    async fn peer_state_before_start_survives_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_inner(
            store.clone(),
            rx,
            Duration::from_millis(50),
            LiveState::default(),
            BTreeMap::new(),
            ListenKeys {
                peers: Some(PEERS_AT_START.into()),
                ..ListenKeys::default()
            },
        );
        let live = PEERS_AT_START.replace("false", "true");
        tx.send(WriterMsg::PeerState(live.clone())).unwrap();
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![layout("a", &["10.0.0.1:80"], 0, true)],
        })
        .unwrap();
        settled(&tx).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(store.get(KEY_PEERS).unwrap(), Some(live));
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
        settled(&tx).await;
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
        settled(&tx).await;
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
        settled(&tx).await;
        let hb1: u64 = store.get(KEY_HEARTBEAT).unwrap().unwrap().parse().unwrap();
        let hb = || -> u64 { store.get(KEY_HEARTBEAT).unwrap().unwrap().parse().unwrap() };
        eventually("heartbeat advances", || hb() > hb1).await;
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
        settled(&tx).await;
        assert_eq!(record(&store, "a").backends[0].checked_at, None);
        tx.send(upd("a", "10.0.0.1:80", HealthState::Healthy, true))
            .unwrap();
        settled(&tx).await;
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
            settled(&tx).await;
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
            outcome: if to == HealthState::Healthy {
                crate::schedule::CheckOutcome::Up
            } else {
                crate::schedule::CheckOutcome::Down
            },
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
        fn clear_stale_readers(&mut self) -> Result<usize, String> {
            self.inner.clear_stale_readers()
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
        settled(&tx).await;
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
        settled(&tx).await;
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
        settled(&tx).await;
        fail_next.store(1, Ordering::SeqCst);
        let (h, u) = (HealthState::Healthy, HealthState::Unhealthy);
        tx.send(trans("a", "10.0.0.1:80", HealthState::Unknown, h, 10, None))
            .unwrap();
        settled(&tx).await;
        assert_eq!(history(&store, "a"), None, "first commit failed");
        tx.send(trans("a", "10.0.0.1:80", h, u, 20, Some("refused")))
            .unwrap();
        settled(&tx).await;
        assert_eq!(ats(&history(&store, "a").unwrap(), "10.0.0.1:80"), [10, 20]);
        fail_next.store(1, Ordering::SeqCst);
        tx.send(trans("a", "10.0.0.1:80", u, h, 30, None)).unwrap();
        eventually("retried at next tick", || {
            ats(&history(&store, "a").unwrap(), "10.0.0.1:80") == [10, 20, 30]
        })
        .await;
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
        spawn_live(
            store.clone(),
            rx,
            LiveState::default(),
            loaded,
            ListenKeys::default(),
        );
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![
                layout("api.svc.emfor", &["10.0.0.11:8080"], 0, true),
                layout("bad.svc.emfor", &["10.0.0.2:80"], 0, true),
            ],
        })
        .unwrap();
        settled(&tx).await;
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
        spawn_live(
            store.clone(),
            rx,
            LiveState::default(),
            loaded,
            ListenKeys::default(),
        );
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
        settled(&tx).await;
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
        settled(&tx).await;
        assert!(history(&store, "db.svc.emfor").is_some());
        tx.send(WriterMsg::SetLayout {
            fresh_start: false,
            site: None,
            services: vec![layout("api.svc.emfor", &["10.0.0.12:8080"], 0, false)],
        })
        .unwrap();
        settled(&tx).await;
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
        settled(&tx).await;
        assert_eq!(history(&store, "api.svc.emfor"), None);
    }

    fn checked(
        service: &str,
        id: &str,
        generation: u64,
        outcome: crate::schedule::CheckOutcome,
        transition: Option<(HealthState, HealthState)>,
    ) -> WriterMsg {
        let state = transition.map_or(HealthState::Unknown, |t| t.1);
        WriterMsg::Update(BackendUpdate {
            service: service.into(),
            generation,
            backend_id: id.into(),
            state,
            checked_at: 1,
            reason: None,
            latency_us: None,
            outcome,
            transition: transition.map(|(from, to)| Transition { from, to }),
        })
    }

    fn counters(live: &LiveState, service: &str, id: &str) -> Option<BackendCounters> {
        live.read()
            .counters
            .get(&(service.to_owned(), id.to_owned()))
            .copied()
    }

    /// Spec metrics-endpoint "Check and transition counters".
    #[tokio::test]
    async fn counts_checks_and_transitions() {
        use crate::schedule::CheckOutcome::{Down, Up};
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let live = LiveState::default();
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_inner(
            store,
            rx,
            Duration::from_secs(3600),
            live.clone(),
            BTreeMap::new(),
            ListenKeys::default(),
        );
        let id = "10.0.0.11:8080";
        let svc = "api.svc.emfor";
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![layout(svc, &[id], 1, true)],
        })
        .unwrap();
        let (h, u) = (HealthState::Healthy, HealthState::Unhealthy);
        for m in [
            checked(svc, id, 1, Up, None),
            checked(svc, id, 1, Up, Some((HealthState::Unknown, h))),
            checked(svc, id, 1, Down, None),
            checked(svc, id, 1, Down, None),
            checked(svc, id, 1, Down, Some((h, u))),
            // From a task replaced by a reload: not counted.
            checked(svc, id, 0, Up, Some((u, h))),
        ] {
            tx.send(m).unwrap();
        }
        settled(&tx).await;
        let c = counters(&live, svc, id).unwrap();
        assert_eq!(c.checks, [2, 3, 0], "up, down, error");
        assert_eq!(c.transitions, [0, 1, 1], "unknown, healthy, unhealthy");
        assert!(live.read().started_at_ms > 0);

        // A reset (check changed) keeps the counters.
        tx.send(WriterMsg::SetLayout {
            fresh_start: false,
            site: None,
            services: vec![layout(svc, &[id], 2, true)],
        })
        .unwrap();
        settled(&tx).await;
        assert_eq!(counters(&live, svc, id).unwrap().checks, [2, 3, 0]);

        // Removed, then added back: from 0.
        tx.send(WriterMsg::SetLayout {
            fresh_start: false,
            site: None,
            services: vec![layout(svc, &["10.0.0.12:8080"], 3, false)],
        })
        .unwrap();
        settled(&tx).await;
        assert_eq!(counters(&live, svc, id), None);
        assert_eq!(
            counters(&live, svc, "10.0.0.12:8080"),
            Some(BackendCounters::default())
        );
        tx.send(WriterMsg::SetLayout {
            fresh_start: false,
            site: None,
            services: vec![layout(svc, &[id, "10.0.0.12:8080"], 4, false)],
        })
        .unwrap();
        settled(&tx).await;
        assert_eq!(counters(&live, svc, id), Some(BackendCounters::default()));
    }

    /// Spec metrics-endpoint "Daemon metrics", scenario "Store commit failure
    /// counted".
    #[tokio::test]
    async fn commit_failures_counted() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let fail_next = Arc::new(AtomicUsize::new(0));
        let sink = Failing {
            inner: store,
            fail_next: fail_next.clone(),
        };
        let live = LiveState::default();
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_inner(
            sink,
            rx,
            Duration::from_secs(3600),
            live.clone(),
            BTreeMap::new(),
            ListenKeys::default(),
        );
        tx.send(WriterMsg::SetLayout {
            fresh_start: true,
            site: None,
            services: vec![layout("a", &["10.0.0.1:80"], 0, true)],
        })
        .unwrap();
        settled(&tx).await;
        assert_eq!(live.read().commit_failures, 0);
        fail_next.store(1, Ordering::SeqCst);
        tx.send(upd("a", "10.0.0.1:80", HealthState::Healthy, true))
            .unwrap();
        settled(&tx).await;
        assert_eq!(live.read().commit_failures, 1);
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
        settled(&tx).await;
        fail_next.store(1, Ordering::SeqCst);
        tx.send(upd("a", "10.0.0.1:80", HealthState::Healthy, true))
            .unwrap();
        settled(&tx).await;
        assert_eq!(
            record(&store, "a").backends[0].state,
            HealthState::Unknown,
            "first commit failed"
        );
        assert!(logs_contain("state store commit failed"));
        eventually("retried at next tick", || {
            record(&store, "a").backends[0].state == HealthState::Healthy
        })
        .await;
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
            ListenKeys::default(),
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
        settled(&tx).await;
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
        settled(&tx).await;
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
            ListenKeys::default(),
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
        settled(&tx).await;
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
        settled(&tx).await;
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
        settled(&tx).await;
        assert_eq!(live_pool(&live), ids);
        tx.send(updl(ids[1], HealthState::Healthy, Some(4001), false))
            .unwrap();
        settled(&tx).await;
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
        settled(&tx).await;
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
                outcome: crate::schedule::CheckOutcome::Up,
                transition: Some(Transition { from, to: state }),
            }))
            .unwrap();
        };
        send(HealthState::Healthy, Some(1500), HealthState::Unknown);
        settled(&tx).await;
        let json = store.get(&service_key("api.svc.emfor")).unwrap().unwrap();
        assert!(json.ends_with(r#""pool":["10.0.0.11:8080"]}"#), "{json}");
        send(HealthState::Unhealthy, None, HealthState::Healthy);
        settled(&tx).await;
        let json = store.get(&service_key("api.svc.emfor")).unwrap().unwrap();
        assert!(json.ends_with(r#""pool":[]}"#), "{json}");
        let rr = store.get(&service_key("rr.svc.emfor")).unwrap().unwrap();
        assert!(!rr.contains("pool"), "{rr}");
    }
}
