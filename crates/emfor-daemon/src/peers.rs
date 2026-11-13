//! Peer group (spec peer-group): emfor daemons of one site that list each
//! other in `[peers]` exchange authenticated UDP messages. Members send a
//! heartbeat every second, and when a backend of theirs turns unhealthy they
//! tell the others, which then recheck it at once. A message never changes a
//! backend's state by itself; every daemon decides from its own checks.
//!
//! One message is one datagram: `EMP1`, a 32-byte HMAC-SHA256 tag over
//! `EMP1` and the payload, then the payload, a JSON object.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use hmac::{Hmac, KeyInit, Mac};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::net::UdpSocket;
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::config::{PeersConfig, read_secret};
use crate::quorum::Quorum;
use crate::schedule::unix_ms;
use crate::writer::WriterMsg;

/// First four bytes of every message: magic and format version 1.
pub const MAGIC: &[u8; 4] = b"EMP1";
const TAG_LEN: usize = 32;
const HEADER_LEN: usize = MAGIC.len() + TAG_LEN;
/// Largest datagram sent or accepted; small enough not to be fragmented.
pub const MAX_DATAGRAM: usize = 1200;
/// Longest `reason` of a `down` message, in bytes of UTF-8.
pub const MAX_REASON_BYTES: usize = 256;
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(1);
/// A member is live while a message from it was accepted this recently.
pub const LIVE_WINDOW: Duration = Duration::from_secs(3);
/// Largest accepted difference between a message's `ts` and our clock.
pub const MAX_SKEW_MS: u64 = 5000;
/// How long messages are still signed with a replaced secret, so members
/// whose file has not changed yet keep accepting them.
pub const SIGN_PREVIOUS_FOR: Duration = Duration::from_secs(60);
/// How long a replaced secret is still accepted: twice
/// [`SIGN_PREVIOUS_FOR`], so that when every member changes its file within
/// that time, no message between any two of them is dropped.
pub const ACCEPT_PREVIOUS_FOR: Duration = Duration::from_secs(120);
/// At most one warning per source address and reason in this time.
const WARN_EVERY: Duration = Duration::from_secs(60);
/// Rate-limit entries kept before old ones are dropped.
const MAX_WARN_ENTRIES: usize = 1024;

/// One peer message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub from: SocketAddr,
    pub site: String,
    pub boot: u64,
    pub seq: u64,
    /// Unix milliseconds when sent.
    pub ts: u64,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Heartbeat,
    Down(Down),
    Result(ResultMsg),
    /// A kind this version does not know (a newer member): it counts for
    /// liveness only.
    Unknown(String),
}

/// A backend of `service` turned unhealthy at the sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Down {
    pub service: String,
    pub digest: String,
    pub backend: String,
    pub reason: String,
}

/// An assigned checker's own result of one check of a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultMsg {
    pub service: String,
    pub digest: String,
    pub backend: String,
    pub outcome: Outcome,
    /// The sender's `interval` of the service, in milliseconds; positive.
    pub interval_ms: u64,
}

/// A vote: what an assigned checker's own check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Up,
    /// Down, with the check's reason.
    Down(String),
}

/// The JSON payload as sent.
#[derive(Serialize, Deserialize)]
struct Wire {
    from: String,
    site: String,
    boot: String,
    seq: u64,
    ts: u64,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interval_ms: Option<u64>,
}

/// The longest prefix of `s` of at most `max` bytes that ends on a
/// character boundary.
pub fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn mac(key: &[u8]) -> Hmac<Sha256> {
    Hmac::<Sha256>::new_from_slice(key)
        .expect("HMAC accepts keys of any length (RFC 2104), so new_from_slice never fails")
}

/// The datagram for `msg`, signed with `key`. A reason (of a `down` or
/// `result` message) is cut to [`MAX_REASON_BYTES`], and further when the
/// datagram would otherwise exceed [`MAX_DATAGRAM`].
pub fn encode(msg: &Message, key: &[u8]) -> Vec<u8> {
    let mut wire = Wire {
        from: msg.from.to_string(),
        site: msg.site.clone(),
        boot: format!("{:016x}", msg.boot),
        seq: msg.seq,
        ts: msg.ts,
        kind: String::new(),
        service: None,
        digest: None,
        backend: None,
        reason: None,
        outcome: None,
        interval_ms: None,
    };
    match &msg.kind {
        Kind::Heartbeat => wire.kind = "heartbeat".into(),
        Kind::Unknown(k) => wire.kind = k.clone(),
        Kind::Down(d) => {
            wire.kind = "down".into();
            wire.service = Some(d.service.clone());
            wire.digest = Some(d.digest.clone());
            wire.backend = Some(d.backend.clone());
            wire.reason = Some(truncate_utf8(&d.reason, MAX_REASON_BYTES).to_string());
        }
        Kind::Result(r) => {
            wire.kind = "result".into();
            wire.service = Some(r.service.clone());
            wire.digest = Some(r.digest.clone());
            wire.backend = Some(r.backend.clone());
            wire.interval_ms = Some(r.interval_ms);
            match &r.outcome {
                Outcome::Up => wire.outcome = Some("up".into()),
                Outcome::Down(reason) => {
                    wire.outcome = Some("down".into());
                    wire.reason = Some(truncate_utf8(reason, MAX_REASON_BYTES).to_string());
                }
            }
        }
    }
    let mut payload = serde_json::to_vec(&wire)
        .expect("a struct of strings and integers always serializes to JSON");
    // Names at their longest and a reason of escaped characters can still
    // overflow one datagram: shorten the reason until it fits. Each round
    // removes at least as many bytes as the payload is over.
    while HEADER_LEN + payload.len() > MAX_DATAGRAM
        && let Some(reason) = wire.reason.as_mut()
        && !reason.is_empty()
    {
        let over = HEADER_LEN + payload.len() - MAX_DATAGRAM;
        let keep = truncate_utf8(reason, reason.len().saturating_sub(over)).len();
        reason.truncate(keep);
        payload = serde_json::to_vec(&wire)
            .expect("a struct of strings and integers always serializes to JSON");
    }
    let mut m = mac(key);
    m.update(MAGIC);
    m.update(&payload);
    let tag = m.finalize().into_bytes();
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&tag);
    out.extend_from_slice(&payload);
    out
}

