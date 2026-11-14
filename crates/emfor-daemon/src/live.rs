//! In-memory copy of every service record, kept by the writer and read by the
//! DNS server, so DNS answers follow health changes without reading LMDB and
//! do not depend on LMDB commits succeeding.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};

use emfor_format::{HealthState, ServiceRecord};

use crate::peers::PeerStats;
use crate::schedule::CheckOutcome;
use crate::select::Family;
use crate::state::Transition;

/// Smooth weighted round-robin state for one candidate list.
#[derive(Debug, Default)]
pub struct Swrr {
    /// Candidates (address and weight) the state belongs to.
    pub candidates: Vec<(IpAddr, u32)>,
    /// Current value per candidate.
    pub current: Vec<i64>,
}

/// Choice state of one service and family.
#[derive(Debug, Default)]
pub struct FamilyChoice {
    /// Round-robin position (`round-robin`, `sticky`, `locality`).
    pub counter: AtomicU64,
    /// `weighted` state, reset when the candidate list changes.
    pub swrr: Mutex<Swrr>,
}

/// Choice state of one service, per family. Shared by every DNS client of
/// the daemon, kept across reloads, dropped with the service.
#[derive(Debug, Default)]
pub struct RoundRobin {
    v4: FamilyChoice,
    v6: FamilyChoice,
    /// SRV answer rotation, independent of the A and AAAA state.
    pub srv: AtomicU64,
}

