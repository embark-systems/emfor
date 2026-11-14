//! Peer counters and member liveness, as `/metrics` reads them.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Why a datagram was dropped, in the order the checks run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reject {
    Malformed,
    Auth,
    Sender,
    Site,
    Stale,
    Replay,
}

impl Reject {
    pub const ALL: [Reject; 6] = [
        Reject::Malformed,
        Reject::Auth,
        Reject::Sender,
        Reject::Site,
        Reject::Stale,
        Reject::Replay,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Reject::Malformed => "malformed",
            Reject::Auth => "auth",
            Reject::Sender => "sender",
            Reject::Site => "site",
            Reject::Stale => "stale",
            Reject::Replay => "replay",
        }
    }

    pub(super) fn index(self) -> usize {
        self as usize
    }
}

/// Counters and member liveness, read by `/metrics`.
#[derive(Debug, Default)]
pub struct PeerStats {
    pub(super) rejected: [AtomicU64; 6],
    pub(super) sent: AtomicU64,
    pub(super) received: AtomicU64,
    pub(super) results_sent: AtomicU64,
    pub(super) results_received: AtomicU64,
    pub(super) disagreements: AtomicU64,
    pub(super) checks_skipped: AtomicU64,
    up: Mutex<BTreeMap<SocketAddr, bool>>,
}

impl PeerStats {
    pub fn rejected(&self, r: Reject) -> u64 {
        self.rejected[r.index()].load(Ordering::Relaxed)
    }

    /// `down` messages sent, one per receiving member.
    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// `down` messages accepted.
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// `result` messages sent, one per receiving member.
    pub fn results_sent(&self) -> u64 {
        self.results_sent.load(Ordering::Relaxed)
    }

    /// `result` messages accepted.
    pub fn results_received(&self) -> u64 {
        self.results_received.load(Ordering::Relaxed)
    }

    /// Backends that are a disagreement, as of the last heartbeat round.
    pub fn disagreements(&self) -> u64 {
        self.disagreements.load(Ordering::Relaxed)
    }

    /// Check times decided by votes alone, as of the last heartbeat round.
    pub fn checks_skipped(&self) -> u64 {
        self.checks_skipped.load(Ordering::Relaxed)
    }

    /// Set the skipped-check counter.
    #[cfg(test)]
    pub fn set_checks_skipped(&self, n: u64) {
        self.checks_skipped.store(n, Ordering::Relaxed);
    }

    /// Set the `result` counters and the disagreement gauge.
    #[cfg(test)]
    pub fn set_results(&self, sent: u64, received: u64, disagreements: u64) {
        self.results_sent.store(sent, Ordering::Relaxed);
        self.results_received.store(received, Ordering::Relaxed);
        self.disagreements.store(disagreements, Ordering::Relaxed);
    }

    /// Every member other than this daemon, with whether it is live.
    pub fn up(&self) -> Vec<(SocketAddr, bool)> {
        let up = self.up.lock().unwrap_or_else(|e| e.into_inner());
        up.iter().map(|(a, l)| (*a, *l)).collect()
    }

    pub(super) fn set_up(&self, members: impl Iterator<Item = (SocketAddr, bool)>) {
        let mut up = self.up.lock().unwrap_or_else(|e| e.into_inner());
        *up = members.collect();
    }
}