/// Parse a datagram without checking its tag; `None` when it is malformed.
fn decode(data: &[u8]) -> Option<Message> {
    if data.len() < HEADER_LEN || data.len() > MAX_DATAGRAM || &data[..4] != MAGIC {
        return None;
    }
    let wire: Wire = serde_json::from_slice(&data[HEADER_LEN..]).ok()?;
    if wire.boot.len() != 16 || !wire.boot.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let kind = match wire.kind.as_str() {
        "heartbeat" => Kind::Heartbeat,
        "down" => Kind::Down(Down {
            service: wire.service?,
            digest: wire.digest?,
            backend: wire.backend?,
            reason: wire.reason?,
        }),
        "result" => Kind::Result(ResultMsg {
            service: wire.service?,
            digest: wire.digest?,
            backend: wire.backend?,
            outcome: match wire.outcome?.as_str() {
                "up" => Outcome::Up,
                "down" => Outcome::Down(wire.reason?),
                _ => return None,
            },
            interval_ms: wire.interval_ms.filter(|ms| *ms > 0)?,
        }),
        other => Kind::Unknown(other.to_string()),
    };
    Some(Message {
        from: wire.from.parse().ok()?,
        site: wire.site,
        boot: u64::from_str_radix(&wire.boot, 16).ok()?,
        seq: wire.seq,
        ts: wire.ts,
        kind,
    })
}

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

    fn index(self) -> usize {
        self as usize
    }
}

/// The current secret and the one it replaced: messages are signed with the
/// replaced one for [`SIGN_PREVIOUS_FOR`] and it is accepted for
/// [`ACCEPT_PREVIOUS_FOR`] after the change.
#[derive(Debug, Clone)]
pub struct Keys {
    current: Vec<u8>,
    previous: Option<(Vec<u8>, Instant)>,
}

impl Keys {
    pub fn new(current: Vec<u8>) -> Keys {
        Keys {
            current,
            previous: None,
        }
    }

    /// The newest secret read from the file.
    pub fn current(&self) -> &[u8] {
        &self.current
    }

    /// The secret to sign with at `now`.
    pub fn signing(&self, now: Instant) -> &[u8] {
        match &self.previous {
            Some((k, at)) if now.duration_since(*at) < SIGN_PREVIOUS_FOR => k,
            _ => &self.current,
        }
    }

    /// Make `new` the current secret. Returns false when it is the current
    /// one already.
    pub fn rotate(&mut self, new: Vec<u8>, now: Instant) -> bool {
        if new == self.current {
            return false;
        }
        let old = std::mem::replace(&mut self.current, new);
        self.previous = Some((old, now));
        true
    }

    /// Constant-time check of `tag` over `MAGIC` and `payload`.
    fn verifies(&self, payload: &[u8], tag: &[u8], now: Instant) -> bool {
        let check = |key: &[u8]| {
            let mut m = mac(key);
            m.update(MAGIC);
            m.update(payload);
            m.verify_slice(tag).is_ok()
        };
        check(&self.current)
            || self
                .previous
                .as_ref()
                .is_some_and(|(k, at)| now.duration_since(*at) < ACCEPT_PREVIOUS_FOR && check(k))
    }
}

/// Counters and member liveness, read by `/metrics`.
#[derive(Debug, Default)]
pub struct PeerStats {
    rejected: [AtomicU64; 6],
    sent: AtomicU64,
    received: AtomicU64,
    results_sent: AtomicU64,
    results_received: AtomicU64,
    disagreements: AtomicU64,
    checks_skipped: AtomicU64,
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

    fn set_up(&self, members: impl Iterator<Item = (SocketAddr, bool)>) {
        let mut up = self.up.lock().unwrap_or_else(|e| e.into_inner());
        *up = members.collect();
    }
}

/// What we know about one other member.
#[derive(Debug, Default)]
struct PeerState {
    /// `(boot, seq, ts)` of the last accepted message.
    last: Option<(u64, u64, u64)>,
    seen: Option<Instant>,
    live: bool,
}

/// The peer group as this daemon sees it: members, secrets, replay state
/// and liveness. No I/O; [`run`] drives it.
#[derive(Debug)]
pub struct Group {
    me: SocketAddr,
    site: String,
    members: Vec<SocketAddr>,
    pub keys: Keys,
    boot: u64,
    seq: u64,
    peers: HashMap<SocketAddr, PeerState>,
    warned: HashMap<(SocketAddr, Reject), Instant>,
    stats: Arc<PeerStats>,
}

impl Group {
    pub fn new(
        me: SocketAddr,
        site: String,
        members: Vec<SocketAddr>,
        keys: Keys,
        boot: u64,
        stats: Arc<PeerStats>,
    ) -> Group {
        let mut g = Group {
            me,
            site,
            members: Vec::new(),
            keys,
            boot,
            seq: 0,
            peers: HashMap::new(),
            warned: HashMap::new(),
            stats,
        };
        g.set_members(members);
        g
    }

    /// Replace the member list: kept members keep their state, removed ones
    /// lose it, added ones start not live.
    pub fn set_members(&mut self, members: Vec<SocketAddr>) {
        let me = self.me;
        self.peers.retain(|a, _| members.contains(a));
        for m in members.iter().filter(|m| **m != me) {
            self.peers.entry(*m).or_default();
        }
        self.members = members;
        self.publish_up();
    }

    pub fn set_site(&mut self, site: String) {
        self.site = site;
    }

    /// Members other than this daemon, in config order.
    pub fn others(&self) -> Vec<SocketAddr> {
        self.members
            .iter()
            .copied()
            .filter(|m| *m != self.me)
            .collect()
    }

    pub fn stats(&self) -> &Arc<PeerStats> {
        &self.stats
    }

    /// The next message of `kind`, numbered and signed with the secret for
    /// `now`.
    pub fn encode(&mut self, kind: Kind, now_ms: u64, now: Instant) -> Vec<u8> {
        self.seq += 1;
        let msg = Message {
            from: self.me,
            site: self.site.clone(),
            boot: self.boot,
            seq: self.seq,
            ts: now_ms,
            kind,
        };
        encode(&msg, self.keys.signing(now))
    }

    /// Check a received datagram. An accepted message marks its sender as
    /// seen (and live); a rejected one is counted and warned about.
    pub fn accept(
        &mut self,
        data: &[u8],
        src: SocketAddr,
        now_ms: u64,
        now: Instant,
    ) -> Result<Message, Reject> {
        match self.check(data, src, now_ms, now) {
            Ok(msg) => {
                if let Some(p) = self.peers.get_mut(&msg.from) {
                    p.last = Some((msg.boot, msg.seq, msg.ts));
                    p.seen = Some(now);
                    if !p.live {
                        p.live = true;
                        tracing::info!(peer = %msg.from, "peer {} is live", msg.from);
                        self.publish_up();
                    }
                }
                Ok(msg)
            }
            Err(r) => {
                self.stats.rejected[r.index()].fetch_add(1, Ordering::Relaxed);
                self.warn_rejected(src, r, now);
                Err(r)
            }
        }
    }