impl RoundRobin {
    pub fn for_family(&self, family: Family) -> &FamilyChoice {
        match family {
            Family::V4 => &self.v4,
            Family::V6 => &self.v6,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LiveService {
    pub record: ServiceRecord,
    /// Per-service override of `[dns] ttl`, in seconds.
    pub dns_ttl: Option<u32>,
    pub round_robin: Arc<RoundRobin>,
}

impl LiveService {
    pub fn new(record: ServiceRecord, dns_ttl: Option<u32>) -> LiveService {
        LiveService {
            record,
            dns_ttl,
            round_robin: Arc::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LiveView {
    /// False until the writer has applied the first layout; until then the
    /// daemon cannot tell which names exist.
    pub ready: bool,
    /// Services by name. Names are already normalized (lowercase, no
    /// trailing dot) by config parsing, so lookups lowercase the query only.
    pub services: HashMap<String, LiveService>,
    /// Top-level `site` of the config, for `locality`.
    pub site: Option<String>,
    /// Check and transition counts per (service, backend id) since the
    /// backend appeared in the running daemon.
    pub counters: HashMap<(String, String), BackendCounters>,
    /// Failed state store commits since the daemon started.
    pub commit_failures: u64,
    /// `meta/started_at` of this run, Unix milliseconds.
    pub started_at_ms: u64,
    /// Unix milliseconds of the last successful store commit; 0 before one.
    pub last_commit_ms: u64,
    /// Bytes of the store in use and the map size, after the last commit.
    pub store_usage: Option<(u64, u64)>,
    /// Plugins of the applied config, by name.
    pub plugins: Vec<(String, Arc<PluginStatus>)>,
    /// Counters and member liveness of the peer group; `None` without
    /// `[peers]`.
    pub peers: Option<Arc<PeerStats>>,
}

/// Kept up to date by a plugin's host, read by `/metrics`.
#[derive(Debug, Default)]
pub struct PluginStatus {
    /// The plugin process is running and answered the handshake.
    pub up: AtomicBool,
    /// Times the plugin was started again after it exited.
    pub restarts: AtomicU64,
}

/// Counts for one backend, indexed by [`BackendCounters::outcome_index`]
/// and [`BackendCounters::state_index`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackendCounters {
    /// Checks with result up, down, error.
    pub checks: [u64; 3],
    /// Transitions to unknown, healthy, unhealthy.
    pub transitions: [u64; 3],
}

impl BackendCounters {
    pub const OUTCOMES: [(CheckOutcome, &'static str); 3] = [
        (CheckOutcome::Up, "up"),
        (CheckOutcome::Down, "down"),
        (CheckOutcome::Error, "error"),
    ];
    pub const STATES: [HealthState; 3] = [
        HealthState::Unknown,
        HealthState::Healthy,
        HealthState::Unhealthy,
    ];

    pub fn outcome_index(o: CheckOutcome) -> usize {
        match o {
            CheckOutcome::Up => 0,
            CheckOutcome::Down => 1,
            CheckOutcome::Error => 2,
        }
    }

    pub fn state_index(s: HealthState) -> usize {
        match s {
            HealthState::Unknown => 0,
            HealthState::Healthy => 1,
            // The daemon's own state machine never produces a state it does
            // not know; were it to, it would count as unhealthy, as readers
            // treat it.
            HealthState::Unhealthy | HealthState::Unrecognized => 2,
        }
    }
}

/// Shared handle; cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct LiveState(Arc<RwLock<LiveView>>);

impl LiveState {
    pub fn read(&self) -> RwLockReadGuard<'_, LiveView> {
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Replace every service, keeping the round-robin position of services
    /// that stay and the counters of backends that stay; new backends start
    /// at 0 and removed ones lose theirs.
    pub(crate) fn replace(&self, mut services: HashMap<String, LiveService>, site: Option<String>) {
        let mut v = self.0.write().unwrap_or_else(|e| e.into_inner());
        for (name, s) in services.iter_mut() {
            if let Some(old) = v.services.get(name) {
                s.round_robin = old.round_robin.clone();
            }
        }
        let mut counters = HashMap::new();
        for (name, s) in &services {
            for b in &s.record.backends {
                let key = (name.clone(), b.id.clone());
                let c = v.counters.get(&key).copied().unwrap_or_default();
                counters.insert(key, c);
            }
        }
        v.counters = counters;
        v.services = services;
        v.site = site;
        v.ready = true;
    }

    /// Take over what one change of backend `index` of `record` touched (that
    /// backend, `all_down` and `pool`), counting the check's outcome (when
    /// it was a check) and the transition under the same lock. Only those
    /// parts are copied: copying the whole record on every check would cost
    /// time in the number of backends, all of it holding the lock that DNS
    /// answers wait on.
    pub(crate) fn update_backend(
        &self,
        record: &ServiceRecord,
        index: usize,
        outcome: Option<CheckOutcome>,
        transition: Option<Transition>,
    ) {
        let Some(backend) = record.backends.get(index) else {
            return;
        };
        let mut v = self.0.write().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = v.services.get_mut(&record.name) {
            let live = &mut s.record;
            match live.backends.get_mut(index) {
                Some(b) if b.id == backend.id => b.clone_from(backend),
                // The writer and the live view take the same layout, so
                // this does not happen; copy all rather than go astray.
                _ => live.clone_from(record),
            }
            live.all_down = record.all_down;
            if live.pool != record.pool {
                live.pool.clone_from(&record.pool);
            }
        }
        let key = (record.name.clone(), backend.id.clone());
        if let Some(c) = v.counters.get_mut(&key) {
            if let Some(outcome) = outcome {
                c.checks[BackendCounters::outcome_index(outcome)] += 1;
            }
            if let Some(t) = transition {
                c.transitions[BackendCounters::state_index(t.to)] += 1;
            }
        }
    }

    /// Count the outcome of a check that changed nothing else.
    pub(crate) fn count_check(&self, service: &Arc<str>, backend_id: &str, outcome: CheckOutcome) {
        let mut v = self.0.write().unwrap_or_else(|e| e.into_inner());
        let key = (service.to_string(), backend_id.to_owned());
        if let Some(c) = v.counters.get_mut(&key) {
            c.checks[BackendCounters::outcome_index(outcome)] += 1;
        }
    }

    pub(crate) fn count_commit_failure(&self) {
        let mut v = self.0.write().unwrap_or_else(|e| e.into_inner());
        v.commit_failures += 1;
    }

    pub(crate) fn record_commit(&self, at_ms: u64, usage: Option<(u64, u64)>) {
        let mut v = self.0.write().unwrap_or_else(|e| e.into_inner());
        v.last_commit_ms = at_ms;
        if usage.is_some() {
            v.store_usage = usage;
        }
    }

    pub fn set_plugins(&self, plugins: Vec<(String, Arc<PluginStatus>)>) {
        let mut v = self.0.write().unwrap_or_else(|e| e.into_inner());
        v.plugins = plugins;
    }

    pub fn set_peers(&self, peers: Option<Arc<PeerStats>>) {
        let mut v = self.0.write().unwrap_or_else(|e| e.into_inner());
        v.peers = peers;
    }

    pub(crate) fn set_started_at(&self, ms: u64) {
        let mut v = self.0.write().unwrap_or_else(|e| e.into_inner());
        v.started_at_ms = ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emfor_format::{Balance, OnAllDown};
    use std::sync::atomic::Ordering;

    fn svc(name: &str) -> (String, LiveService) {
        let record = ServiceRecord {
            name: name.into(),
            on_all_down: OnAllDown::Open,
            pool: None,
            balance: Balance::RoundRobin,
            all_down: true,
            slow_start_ms: 0,
            backends: vec![],
        };
        (name.to_string(), LiveService::new(record, None))
    }

    /// One backend's change reaches the live record, with `all_down` and
    /// `pool`; the other backends are left as they are.
    #[test]
    fn update_backend_copies_the_changed_parts() {
        use emfor_format::BackendRecord;
        let backend = |id: &str| BackendRecord {
            id: id.into(),
            ip: "10.0.0.1".into(),
            port: 80,
            weight: 1,
            site: None,
            backup: false,
            state: HealthState::Unknown,
            checked_at: None,
            reason: None,
            latency_us: None,
            recovered_at: None,
        };
        let live = LiveState::default();
        let (name, mut s) = svc("a");
        s.record.balance = Balance::Fastest;
        s.record.backends = vec![backend("x"), backend("y")];
        let mut record = s.record.clone();
        live.replace([(name, s)].into_iter().collect(), None);
        record.backends[1].state = HealthState::Healthy;
        record.backends[1].latency_us = Some(5);
        record.backends[0].reason = Some("not taken over".into());
        record.all_down = false;
        record.pool = Some(vec!["y".into()]);
        let t = Transition {
            from: HealthState::Unknown,
            to: HealthState::Healthy,
        };
        live.update_backend(&record, 1, Some(CheckOutcome::Up), Some(t));
        live.update_backend(&record, 1, None, None);
        let v = live.read();
        let r = &v.services["a"].record;
        assert_eq!(r.backends[1], record.backends[1]);
        assert_eq!(r.backends[0].reason, None);
        assert!(!r.all_down);
        assert_eq!(r.pool.as_deref(), Some(&["y".to_owned()][..]));
        let c = v.counters[&("a".to_owned(), "y".to_owned())];
        assert_eq!(c.checks, [1, 0, 0]);
        assert_eq!(c.transitions, [0, 1, 0]);
    }

    #[test]
    fn replace_keeps_counters_of_kept_services_and_drops_removed() {
        let live = LiveState::default();
        assert!(!live.read().ready);
        live.replace([svc("a"), svc("b")].into_iter().collect(), None);
        assert!(live.read().ready);
        live.read().services["a"]
            .round_robin
            .for_family(Family::V4)
            .counter
            .store(7, Ordering::Relaxed);
        live.read().services["a"]
            .round_robin
            .srv
            .store(5, Ordering::Relaxed);
        live.replace([svc("a")].into_iter().collect(), None);
        let v = live.read();
        assert_eq!(v.services.len(), 1);
        assert_eq!(
            v.services["a"]
                .round_robin
                .for_family(Family::V4)
                .counter
                .load(Ordering::Relaxed),
            7
        );
        assert_eq!(v.services["a"].round_robin.srv.load(Ordering::Relaxed), 5);
    }
}
