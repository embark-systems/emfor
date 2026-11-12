//! In-memory copy of every service record, kept by the writer and read by the
//! DNS server, so DNS answers follow health changes without reading LMDB and
//! do not depend on LMDB commits succeeding.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock, RwLockReadGuard};

use emfor_format::ServiceRecord;

use crate::select::Family;

/// Round-robin positions of one service, per family. Shared by every DNS
/// client of the daemon, kept across reloads, dropped with the service.
#[derive(Debug, Default)]
pub struct RoundRobin {
    v4: AtomicU64,
    v6: AtomicU64,
}

impl RoundRobin {
    pub fn for_family(&self, family: Family) -> &AtomicU64 {
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
}

/// Shared handle; cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct LiveState(Arc<RwLock<LiveView>>);

impl LiveState {
    pub fn read(&self) -> RwLockReadGuard<'_, LiveView> {
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Replace every service, keeping the round-robin position of services
    /// that stay.
    pub(crate) fn replace(&self, mut services: HashMap<String, LiveService>) {
        let mut v = self.0.write().unwrap_or_else(|e| e.into_inner());
        for (name, s) in services.iter_mut() {
            if let Some(old) = v.services.get(name) {
                s.round_robin = old.round_robin.clone();
            }
        }
        v.services = services;
        v.ready = true;
    }

    pub(crate) fn set_record(&self, record: &ServiceRecord) {
        let mut v = self.0.write().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = v.services.get_mut(&record.name) {
            s.record = record.clone();
        }
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
            balance: Balance::RoundRobin,
            all_down: true,
            backends: vec![],
        };
        (name.to_string(), LiveService::new(record, None))
    }

    #[test]
    fn replace_keeps_counters_of_kept_services_and_drops_removed() {
        let live = LiveState::default();
        assert!(!live.read().ready);
        live.replace([svc("a"), svc("b")].into_iter().collect());
        assert!(live.read().ready);
        live.read().services["a"]
            .round_robin
            .for_family(Family::V4)
            .store(7, Ordering::Relaxed);
        live.replace([svc("a")].into_iter().collect());
        let v = live.read();
        assert_eq!(v.services.len(), 1);
        assert_eq!(
            v.services["a"]
                .round_robin
                .for_family(Family::V4)
                .load(Ordering::Relaxed),
            7
        );
    }
}