    fn check(
        &self,
        data: &[u8],
        src: SocketAddr,
        now_ms: u64,
        now: Instant,
    ) -> Result<Message, Reject> {
        let msg = decode(data).ok_or(Reject::Malformed)?;
        if !self
            .keys
            .verifies(&data[HEADER_LEN..], &data[MAGIC.len()..HEADER_LEN], now)
        {
            return Err(Reject::Auth);
        }
        if msg.from == self.me || msg.from != src || !self.members.contains(&msg.from) {
            return Err(Reject::Sender);
        }
        if msg.site != self.site {
            return Err(Reject::Site);
        }
        if now_ms.abs_diff(msg.ts) > MAX_SKEW_MS {
            return Err(Reject::Stale);
        }
        if let Some((boot, seq, ts)) = self.peers.get(&msg.from).and_then(|p| p.last) {
            let newer = if msg.boot == boot {
                msg.seq > seq
            } else {
                msg.ts > ts
            };
            if !newer {
                return Err(Reject::Replay);
            }
        }
        Ok(msg)
    }

    fn warn_rejected(&mut self, src: SocketAddr, r: Reject, now: Instant) {
        if self.warned.len() >= MAX_WARN_ENTRIES {
            self.warned
                .retain(|_, at| now.duration_since(*at) < WARN_EVERY);
        }
        let recent = self
            .warned
            .get(&(src, r))
            .is_some_and(|at| now.duration_since(*at) < WARN_EVERY);
        if !recent && self.warned.len() < MAX_WARN_ENTRIES {
            self.warned.insert((src, r), now);
            tracing::warn!(
                source = %src,
                reason = r.as_str(),
                "dropped peer message from {src} ({}); further ones from it for this reason are not logged for 60s",
                r.as_str(),
            );
        }
    }

    /// Mark members not heard from for [`LIVE_WINDOW`] as not live.
    pub fn check_liveness(&mut self, now: Instant) {
        let mut changed = false;
        for (addr, p) in self.peers.iter_mut() {
            let live = p.seen.is_some_and(|s| now.duration_since(s) <= LIVE_WINDOW);
            if p.live && !live {
                p.live = false;
                changed = true;
                tracing::info!(
                    peer = %addr,
                    "peer {addr} is not live (no valid message for {}s)",
                    LIVE_WINDOW.as_secs()
                );
            }
        }
        if changed {
            self.publish_up();
        }
    }

    pub fn is_live(&self, member: SocketAddr) -> bool {
        self.peers.get(&member).is_some_and(|p| p.live)
    }

    /// The members other than this daemon that are live.
    pub fn live_members(&self) -> Vec<SocketAddr> {
        self.peers
            .iter()
            .filter(|(_, p)| p.live)
            .map(|(a, _)| *a)
            .collect()
    }

    /// Members other than this daemon in config order, with liveness.
    pub fn members_live(&self) -> Vec<(SocketAddr, bool)> {
        self.others()
            .into_iter()
            .map(|m| (m, self.is_live(m)))
            .collect()
    }

    fn publish_up(&self) {
        self.stats
            .set_up(self.peers.iter().map(|(a, p)| (*a, p.live)));
    }
}

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
pub struct Targets(Arc<RwLock<HashMap<String, ServiceTargets>>>);

impl Targets {
    pub fn replace(&self, services: HashMap<String, ServiceTargets>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = services;
    }

    pub fn digest(&self, service: &str) -> Option<String> {
        let map = self.0.read().unwrap_or_else(|e| e.into_inner());
        map.get(service).map(|s| s.digest.clone())
    }

    /// The digest of `service` and whether it has `backend`; `None` when
    /// this daemon has no such service.
    pub fn backend(&self, service: &str, backend: &str) -> Option<(String, bool)> {
        let map = self.0.read().unwrap_or_else(|e| e.into_inner());
        map.get(service)
            .map(|s| (s.digest.clone(), s.backends.contains_key(backend)))
    }

    /// Hand a `down` report to its backend task. `Err` says why it was
    /// ignored.
    pub fn deliver(&self, down: &Down, from: SocketAddr) -> Result<(), &'static str> {
        let map = self.0.read().unwrap_or_else(|e| e.into_inner());
        let s = map.get(&down.service).ok_or("no such service here")?;
        if s.digest != down.digest {
            return Err(DIGEST_DIFFERS);
        }
        let r = s
            .backends
            .get(&down.backend)
            .ok_or("no such backend here")?;
        r.send(PeerReport {
            from,
            reason: down.reason.clone(),
        });
        Ok(())
    }
}

/// A backend of this daemon turned from healthy to unhealthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendDown {
    pub service: Arc<str>,
    pub backend: String,
    pub reason: String,
}

/// An assigned checker's own `up` or `down` result of one check, to be
/// sent to the other members as a `result` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendResult {
    pub service: Arc<str>,
    pub backend: String,
    pub outcome: Outcome,
    pub interval: Duration,
}

/// What a backend task hands to the peer task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerOut {
    Down(BackendDown),
    Result(BackendResult),
}

pub type PeerSender = mpsc::UnboundedSender<PeerOut>;

/// What a backend task of a daemon in a peer group uses: where to send
/// announcements and results, the votes to combine with, and the current
/// digests.
#[derive(Debug, Clone)]
pub struct PeerHooks {
    pub out: PeerSender,
    pub quorum: Arc<Quorum>,
    pub targets: Targets,
}

/// Settings the running peer task follows on reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerSettings {
    pub members: Vec<SocketAddr>,
    pub secret_file: PathBuf,
    pub site: String,
}

/// The running peer task.
#[derive(Debug)]
pub struct PeersHandle {
    pub listen: SocketAddr,
    pub stats: Arc<PeerStats>,
    pub out: PeerSender,
    pub quorum: Arc<Quorum>,
    pub targets: Targets,
    /// The first value of `meta/peers`, written in the start transaction.
    pub initial_state: String,
    settings: watch::Sender<PeerSettings>,
    task: JoinHandle<()>,
}

impl PeersHandle {
    /// What backend tasks need from the peer group.
    pub fn hooks(&self) -> PeerHooks {
        PeerHooks {
            out: self.out.clone(),
            quorum: self.quorum.clone(),
            targets: self.targets.clone(),
        }
    }

    /// Apply new members, secret file and site.
    pub fn reload(&self, config: &PeersConfig, site: &str) {
        self.settings.send_replace(PeerSettings {
            members: config.members.clone(),
            secret_file: config.secret_file.clone(),
            site: site.to_string(),
        });
    }
}

