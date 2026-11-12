//! In-memory copy of every service record, kept by the writer and read by the
//! DNS server, so DNS answers follow health changes without reading LMDB and
//! do not depend on LMDB commits succeeding.

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard};

use emfor_format::ServiceRecord;

#[derive(Debug, Clone)]
pub struct LiveService {
    pub record: ServiceRecord,
    /// Per-service override of `[dns] ttl`, in seconds.
    pub dns_ttl: Option<u32>,
}

/// Services by normalized (lowercase) name.
#[derive(Debug, Clone, Default)]
pub struct LiveView {
    pub services: HashMap<String, LiveService>,
}

/// Shared handle; cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct LiveState(Arc<RwLock<LiveView>>);

impl LiveState {
    pub fn read(&self) -> RwLockReadGuard<'_, LiveView> {
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn replace(&self, view: LiveView) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = view;
    }

    pub(crate) fn set_record(&self, record: &ServiceRecord) {
        let mut v = self.0.write().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = v.services.get_mut(&record.name) {
            s.record = record.clone();
        }
    }
}
