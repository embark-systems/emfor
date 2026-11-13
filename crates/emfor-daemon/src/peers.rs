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
use crate::schedule::unix_ms;

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

/// The datagram for `msg`, signed with `key`. A `down` reason is cut to
/// [`MAX_REASON_BYTES`].
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
    }
    let payload = serde_json::to_vec(&wire)
        .expect("a struct of strings and integers always serializes to JSON");
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

    /// Hand a `down` report to its backend task. `Err` says why it was
    /// ignored.
    pub fn deliver(&self, down: &Down, from: SocketAddr) -> Result<(), &'static str> {
        let map = self.0.read().unwrap_or_else(|e| e.into_inner());
        let s = map.get(&down.service).ok_or("no such service here")?;
        if s.digest != down.digest {
            return Err("check definition differs");
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

pub type DownSender = mpsc::UnboundedSender<BackendDown>;

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
    pub down: DownSender,
    settings: watch::Sender<PeerSettings>,
    task: JoinHandle<()>,
}

impl PeersHandle {
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

/// Bind `[peers] listen` and start the peer task.
pub async fn start(
    config: &PeersConfig,
    site: &str,
    targets: Targets,
) -> Result<PeersHandle, String> {
    let socket = UdpSocket::bind(config.listen)
        .await
        .map_err(|e| format!("cannot bind peer address {}: {e}", config.listen))?;
    let key = read_secret(&config.secret_file).map_err(|e| format!("peers.secret_file: {e}"))?;
    Ok(start_on(socket, config, site, key, targets))
}

/// Start the peer task on a bound socket.
pub fn start_on(
    socket: UdpSocket,
    config: &PeersConfig,
    site: &str,
    key: Vec<u8>,
    targets: Targets,
) -> PeersHandle {
    let stats = Arc::new(PeerStats::default());
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
    let (down_tx, down_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(socket, group, settings_rx, down_rx, targets));
    PeersHandle {
        listen: config.listen,
        stats,
        down: down_tx,
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

async fn run(
    socket: UdpSocket,
    mut group: Group,
    mut settings: watch::Receiver<PeerSettings>,
    mut down: mpsc::UnboundedReceiver<BackendDown>,
    targets: Targets,
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
                    if let Ok(msg) = group.accept(&buf[..n], src, unix_ms(), now)
                        && let Kind::Down(d) = &msg.kind
                    {
                        group.stats().received.fetch_add(1, Ordering::Relaxed);
                        if let Err(why) = targets.deliver(d, msg.from) {
                            tracing::debug!(
                                peer = %msg.from,
                                "ignoring report of backend {} of {} down from peer {}: {why}",
                                d.backend, d.service, msg.from,
                            );
                        }
                    }
                }
                Err(e) => tracing::debug!("peer socket receive error: {e}"),
            },
            _ = tick.tick() => {
                let now = Instant::now();
                refresh_secret(&mut group, &secret_file, &mut secret_warned, now);
                group.check_liveness(now);
                let data = group.encode(Kind::Heartbeat, unix_ms(), now);
                send_all(&socket, &group.others(), &data).await;
            }
            Some(ev) = down.recv() => {
                let Some(digest) = targets.digest(&ev.service) else {
                    continue; // service removed meanwhile
                };
                let others = group.others();
                let data = group.encode(
                    Kind::Down(Down {
                        service: ev.service.to_string(),
                        digest,
                        backend: ev.backend,
                        reason: ev.reason,
                    }),
                    unix_ms(),
                    Instant::now(),
                );
                send_all(&socket, &others, &data).await;
                group.stats().sent.fetch_add(others.len() as u64, Ordering::Relaxed);
            }
            changed = settings.changed() => {
                if changed.is_err() {
                    return; // handle dropped
                }
                let s = settings.borrow_and_update().clone();
                group.set_members(s.members);
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

    #[test]
    fn encode_then_decode() {
        for kind in [Kind::Heartbeat, down("connection refused")] {
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
        let kind = Kind::Down(Down {
            service: format!("{}.svc", "a".repeat(248)),
            digest: "f".repeat(32),
            backend: "[ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff]:65535".into(),
            // Quotes double when escaped in JSON.
            reason: "\"".repeat(400),
        });
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
                encode(&msg(B, 1, 1, t, Kind::Unknown("result".into())), KEY),
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
    async fn pair(targets_b: Targets) -> (PeersHandle, PeersHandle, tempfile::TempDir, PathBuf) {
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
        let ha = start_on(sa, &conf(a), "dc1", vec![3u8; 32], Targets::default());
        let hb = start_on(sb, &conf(b), "dc1", vec![3u8; 32], targets_b);
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
        let (ha, hb, _dir, path) = pair(Targets::default()).await;
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
        let (ha, hb, _dir, _path) = pair(tb).await;
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
        );
        ha.down
            .send(BackendDown {
                service: "api.svc.emfor".into(),
                backend: "10.0.0.11:8080".into(),
                reason: "refused".into(),
            })
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
}