impl Drop for PeersHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Bind `[peers] listen` and start the peer task. Changes of `meta/peers`
/// go to `writer`.
pub async fn start(
    config: &PeersConfig,
    site: &str,
    targets: Targets,
    writer: Option<mpsc::UnboundedSender<WriterMsg>>,
    services_off: Vec<String>,
) -> Result<PeersHandle, String> {
    let socket = UdpSocket::bind(config.listen)
        .await
        .map_err(|e| format!("cannot bind peer address {}: {e}", config.listen))?;
    let key = read_secret(&config.secret_file).map_err(|e| format!("peers.secret_file: {e}"))?;
    Ok(start_on(
        socket,
        config,
        site,
        key,
        targets,
        writer,
        services_off,
    ))
}

/// Start the peer task on a bound socket. `services_off` are the services
/// with `peers = false`, shown in the first `meta/peers`.
pub fn start_on(
    socket: UdpSocket,
    config: &PeersConfig,
    site: &str,
    key: Vec<u8>,
    targets: Targets,
    writer: Option<mpsc::UnboundedSender<WriterMsg>>,
    services_off: Vec<String>,
) -> PeersHandle {
    let stats = Arc::new(PeerStats::default());
    let quorum = Arc::new(Quorum::new(config.listen));
    quorum.set_off(services_off);
    let boot = rand::rng().random::<u64>();
    let group = Group::new(
        config.listen,
        site.to_string(),
        config.members.clone(),
        Keys::new(key),
        boot,
        stats.clone(),
    );
    let settings = PeerSettings {
        members: config.members.clone(),
        secret_file: config.secret_file.clone(),
        site: site.to_string(),
    };
    let (settings_tx, settings_rx) = watch::channel(settings);
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let initial_state = quorum.summary(&group.members_live(), Instant::now());
    let task = tokio::spawn(run(
        socket,
        group,
        settings_rx,
        out_rx,
        targets.clone(),
        quorum.clone(),
        StateOut {
            writer,
            last: initial_state.clone(),
        },
    ));
    PeersHandle {
        listen: config.listen,
        stats,
        out: out_tx,
        quorum,
        targets,
        initial_state,
        settings: settings_tx,
        task,
    }
}

/// Re-read the secret: a changed valid one becomes current; a missing or
/// invalid one keeps the last valid secret, warning at most once a minute.
fn refresh_secret(group: &mut Group, path: &Path, warned: &mut Option<Instant>, now: Instant) {
    match read_secret(path) {
        Ok(k) => {
            if group.keys.rotate(k, now) {
                tracing::info!(
                    "peer secret changed; messages are signed with the previous one for {}s more and it is accepted for {}s",
                    SIGN_PREVIOUS_FOR.as_secs(),
                    ACCEPT_PREVIOUS_FOR.as_secs()
                );
            }
        }
        Err(e) => {
            if warned.is_none_or(|w| now.duration_since(w) >= WARN_EVERY) {
                *warned = Some(now);
                tracing::warn!("peers.secret_file: {e}; keeping the last valid secret");
            }
        }
    }
}

async fn send_all(socket: &UdpSocket, to: &[SocketAddr], data: &[u8]) {
    for addr in to {
        if let Err(e) = socket.send_to(data, addr).await {
            tracing::debug!("cannot send peer message to {addr}: {e}");
        }
    }
}

/// What [`Targets::deliver`] says when the digest differs.
const DIGEST_DIFFERS: &str = "check definition differs";

/// Handle an accepted `down` or `result` message.
fn received(group: &Group, targets: &Targets, quorum: &Quorum, msg: &Message, now: Instant) {
    match &msg.kind {
        Kind::Down(d) => {
            group.stats().received.fetch_add(1, Ordering::Relaxed);
            if let Err(why) = targets.deliver(d, msg.from) {
                if why == DIGEST_DIFFERS {
                    quorum.note_mismatch(&d.service, msg.from, now);
                }
                tracing::debug!(
                    peer = %msg.from,
                    "ignoring report of backend {} of {} down from peer {}: {why}",
                    d.backend, d.service, msg.from,
                );
            }
        }
        Kind::Result(r) => {
            group
                .stats()
                .results_received
                .fetch_add(1, Ordering::Relaxed);
            let Some((digest, has_backend)) = targets.backend(&r.service, &r.backend) else {
                return;
            };
            if digest != r.digest {
                quorum.note_mismatch(&r.service, msg.from, now);
                tracing::debug!(
                    peer = %msg.from,
                    "result for backend {} of {} from peer {} has another check definition; not counted",
                    r.backend, r.service, msg.from,
                );
            }
            if has_backend {
                quorum.record(msg.from, r, now);
            }
        }
        Kind::Heartbeat | Kind::Unknown(_) => {}
    }
}

/// Where `meta/peers` goes, and the value it was given last.
struct StateOut {
    writer: Option<mpsc::UnboundedSender<WriterMsg>>,
    last: String,
}

impl StateOut {
    /// Hand `state` to the writer when it differs from the last value.
    fn publish(&mut self, state: String) {
        if state != self.last {
            if let Some(w) = &self.writer {
                // The writer is gone only while the daemon stops.
                let _ = w.send(WriterMsg::PeerState(state.clone()));
            }
            self.last = state;
        }
    }
}

