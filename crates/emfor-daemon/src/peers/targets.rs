//! The backend tasks peers can reach: each service's check definition
//! digest and its backends' recheck channels, kept by the supervisor.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};

use tokio::sync::Notify;

use super::wire::Down;

/// A peer's report that a backend is down, handed to that backend's task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerReport {
    pub from: SocketAddr,
    pub reason: String,
}

/// "Recheck now" channel of one backend task. A report sent while the task
/// is checking is kept, and its next wait ends at once.
#[derive(Debug, Default)]
pub struct Recheck {
    notify: Notify,
    report: Mutex<Option<PeerReport>>,
}

impl Recheck {
    pub fn send(&self, report: PeerReport) {
        *self.report.lock().unwrap_or_else(|e| e.into_inner()) = Some(report);
        self.notify.notify_one();
    }

    /// Wait until a report is sent.
    pub async fn notified(&self) {
        self.notify.notified().await;
    }

    /// The report sent last, if not taken yet.
    pub fn take(&self) -> Option<PeerReport> {
        self.report.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// The backends of one service that peers can ask to recheck.
#[derive(Debug, Clone, Default)]
pub struct ServiceTargets {
    pub digest: String,
    pub backends: HashMap<String, Arc<Recheck>>,
}

/// Every running backend task's [`Recheck`], by service and backend id;
/// kept by the supervisor, read when a `down` message arrives.
#[derive(Debug, Clone, Default)]
pub struct Targets(pub(super) Arc<RwLock<HashMap<String, ServiceTargets>>>);

impl Targets {
    pub fn replace(&self, services: HashMap<String, ServiceTargets>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = services;
    }

    /// `f` of what this daemon has for `service`; `None` when it has no
    /// such service. Every lookup of a service and its digest goes
    /// through here.
    fn service<R>(&self, service: &str, f: impl FnOnce(&ServiceTargets) -> R) -> Option<R> {
        let map = self.0.read().unwrap_or_else(|e| e.into_inner());
        map.get(service).map(f)
    }

    /// This daemon's digest of `service`.
    pub fn digest(&self, service: &str) -> Option<String> {
        self.service(service, |s| s.digest.clone())
    }

    /// What this daemon has for the `service`, `digest` and `backend` a
    /// message names; `None` when it has no such service.
    pub(super) fn lookup(&self, service: &str, digest: &str, backend: &str) -> Option<Lookup> {
        self.service(service, |s| Lookup {
            same_digest: s.digest == digest,
            recheck: s.backends.get(backend).cloned(),
        })
    }

    /// Hand a `down` report to its backend task. `Err` says why it was
    /// ignored.
    pub fn deliver(&self, down: &Down, from: SocketAddr) -> Result<(), &'static str> {
        let found = self
            .lookup(&down.service, &down.digest, &down.backend)
            .ok_or("no such service here")?;
        if !found.same_digest {
            return Err(DIGEST_DIFFERS);
        }
        let r = found.recheck.ok_or("no such backend here")?;
        r.send(PeerReport {
            from,
            reason: down.reason.clone(),
        });
        Ok(())
    }
}

/// What [`Targets::lookup`] found for a service this daemon has.
#[derive(Debug)]
pub(super) struct Lookup {
    /// The message's digest is this daemon's digest of the service.
    pub(super) same_digest: bool,
    /// The backend's recheck channel, when the service has that backend.
    pub(super) recheck: Option<Arc<Recheck>>,
}

/// What [`Targets::deliver`] says when the digest differs.
pub(super) const DIGEST_DIFFERS: &str = "check definition differs";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peers::test_support::*;
    use crate::peers::wire::Kind;

    #[test]
    fn deliver_checks_service_digest_and_backend() {
        let Kind::Down(d) = down("refused") else {
            unreachable!("down() builds a down kind")
        };
        let (t, r) = targets(&d.digest);
        assert_eq!(t.deliver(&d, addr(B)), Ok(()));
        assert_eq!(
            r.take(),
            Some(PeerReport {
                from: addr(B),
                reason: "refused".into()
            })
        );
        let (t, r) = targets("ffffffffffffffffffffffffffffffff");
        assert_eq!(t.deliver(&d, addr(B)), Err("check definition differs"));
        assert_eq!(r.take(), None);
        let other = Down {
            backend: "10.0.0.12:8080".into(),
            ..d.clone()
        };
        assert!(t.deliver(&other, addr(B)).is_err());
    }

    /// One lookup answers for the digest and the backend together.
    #[test]
    fn lookup_compares_digest_and_finds_backend() {
        let (t, r) = targets(DIGEST);
        let found = t
            .lookup("api.svc.emfor", DIGEST, "10.0.0.11:8080")
            .expect("the service is there");
        assert!(found.same_digest);
        assert!(found.recheck.is_some_and(|f| Arc::ptr_eq(&f, &r)));
        let found = t
            .lookup("api.svc.emfor", &"f".repeat(32), "10.0.0.12:8080")
            .expect("the service is there");
        assert!(!found.same_digest);
        assert!(found.recheck.is_none());
        assert!(
            t.lookup("other.svc.emfor", DIGEST, "10.0.0.11:8080")
                .is_none()
        );
        assert_eq!(t.digest("api.svc.emfor").as_deref(), Some(DIGEST));
        assert_eq!(t.digest("other.svc.emfor"), None);
    }
}