async fn run(
    socket: UdpSocket,
    mut group: Group,
    mut settings: watch::Receiver<PeerSettings>,
    mut out: mpsc::UnboundedReceiver<PeerOut>,
    targets: Targets,
    quorum: Arc<Quorum>,
    mut state: StateOut,
) {
    let mut secret_file = settings.borrow().secret_file.clone();
    let mut secret_warned = None;
    let mut tick = tokio::time::interval(HEARTBEAT_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // One byte more than accepted, so an oversized datagram is seen as such.
    let mut buf = vec![0u8; MAX_DATAGRAM + 1];
    loop {
        tokio::select! {
            r = socket.recv_from(&mut buf) => match r {
                Ok((n, src)) => {
                    let now = Instant::now();
                    let was_live = group.is_live(src);
                    if let Ok(msg) = group.accept(&buf[..n], src, unix_ms(), now) {
                        // An accepted message can only make its sender live;
                        // members stop being live on the tick below.
                        if !was_live {
                            quorum.set_live(group.live_members());
                        }
                        received(&group, &targets, &quorum, &msg, now);
                    }
                }
                Err(e) => tracing::debug!("peer socket receive error: {e}"),
            },
            _ = tick.tick() => {
                let now = Instant::now();
                refresh_secret(&mut group, &secret_file, &mut secret_warned, now);
                group.check_liveness(now);
                quorum.set_live(group.live_members());
                group
                    .stats()
                    .disagreements
                    .store(quorum.disagreements() as u64, Ordering::Relaxed);
                group
                    .stats()
                    .checks_skipped
                    .store(quorum.skipped(), Ordering::Relaxed);
                state.publish(quorum.summary(&group.members_live(), now));
                let data = group.encode(Kind::Heartbeat, unix_ms(), now);
                send_all(&socket, &group.others(), &data).await;
            }
            Some(ev) = out.recv() => {
                let service = match &ev {
                    PeerOut::Down(d) => d.service.clone(),
                    PeerOut::Result(r) => r.service.clone(),
                };
                let Some(digest) = targets.digest(&service) else {
                    continue; // service removed meanwhile
                };
                let (kind, counter) = match ev {
                    PeerOut::Down(d) => (
                        Kind::Down(Down {
                            service: service.to_string(),
                            digest,
                            backend: d.backend,
                            reason: d.reason,
                        }),
                        &group.stats().sent,
                    ),
                    PeerOut::Result(r) => (
                        Kind::Result(ResultMsg {
                            service: service.to_string(),
                            digest,
                            backend: r.backend,
                            outcome: r.outcome,
                            interval_ms: u64::try_from(r.interval.as_millis())
                                .unwrap_or(u64::MAX)
                                .max(1),
                        }),
                        &group.stats().results_sent,
                    ),
                };
                let others = group.others();
                counter.fetch_add(others.len() as u64, Ordering::Relaxed);
                let data = group.encode(kind, unix_ms(), Instant::now());
                send_all(&socket, &others, &data).await;
            }
            changed = settings.changed() => {
                if changed.is_err() {
                    return; // handle dropped
                }
                let s = settings.borrow_and_update().clone();
                quorum.retain_members(&s.members);
                group.set_members(s.members);
                quorum.set_live(group.live_members());
                group.set_site(s.site);
                if s.secret_file != secret_file {
                    secret_file = s.secret_file;
                    refresh_secret(&mut group, &secret_file, &mut secret_warned, Instant::now());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::secret_file;

    const KEY: &[u8] = &[7u8; 32];
    const DIGEST: &str = "0123456789abcdef0123456789abcdef";
    const ME: &str = "10.0.0.5:5302";
    const B: &str = "10.0.0.6:5302";

    fn addr(s: &str) -> SocketAddr {
        s.parse()
            .expect("test addresses are valid socket addresses")
    }

    fn msg(from: &str, boot: u64, seq: u64, ts: u64, kind: Kind) -> Message {
        Message {
            from: addr(from),
            site: "dc1".into(),
            boot,
            seq,
            ts,
            kind,
        }
    }

    fn group(now: Instant) -> Group {
        let _ = now;
        Group::new(
            addr(ME),
            "dc1".into(),
            vec![addr(ME), addr(B)],
            Keys::new(KEY.to_vec()),
            1,
            Arc::new(PeerStats::default()),
        )
    }

    fn down(reason: &str) -> Kind {
        Kind::Down(Down {
            service: "api.svc.emfor".into(),
            digest: "0123456789abcdef0123456789abcdef".into(),
            backend: "10.0.0.11:8080".into(),
            reason: reason.into(),
        })
    }

    fn result(outcome: Outcome) -> Kind {
        Kind::Result(ResultMsg {
            service: "api.svc.emfor".into(),
            digest: "0123456789abcdef0123456789abcdef".into(),
            backend: "10.0.0.11:8080".into(),
            outcome,
            interval_ms: 2000,
        })
    }

    /// A signed datagram whose payload is `json`.
    fn raw(json: &str) -> Vec<u8> {
        let mut m = mac(KEY);
        m.update(MAGIC);
        m.update(json.as_bytes());
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&m.finalize().into_bytes());
        out.extend_from_slice(json.as_bytes());
        out
    }

    /// Spec peer-group "Assigned checkers and check results": a `result`
    /// that lacks a field or has an invalid value is malformed.
    #[test]
    fn malformed_result_messages() {
        let now = Instant::now();
        let t = 1_763_000_000_000u64;
        let head = format!(
            r#""from":"{B}","site":"dc1","boot":"0000000000000001","ts":{t},"kind":"result","service":"api.svc.emfor","digest":"0123456789abcdef0123456789abcdef","backend":"10.0.0.11:8080""#
        );
        let cases = [
            (r#""outcome":"pending","interval_ms":2000"#, false),
            (r#""outcome":"down","interval_ms":2000"#, false),
            (r#""outcome":"up","interval_ms":0"#, false),
            (r#""outcome":"up""#, false),
            (r#""interval_ms":2000"#, false),
            (r#""outcome":"up","interval_ms":2000"#, true),
            (
                r#""outcome":"down","reason":"timeout","interval_ms":2000"#,
                true,
            ),
        ];
        for (seq, (tail, ok)) in cases.into_iter().enumerate() {
            let mut g = group(now);
            let data = raw(&format!("{{{head},\"seq\":{},{tail}}}", seq + 1));
            let got = g.accept(&data, addr(B), t, now).map(|_| ());
            let want = if ok { Ok(()) } else { Err(Reject::Malformed) };
            assert_eq!(got, want, "{tail}");
        }
    }

    #[test]
    fn encode_then_decode() {
        for kind in [
            Kind::Heartbeat,
            down("connection refused"),
            result(Outcome::Up),
            result(Outcome::Down("connect: Connection refused".into())),
        ] {
            let m = msg(B, 0xaaaa_aaaa_aaaa_aaaa, 42, 1_763_000_000_123, kind);
            let data = encode(&m, KEY);
            assert_eq!(&data[..4], MAGIC);
            assert_eq!(decode(&data), Some(m));
        }
        let m = msg("[2001:db8::1]:5302", 1, 1, 1, Kind::Heartbeat);
        assert_eq!(decode(&encode(&m, KEY)), Some(m));
    }

    #[test]
    fn reason_is_cut_at_a_character_boundary() {
        // 255 ASCII bytes then a 3-byte character: 258 bytes.
        let reason = format!("{}€", "x".repeat(255));
        let data = encode(&msg(B, 1, 1, 1, down(&reason)), KEY);
        let Some(Message {
            kind: Kind::Down(d),
            ..
        }) = decode(&data)
        else {
            panic!("a down message decodes as down");
        };
        assert_eq!(d.reason, "x".repeat(255));
        assert_eq!(truncate_utf8("ab€", 4), "ab");
        assert_eq!(truncate_utf8("ab€", 5), "ab€");
    }

    #[test]
    fn largest_message_fits_one_datagram() {
        let service = format!("{}.svc", "a".repeat(248));
        let backend = "[ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff]:65535".to_string();
        // Quotes double when escaped in JSON.
        let reason = "\"".repeat(400);
        let kinds = [
            Kind::Down(Down {
                service: service.clone(),
                digest: "f".repeat(32),
                backend: backend.clone(),
                reason: reason.clone(),
            }),
            Kind::Result(ResultMsg {
                service,
                digest: "f".repeat(32),
                backend,
                outcome: Outcome::Down(reason),
                interval_ms: u64::MAX,
            }),
        ];
        for kind in kinds {
            let mut m = msg(
                "[ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff]:65535",
                u64::MAX,
                u64::MAX,
                u64::MAX,
                kind,
            );
            m.site = "s".repeat(63);
            let data = encode(&m, KEY);
            assert!(data.len() <= MAX_DATAGRAM, "{} bytes", data.len());
            assert!(
                decode(&data).is_some(),
                "the shortened message still decodes"
            );
        }
    }

    /// Spec peer-group "acceptance checks" table.
    #[test]
    fn acceptance_checks() {
        let now = Instant::now();
        let t = 1_763_000_000_000u64;
        let hb = |seq| msg(B, 1, seq, t, Kind::Heartbeat);
        type Case<'a> = (&'a str, Vec<u8>, &'a str, Result<(), Reject>);
        let cases: Vec<Case> = vec![
            ("valid heartbeat", encode(&hb(1), KEY), B, Ok(())),
            ("20 bytes", vec![b'E'; 20], B, Err(Reject::Malformed)),
            (
                "result with outcome pending",
                raw(&format!(
                    r#"{{"from":"{B}","site":"dc1","boot":"0000000000000001","seq":1,"ts":{t},"kind":"result","service":"s","digest":"d","backend":"b","outcome":"pending","interval_ms":2000}}"#
                )),
                B,
                Err(Reject::Malformed),
            ),
            (
                "other secret",
                encode(&hb(1), &[8u8; 32]),
                B,
                Err(Reject::Auth),
            ),
            (
                "spoofed source",
                encode(&hb(1), KEY),
                "10.0.0.8:5302",
                Err(Reject::Sender),
            ),
            (
                "not a member",
                encode(&msg("10.0.0.8:5302", 1, 1, t, Kind::Heartbeat), KEY),
                "10.0.0.8:5302",
                Err(Reject::Sender),
            ),
            (
                "other site",
                encode(
                    &Message {
                        site: "dc2".into(),
                        ..hb(1)
                    },
                    KEY,
                ),
                B,
                Err(Reject::Site),
            ),
            (
                "6s behind",
                encode(
                    &Message {
                        ts: t - 6000,
                        ..hb(1)
                    },
                    KEY,
                ),
                B,
                Err(Reject::Stale),
            ),
            (
                "unknown kind",
                encode(&msg(B, 1, 1, t, Kind::Unknown("assign".into())), KEY),
                B,
                Ok(()),
            ),
        ];
        for (what, data, src, want) in cases {
            let mut g = group(now);
            let got = g.accept(&data, addr(src), t, now).map(|_| ());
            assert_eq!(got, want, "{what}");
            if let Err(r) = want {
                assert_eq!(g.stats.rejected(r), 1, "{what}");
            } else {
                assert!(g.is_live(addr(B)), "{what}");
            }
        }
        // Tampered payload.
        let mut g = group(now);
        let mut data = encode(&hb(1), KEY);
        let at = data
            .windows(7)
            .position(|w| w == b"\"seq\":1")
            .expect("the payload carries \"seq\":1");
        data[at + 6] = b'2';
        assert_eq!(
            g.accept(&data, addr(B), t, now).map(|_| ()),
            Err(Reject::Auth)
        );
        // Replay: same boot, seq equal to the last accepted.
        let mut g = group(now);
        assert!(g.accept(&encode(&hb(5), KEY), addr(B), t, now).is_ok());
        assert_eq!(
            g.accept(&encode(&hb(5), KEY), addr(B), t, now).map(|_| ()),
            Err(Reject::Replay)
        );
        assert!(g.accept(&encode(&hb(6), KEY), addr(B), t, now).is_ok());
        // A message from this daemon itself.
        let mut g = group(now);
        let own = encode(&msg(ME, 1, 1, t, Kind::Heartbeat), KEY);
        assert_eq!(
            g.accept(&own, addr(ME), t, now).map(|_| ()),
            Err(Reject::Sender)
        );
    }

    /// Spec peer-group "Sender restarts".
    #[test]
    fn sender_restarts() {
        let now = Instant::now();
        let t = 1_763_000_000_000u64;
        let mut g = group(now);
        let old = msg(B, 0xaaaa_aaaa_aaaa_aaaa, 900, t, Kind::Heartbeat);
        assert!(g.accept(&encode(&old, KEY), addr(B), t, now).is_ok());
        let new = msg(B, 0xbbbb_bbbb_bbbb_bbbb, 1, t + 10, Kind::Heartbeat);
        assert!(g.accept(&encode(&new, KEY), addr(B), t + 10, now).is_ok());
        // The old boot replayed afterwards, still within 5s: rejected.
        let replayed = msg(B, 0xaaaa_aaaa_aaaa_aaaa, 901, t + 5, Kind::Heartbeat);
        assert_eq!(
            g.accept(&encode(&replayed, KEY), addr(B), t + 20, now)
                .map(|_| ()),
            Err(Reject::Replay)
        );
    }

    /// Spec peer-group "acceptance checks", rows on a replaced secret.
    #[test]
    fn previous_secret_accepted_for_120_seconds() {
        let start = Instant::now();
        let t = 1_763_000_000_000u64;
        let mut g = group(start);
        assert!(g.keys.rotate(vec![9u8; 32], start));
        assert!(!g.keys.rotate(vec![9u8; 32], start));
        let old = |seq| encode(&msg(B, 1, seq, t, Kind::Heartbeat), KEY);
        for (secs, seq) in [(30, 1), (90, 2)] {
            let at = start + Duration::from_secs(secs);
            assert!(g.accept(&old(seq), addr(B), t, at).is_ok(), "{secs}s");
        }
        let at130 = start + Duration::from_secs(130);
        assert_eq!(
            g.accept(&old(3), addr(B), t, at130).map(|_| ()),
            Err(Reject::Auth)
        );
        let new = encode(&msg(B, 1, 4, t, Kind::Heartbeat), &[9u8; 32]);
        assert!(g.accept(&new, addr(B), t, at130).is_ok());
    }

    /// Spec peer-group "Secret rotated on one host first": A's file changes
    /// first, B's 40 seconds later; every message both ways is accepted.
    #[test]
    fn secret_rotated_on_one_host_first() {
        let start = Instant::now();
        let t = 1_763_000_000_000u64;
        let stats = || Arc::new(PeerStats::default());
        let members = vec![addr(ME), addr(B)];
        let mut a = Group::new(
            addr(ME),
            "dc1".into(),
            members.clone(),
            Keys::new(KEY.to_vec()),
            1,
            stats(),
        );
        let mut b = Group::new(
            addr(B),
            "dc1".into(),
            members,
            Keys::new(KEY.to_vec()),
            2,
            stats(),
        );
        let new = vec![9u8; 32];
        for secs in (0..=200).step_by(5) {
            let now = start + Duration::from_secs(secs);
            if secs == 0 {
                a.keys.rotate(new.clone(), now);
            }
            if secs == 40 {
                b.keys.rotate(new.clone(), now);
            }
            let from_a = a.encode(Kind::Heartbeat, t, now);
            assert!(
                b.accept(&from_a, addr(ME), t, now).is_ok(),
                "A to B at {secs}s"
            );
            let from_b = b.encode(Kind::Heartbeat, t, now);
            assert!(
                a.accept(&from_b, addr(B), t, now).is_ok(),
                "B to A at {secs}s"
            );
        }
        // Signing switches to the new secret 60 seconds after the change.
        let mine = a.encode(Kind::Heartbeat, t, start + Duration::from_secs(59));
        assert!(Keys::new(KEY.to_vec()).verifies(&mine[HEADER_LEN..], &mine[4..HEADER_LEN], start));
        let mine = a.encode(Kind::Heartbeat, t, start + Duration::from_secs(60));
        assert!(Keys::new(new).verifies(&mine[HEADER_LEN..], &mine[4..HEADER_LEN], start));
    }

    #[test]
    fn secret_file_reload() {
        let start = Instant::now();
        let (_d, path) = secret_file(&[1u8; 40], 0o600);
        let mut g = group(start);
        let mut warned = None;
        refresh_secret(&mut g, &path, &mut warned, start);
        assert_eq!(g.keys.current(), &[1u8; 40]);
        std::fs::remove_file(&path).expect("the temporary secret file can be removed");
        refresh_secret(&mut g, &path, &mut warned, start + Duration::from_secs(1));
        assert_eq!(g.keys.current(), &[1u8; 40], "keeps the last valid secret");
        assert_eq!(warned, Some(start + Duration::from_secs(1)));
        refresh_secret(&mut g, &path, &mut warned, start + Duration::from_secs(30));
        assert_eq!(
            warned,
            Some(start + Duration::from_secs(1)),
            "one warning a minute"
        );
        refresh_secret(&mut g, &path, &mut warned, start + Duration::from_secs(61));
        assert_eq!(warned, Some(start + Duration::from_secs(61)));
    }

    #[test]
    #[tracing_test::traced_test]
    fn rejections_are_warned_once_a_minute() {
        let start = Instant::now();
        let mut g = group(start);
        for s in [0, 10, 59, 61] {
            let _ = g.accept(b"short", addr(B), 0, start + Duration::from_secs(s));
        }
        logs_assert(|lines: &[&str]| {
            match lines
                .iter()
                .filter(|l| l.contains("dropped peer message"))
                .count()
            {
                2 => Ok(()),
                n => Err(format!("{n} warnings")),
            }
        });
        assert_eq!(g.stats.rejected(Reject::Malformed), 4);
    }

    #[test]
    fn liveness_and_members() {
        let start = Instant::now();
        let t = 1_763_000_000_000u64;
        let mut g = group(start);
        assert_eq!(g.stats.up(), vec![(addr(B), false)]);
        assert!(
            g.accept(
                &encode(&msg(B, 1, 1, t, Kind::Heartbeat), KEY),
                addr(B),
                t,
                start
            )
            .is_ok()
        );
        assert_eq!(g.stats.up(), vec![(addr(B), true)]);
        g.check_liveness(start + Duration::from_secs(3));
        assert!(g.is_live(addr(B)));
        g.check_liveness(start + Duration::from_millis(3001));
        assert_eq!(g.stats.up(), vec![(addr(B), false)]);
        // Reload: B removed, C added as not live.
        g.set_members(vec![addr(ME), addr("10.0.0.7:5302")]);
        assert_eq!(g.stats.up(), vec![(addr("10.0.0.7:5302"), false)]);
        assert_eq!(g.others(), vec![addr("10.0.0.7:5302")]);
    }

    fn targets(digest: &str) -> (Targets, Arc<Recheck>) {
        let r = Arc::new(Recheck::default());
        let t = Targets::default();
        t.replace(HashMap::from([(
            "api.svc.emfor".to_string(),
            ServiceTargets {
                digest: digest.into(),
                backends: HashMap::from([("10.0.0.11:8080".to_string(), r.clone())]),
            },
        )]));
        (t, r)
    }

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

    /// Two peer tasks on 127.0.0.1 (real time).
    async fn pair(
        targets_a: Targets,
        targets_b: Targets,
    ) -> (PeersHandle, PeersHandle, tempfile::TempDir, PathBuf) {
        let sa = UdpSocket::bind("127.0.0.1:0").await.expect("bind a");
        let sb = UdpSocket::bind("127.0.0.1:0").await.expect("bind b");
        let (a, b) = (
            sa.local_addr().expect("a has an address"),
            sb.local_addr().expect("b has an address"),
        );
        let (dir, path) = secret_file(&[3u8; 32], 0o600);
        let conf = |me| PeersConfig {
            listen: me,
            members: vec![a, b],
            secret_file: path.clone(),
        };
        let ha = start_on(
            sa,
            &conf(a),
            "dc1",
            vec![3u8; 32],
            targets_a,
            None,
            Vec::new(),
        );
        let hb = start_on(
            sb,
            &conf(b),
            "dc1",
            vec![3u8; 32],
            targets_b,
            None,
            Vec::new(),
        );
        (ha, hb, dir, path)
    }

    async fn wait_until(what: &str, limit: Duration, mut f: impl FnMut() -> bool) {
        let end = Instant::now() + limit;
        while !f() {
            assert!(Instant::now() < end, "{what} not within {limit:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Spec peer-group "Member stops" and "Member added on reload".
    #[tokio::test]
    async fn heartbeats_liveness_and_reload() {
        let (ha, hb, _dir, path) = pair(Targets::default(), Targets::default()).await;
        let b = hb.listen;
        wait_until("b live at a", Duration::from_millis(1500), || {
            ha.stats.up() == vec![(b, true)]
        })
        .await;
        drop(hb);
        let stopped = Instant::now();
        wait_until("b not live at a", Duration::from_secs(5), || {
            ha.stats.up() == vec![(b, false)]
        })
        .await;
        assert!(stopped.elapsed() <= Duration::from_secs(4) + Duration::from_millis(200));
        // Add a third member: it appears, not live, and receives heartbeats.
        let sc = UdpSocket::bind("127.0.0.1:0").await.expect("bind c");
        let c = sc.local_addr().expect("c has an address");
        ha.reload(
            &PeersConfig {
                listen: ha.listen,
                members: vec![ha.listen, b, c],
                secret_file: path,
            },
            "dc1",
        );
        let mut buf = [0u8; 1500];
        let (n, from) = tokio::time::timeout(Duration::from_secs(2), sc.recv_from(&mut buf))
            .await
            .expect("c receives a heartbeat within 2s")
            .expect("receive works");
        assert_eq!(from, ha.listen);
        assert!(matches!(
            decode(&buf[..n]),
            Some(Message {
                kind: Kind::Heartbeat,
                ..
            })
        ));
        assert!(ha.stats.up().contains(&(c, false)));
    }

    #[tokio::test]
    async fn down_report_reaches_the_backend_task() {
        let Kind::Down(d) = down("refused") else {
            unreachable!("down() builds a down kind")
        };
        let (tb, rb) = targets(&d.digest);
        let (ha, hb, _dir, _path) = pair(Targets::default(), tb).await;
        // A sends only for services it has; give it the same one.
        let (ta, _) = targets(&d.digest);
        drop(ha);
        let sa = UdpSocket::bind("127.0.0.1:0").await.expect("bind a again");
        let a = sa.local_addr().expect("a has an address");
        let (_d2, path) = secret_file(&[3u8; 32], 0o600);
        hb.reload(
            &PeersConfig {
                listen: hb.listen,
                members: vec![a, hb.listen],
                secret_file: path.clone(),
            },
            "dc1",
        );
        let ha = start_on(
            sa,
            &PeersConfig {
                listen: a,
                members: vec![a, hb.listen],
                secret_file: path,
            },
            "dc1",
            vec![3u8; 32],
            ta,
            None,
            Vec::new(),
        );
        ha.out
            .send(PeerOut::Down(BackendDown {
                service: "api.svc.emfor".into(),
                backend: "10.0.0.11:8080".into(),
                reason: "refused".into(),
            }))
            .expect("the peer task is running");
        tokio::time::timeout(Duration::from_secs(2), rb.notified())
            .await
            .expect("b's backend task is woken within 2s");
        assert_eq!(rb.take().map(|r| r.from), Some(a));
        wait_until("counters", Duration::from_secs(1), || {
            ha.stats.sent() == 1 && hb.stats.received() == 1
        })
        .await;
    }

    /// A `result` from A is kept at B for a backend B has; one for a
    /// service B does not have is only counted.
    #[tokio::test]
    async fn result_reaches_the_votes() {
        let (ta, _) = targets(DIGEST);
        let mut services = ta.0.read().unwrap_or_else(|e| e.into_inner()).clone();
        services.insert(
            "other.svc.emfor".into(),
            ServiceTargets {
                digest: DIGEST.into(),
                backends: HashMap::new(),
            },
        );
        ta.replace(services);
        let (tb, _) = targets(DIGEST);
        let (ha, hb, _dir, _path) = pair(ta, tb).await;
        let a = ha.listen;
        for service in ["api.svc.emfor", "other.svc.emfor"] {
            ha.out
                .send(PeerOut::Result(BackendResult {
                    service: service.into(),
                    backend: "10.0.0.11:8080".into(),
                    outcome: Outcome::Down("refused".into()),
                    interval: Duration::from_secs(2),
                }))
                .expect("the peer task is running");
        }
        wait_until("counters", Duration::from_secs(2), || {
            ha.stats.results_sent() == 2 && hb.stats.results_received() == 2
        })
        .await;
        assert_eq!(
            hb.quorum.kept("api.svc.emfor", "10.0.0.11:8080", a),
            Some(Outcome::Down("refused".into()))
        );
        assert_eq!(hb.quorum.kept("other.svc.emfor", "10.0.0.11:8080", a), None);
        assert_eq!(
            ha.stats.sent(),
            0,
            "results are not counted as down messages"
        );
    }

    /// Spec peer-group "Down message for a service left out": the
    /// supervisor leaves a `peers = false` service out of `Targets`, so a
    /// `down` for it reaches no backend task and records no mismatch, even
    /// with another digest.
    #[tokio::test]
    async fn down_for_a_service_left_out_is_ignored() {
        let (ta, _) = targets(&"f".repeat(32));
        // B had the service in the group; a reload to `peers = false`
        // publishes targets without it.
        let (tb, left_out) = targets(DIGEST);
        tb.replace(HashMap::new());
        let (ha, hb, _dir, _path) = pair(ta, tb).await;
        ha.out
            .send(PeerOut::Down(BackendDown {
                service: "api.svc.emfor".into(),
                backend: "10.0.0.11:8080".into(),
                reason: "refused".into(),
            }))
            .expect("the peer task is running");
        wait_until("B accepts the down", Duration::from_secs(2), || {
            hb.stats.received() == 1
        })
        .await;
        assert_eq!(left_out.take(), None, "no fast recheck run");
        let summary = hb.quorum.summary(&[], Instant::now());
        assert!(summary.contains(r#""digest_mismatch":[]"#), "{summary}");
    }

    /// A message with another digest is a mismatch in `meta/peers`.
    #[tokio::test]
    async fn other_digest_is_a_mismatch() {
        let (ta, _) = targets(DIGEST);
        let (tb, _) = targets(&"f".repeat(32));
        let (ha, hb, _dir, _path) = pair(ta, tb).await;
        ha.out
            .send(PeerOut::Result(BackendResult {
                service: "api.svc.emfor".into(),
                backend: "10.0.0.11:8080".into(),
                outcome: Outcome::Up,
                interval: Duration::from_secs(2),
            }))
            .expect("the peer task is running");
        let a = ha.listen.to_string();
        wait_until("mismatch", Duration::from_secs(2), || {
            hb.quorum
                .summary(&[], Instant::now())
                .contains(&format!(r#"{{"service":"api.svc.emfor","member":"{a}"}}"#))
        })
        .await;
    }
}
