//! Peer group (spec peer-group): emfor daemons of one site that list each
//! other in `[peers]` exchange authenticated UDP messages. Members send a
//! heartbeat every second, and when a backend of theirs turns unhealthy they
//! tell the others, which then recheck it at once. A message never changes a
//! backend's state by itself; every daemon decides from its own checks.
//!
//! One message is one datagram whose payload is a JSON object, in one of two
//! formats. `EMP1`: the magic, a 32-byte HMAC-SHA256 tag over the magic and
//! the payload, then the payload in plaintext. `EMP2`: the magic, a 12-byte
//! random nonce, then the payload encrypted with ChaCha20-Poly1305 and its
//! 16-byte tag, with the magic as additional data and a key derived from
//! the secret with HKDF-SHA256. `[peers] encrypt` picks the format sent;
//! both are accepted.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chacha20poly1305::aead::{Aead, Generate, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
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
/// First four bytes of an encrypted message: magic and format version 2.
pub const MAGIC_V2: &[u8; 4] = b"EMP2";
const NONCE_LEN: usize = 12;
const AEAD_TAG_LEN: usize = 16;
/// Bytes of an `EMP2` datagram besides the payload: magic, nonce, tag.
const OVERHEAD_V2: usize = MAGIC_V2.len() + NONCE_LEN + AEAD_TAG_LEN;
/// Shortest `EMP2` datagram: the overhead and a payload of `{}`.
const MIN_DATAGRAM_V2: usize = OVERHEAD_V2 + 2;
const HKDF_SALT: &[u8] = b"emfor peers";
const HKDF_INFO: &[u8] = b"EMP2 chacha20-poly1305";
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
    /// With the service tags the sender leaves out of the peer group
    /// (`off`), sorted; empty when it leaves none out.
    Heartbeat(Vec<u32>),
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

/// The datagram format of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `EMP1`: authenticated, payload in plaintext.
    Plain,
    /// `EMP2`: payload encrypted.
    Encrypted,
}

/// A peer secret and the `EMP2` key derived from it.
#[derive(Clone)]
pub struct Secret {
    raw: Vec<u8>,
    aead: [u8; 32],
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Secret {
    pub fn new(raw: Vec<u8>) -> Secret {
        let mut aead = [0u8; 32];
        Hkdf::<Sha256>::new(Some(HKDF_SALT), &raw)
            .expand(HKDF_INFO, &mut aead)
            .expect("32 bytes is far below HKDF-SHA256's limit of 255 * 32 bytes of output");
        Secret { raw, aead }
    }

    /// The secret as read from the file.
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    fn cipher(&self) -> ChaCha20Poly1305 {
        ChaCha20Poly1305::new(&self.aead.into())
    }
}

/// The JSON payload as sent.
#[derive(Serialize)]
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
    /// Service tags of a heartbeat, as 8 lowercase hex digits each.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    off: Option<Vec<String>>,
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

/// The `EMP1` datagram for `msg`, signed with `key`. A reason (of a `down`
/// or `result` message) is cut to [`MAX_REASON_BYTES`], and further when
/// the datagram would otherwise exceed [`MAX_DATAGRAM`].
pub fn encode(msg: &Message, key: &[u8]) -> Vec<u8> {
    let payload = payload_for(msg, HEADER_LEN);
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

/// The `EMP2` datagram for `msg`, encrypted with the key of `secret` and a
/// nonce from the operating system's random generator. Reasons are cut as
/// for [`encode`].
pub fn encode_encrypted(msg: &Message, secret: &Secret) -> Vec<u8> {
    seal(&payload_for(msg, OVERHEAD_V2), secret, &Nonce::generate())
}

/// `EMP2`, the nonce, then `payload` encrypted under `nonce`.
fn seal(payload: &[u8], secret: &Secret, nonce: &Nonce) -> Vec<u8> {
    let sealed = secret
        .cipher()
        .encrypt(
            nonce,
            Payload {
                msg: payload,
                aad: MAGIC_V2,
            },
        )
        .expect(
            "ChaCha20-Poly1305 encrypts any payload below 256 GiB, and ours is at most 1200 bytes",
        );
    let mut out = Vec::with_capacity(MAGIC_V2.len() + NONCE_LEN + sealed.len());
    out.extend_from_slice(MAGIC_V2);
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&sealed);
    out
}

/// The JSON payload for `msg`, short enough that `overhead` more bytes fit
/// in [`MAX_DATAGRAM`] (by cutting the reason when there is one).
fn payload_for(msg: &Message, overhead: usize) -> Vec<u8> {
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
        off: None,
    };
    match &msg.kind {
        Kind::Heartbeat(tags) => {
            wire.kind = "heartbeat".into();
            if !tags.is_empty() {
                wire.off = Some(tags.iter().map(|t| format!("{t:08x}")).collect());
            }
        }
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
    while overhead + payload.len() > MAX_DATAGRAM
        && let Some(reason) = wire.reason.as_mut()
        && !reason.is_empty()
    {
        let over = overhead + payload.len() - MAX_DATAGRAM;
        let keep = truncate_utf8(reason, reason.len().saturating_sub(over)).len();
        reason.truncate(keep);
        payload = serde_json::to_vec(&wire)
            .expect("a struct of strings and integers always serializes to JSON");
    }
    payload
}

/// A service tag written as exactly 8 lowercase hex digits.
fn parse_tag(s: &str) -> Option<u32> {
    let ok = s.len() == 8 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    ok.then(|| u32::from_str_radix(s, 16).ok()).flatten()
}

/// Parse an `EMP1` datagram without checking its tag; `None` when it is
/// malformed. For tests and fuzzing: a receiver checks the tag first.
#[cfg(any(test, fuzzing))]
fn decode(data: &[u8]) -> Option<Message> {
    if data.len() < HEADER_LEN || data.len() > MAX_DATAGRAM || &data[..4] != MAGIC {
        return None;
    }
    parse_payload(&data[HEADER_LEN..])
}

/// The JSON payload as received: the common fields with their types, and
/// the fields of the known kinds as any JSON value, checked only for the
/// kind that uses them, so that a newer kind may give one of these names
/// another type.
#[derive(Deserialize)]
struct WireIn {
    from: String,
    site: String,
    boot: String,
    seq: u64,
    ts: u64,
    kind: String,
    #[serde(default)]
    service: Option<serde_json::Value>,
    #[serde(default)]
    digest: Option<serde_json::Value>,
    #[serde(default)]
    backend: Option<serde_json::Value>,
    #[serde(default)]
    reason: Option<serde_json::Value>,
    #[serde(default)]
    outcome: Option<serde_json::Value>,
    #[serde(default)]
    interval_ms: Option<serde_json::Value>,
    #[serde(default)]
    off: Option<serde_json::Value>,
}

/// A string field of a known kind; `None` when absent or not a string.
fn string(v: Option<serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::String(s) => Some(s),
        _ => None,
    }
}

/// Parse a JSON payload; `None` when it is malformed.
fn parse_payload(payload: &[u8]) -> Option<Message> {
    let wire: WireIn = serde_json::from_slice(payload).ok()?;
    if wire.boot.len() != 16 || !wire.boot.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let kind = match wire.kind.as_str() {
        "heartbeat" => Kind::Heartbeat(match wire.off {
            None => Vec::new(),
            Some(serde_json::Value::Array(off)) => off
                .iter()
                .map(|v| v.as_str().and_then(parse_tag))
                .collect::<Option<Vec<u32>>>()?,
            Some(_) => return None,
        }),
        "down" => Kind::Down(Down {
            service: string(wire.service)?,
            digest: string(wire.digest)?,
            backend: string(wire.backend)?,
            reason: string(wire.reason)?,
        }),
        "result" => Kind::Result(ResultMsg {
            service: string(wire.service)?,
            digest: string(wire.digest)?,
            backend: string(wire.backend)?,
            outcome: match string(wire.outcome)?.as_str() {
                "up" => Outcome::Up,
                "down" => Outcome::Down(string(wire.reason)?),
                _ => return None,
            },
            interval_ms: wire
                .interval_ms
                .as_ref()
                .and_then(serde_json::Value::as_u64)
                .filter(|ms| *ms > 0)?,
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

/// The JSON payload of a datagram of either format that `secret` signed
/// (`EMP1`) or encrypted (`EMP2`); `None` otherwise. For tests and tools
/// that read what members send.
pub fn open_with(data: &[u8], secret: &[u8]) -> Option<Vec<u8>> {
    let keys = Keys::new(secret.to_vec());
    let now = Instant::now();
    match data.get(..4) {
        Some(m) if m == MAGIC && (HEADER_LEN..=MAX_DATAGRAM).contains(&data.len()) => keys
            .verifies(&data[HEADER_LEN..], &data[MAGIC.len()..HEADER_LEN], now)
            .then(|| data[HEADER_LEN..].to_vec()),
        Some(m) if m == MAGIC_V2 && (MIN_DATAGRAM_V2..=MAX_DATAGRAM).contains(&data.len()) => {
            keys.open(data, now)
        }
        _ => None,
    }
}

/// The secret of the `peers` fuzz target and its seeds
/// (`fuzz/seeds/peers.py`).
#[cfg(fuzzing)]
pub const FUZZ_SECRET: &[u8] = &[0x5a; 32];

/// Fuzzing entry point (`fuzz/fuzz_targets/peers.rs`). The first byte picks
/// what the rest is: 0, an `EMP1` datagram; 1, an `EMP2` datagram, opened
/// with [`FUZZ_SECRET`]; 2, a payload, encrypted with [`FUZZ_SECRET`] and
/// then received as an `EMP2` datagram, so that mutations reach the JSON
/// parsing behind the decryption, which a mutated datagram almost never
/// passes.
#[cfg(fuzzing)]
pub fn fuzz_decode(data: &[u8]) {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let datagram = match mode % 3 {
        0 => {
            let _ = decode(rest);
            return;
        }
        1 => rest.to_vec(),
        _ => seal(
            rest,
            &Secret::new(FUZZ_SECRET.to_vec()),
            &Nonce::from([0; NONCE_LEN]),
        ),
    };
    if let Some(payload) = open_with(&datagram, FUZZ_SECRET) {
        let _ = parse_payload(&payload);
    }
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

/// The current secret and the ones it replaced: messages are signed with
/// the replaced one for [`SIGN_PREVIOUS_FOR`] and it is accepted for
/// [`ACCEPT_PREVIOUS_FOR`] after the change.
#[derive(Debug, Clone)]
pub struct Keys {
    current: Secret,
    /// The secret signed with until [`SIGN_PREVIOUS_FOR`] after the time
    /// given, and accepted until [`ACCEPT_PREVIOUS_FOR`] after it.
    previous: Option<(Secret, Instant)>,
    /// Other secrets replaced, with when: each is accepted until
    /// [`ACCEPT_PREVIOUS_FOR`] after it was replaced.
    replaced: Vec<(Secret, Instant)>,
}

impl Keys {
    pub fn new(current: Vec<u8>) -> Keys {
        Keys {
            current: Secret::new(current),
            previous: None,
            replaced: Vec::new(),
        }
    }

    /// The secrets at start (spec peer-group "Peer secret reload"): when
    /// `previous` (from `previous_secret_file`) differs from `current` and
    /// the current secret's file changed `changed_ago` before `now`, less
    /// than [`ACCEPT_PREVIOUS_FOR`] ago, `previous` is used as if the
    /// daemon had been running when the file changed: signed with until
    /// [`SIGN_PREVIOUS_FOR`] after the change, accepted until
    /// [`ACCEPT_PREVIOUS_FOR`] after it. Otherwise only `current` is used.
    pub fn resumed(
        current: Vec<u8>,
        previous: Option<Vec<u8>>,
        changed_ago: Duration,
        now: Instant,
    ) -> Keys {
        let mut keys = Keys::new(current);
        if let Some(previous) = previous
            && previous != keys.current.raw
            && changed_ago < ACCEPT_PREVIOUS_FOR
        {
            // An Instant cannot go back before the clock's origin (shortly
            // after the host booted); the change then counts as now.
            let at = now.checked_sub(changed_ago).unwrap_or(now);
            keys.previous = Some((Secret::new(previous), at));
        }
        keys
    }

    /// The newest secret read from the file.
    pub fn current(&self) -> &[u8] {
        self.current.raw()
    }

    /// The secret to sign or encrypt with at `now`.
    pub fn signing(&self, now: Instant) -> &Secret {
        match &self.previous {
            Some((k, at)) if now.duration_since(*at) < SIGN_PREVIOUS_FOR => k,
            _ => &self.current,
        }
    }

    /// Make `new` the current secret. Returns false when it is the current
    /// one already. While messages are still signed with an earlier secret
    /// (within [`SIGN_PREVIOUS_FOR`] of the last change), they stay signed
    /// with it, now for [`SIGN_PREVIOUS_FOR`] after this change: the
    /// secret this change replaces was never signed with, so members whose
    /// file has not changed yet do not know it.
    pub fn rotate(&mut self, new: Vec<u8>, now: Instant) -> bool {
        if new == self.current.raw {
            return false;
        }
        let old = std::mem::replace(&mut self.current, Secret::new(new));
        self.replaced
            .retain(|(_, at)| now.duration_since(*at) < ACCEPT_PREVIOUS_FOR);
        match &mut self.previous {
            Some((_, at)) if now.duration_since(*at) < SIGN_PREVIOUS_FOR => {
                *at = now;
                self.replaced.push((old, now));
            }
            previous => {
                if let Some(earlier) = previous.replace((old, now))
                    && now.duration_since(earlier.1) < ACCEPT_PREVIOUS_FOR
                {
                    self.replaced.push(earlier);
                }
            }
        }
        true
    }

    /// The secrets a received message may use at `now`: the current one,
    /// and each replaced one for [`ACCEPT_PREVIOUS_FOR`].
    fn accepted(&self, now: Instant) -> impl Iterator<Item = &Secret> {
        let recent =
            move |(_, at): &&(Secret, Instant)| now.duration_since(*at) < ACCEPT_PREVIOUS_FOR;
        std::iter::once(&self.current)
            .chain(self.previous.iter().filter(recent).map(|(k, _)| k))
            .chain(self.replaced.iter().filter(recent).map(|(k, _)| k))
    }

    /// Constant-time check of `tag` over `MAGIC` and `payload`.
    fn verifies(&self, payload: &[u8], tag: &[u8], now: Instant) -> bool {
        self.accepted(now).any(|k| {
            let mut m = mac(k.raw());
            m.update(MAGIC);
            m.update(payload);
            m.verify_slice(tag).is_ok()
        })
    }

    /// The payload of an `EMP2` datagram of a valid length, or `None` when
    /// no accepted secret decrypts it.
    fn open(&self, data: &[u8], now: Instant) -> Option<Vec<u8>> {
        let nonce = Nonce::try_from(&data[MAGIC_V2.len()..MAGIC_V2.len() + NONCE_LEN]).ok()?;
        let sealed = &data[MAGIC_V2.len() + NONCE_LEN..];
        self.accepted(now).find_map(|k| {
            k.cipher()
                .decrypt(
                    &nonce,
                    Payload {
                        msg: sealed,
                        aad: MAGIC_V2,
                    },
                )
                .ok()
        })
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

/// How many of a sender's latest `seq` numbers are remembered, so that a
/// message overtaken on the network is still accepted once.
const REPLAY_WINDOW: u64 = 64;

/// Replay state of one sender: its latest `boot`, the highest `seq`
/// accepted with it, the highest `ts`, and which of the
/// [`REPLAY_WINDOW`] numbers up to that `seq` were accepted (bit `i`:
/// `seq - i`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Last {
    boot: u64,
    seq: u64,
    ts: u64,
    window: u64,
}

impl Last {
    /// Whether `msg` is not a replay: with the same `boot`, a `seq` above
    /// the highest, or within the window and not accepted yet; with
    /// another `boot` (the sender restarted), a later `ts`.
    fn admits(&self, msg: &Message) -> bool {
        if msg.boot != self.boot {
            return msg.ts > self.ts;
        }
        if msg.seq > self.seq {
            return true;
        }
        let back = self.seq - msg.seq;
        back < REPLAY_WINDOW && self.window & (1 << back) == 0
    }

    /// The state after accepting `msg`, which [`Last::admits`] allowed.
    fn after(last: Option<Last>, msg: &Message) -> Last {
        match last {
            Some(l) if l.boot == msg.boot => {
                if msg.seq > l.seq {
                    let shift = msg.seq - l.seq;
                    let window = if shift >= REPLAY_WINDOW {
                        0
                    } else {
                        l.window << shift
                    };
                    Last {
                        boot: l.boot,
                        seq: msg.seq,
                        ts: l.ts.max(msg.ts),
                        window: window | 1,
                    }
                } else {
                    Last {
                        ts: l.ts.max(msg.ts),
                        window: l.window | 1 << (l.seq - msg.seq),
                        ..l
                    }
                }
            }
            _ => Last {
                boot: msg.boot,
                seq: msg.seq,
                ts: msg.ts,
                window: 1,
            },
        }
    }
}

/// What we know about one other member.
#[derive(Debug, Default)]
struct PeerState {
    /// Replay state, from the messages accepted so far.
    last: Option<Last>,
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
    /// Sources last warned about for sending `EMP1` while `encrypt` is on.
    plain_warned: HashMap<SocketAddr, Instant>,
    /// `[peers] encrypt`: send `EMP2` rather than `EMP1`.
    encrypt: bool,
    stats: Arc<PeerStats>,
}

/// Whether a warning keyed by `key` is due: not logged in the last
/// [`WARN_EVERY`], and `warned` has room. Records `now` for `key` when it is.
/// Entries older than [`WARN_EVERY`] are dropped once `warned` holds
/// [`MAX_WARN_ENTRIES`], so a flood of sources cannot grow it further.
/// A `member` key (from a configured member's address, at most
/// [`crate::config::MAX_MEMBERS`] × 6 of them) always has room, so such a
/// flood cannot hide the warnings about real members.
fn should_warn<K: Hash + Eq>(
    warned: &mut HashMap<K, Instant>,
    key: K,
    member: bool,
    now: Instant,
) -> bool {
    if warned.len() >= MAX_WARN_ENTRIES {
        warned.retain(|_, at| now.duration_since(*at) < WARN_EVERY);
    }
    let recent = warned
        .get(&key)
        .is_some_and(|at| now.duration_since(*at) < WARN_EVERY);
    if recent || (!member && warned.len() >= MAX_WARN_ENTRIES) {
        return false;
    }
    warned.insert(key, now);
    true
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
            plain_warned: HashMap::new(),
            encrypt: true,
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

    /// `[peers] encrypt`: the format of the next messages sent.
    pub fn set_encrypt(&mut self, encrypt: bool) {
        self.encrypt = encrypt;
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
        let secret = self.keys.signing(now);
        if self.encrypt {
            encode_encrypted(&msg, secret)
        } else {
            encode(&msg, secret.raw())
        }
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
            Ok((msg, format)) => {
                if format == Format::Plain && self.encrypt {
                    self.warn_plaintext(src, now);
                }
                if let Some(p) = self.peers.get_mut(&msg.from) {
                    p.last = Some(Last::after(p.last, &msg));
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
    ) -> Result<(Message, Format), Reject> {
        let (msg, format) = match data.get(..4) {
            Some(m) if m == MAGIC => {
                if !(HEADER_LEN..=MAX_DATAGRAM).contains(&data.len()) {
                    return Err(Reject::Malformed);
                }
                // The tag first: unauthenticated bytes never reach the JSON
                // parser, and a forged datagram counts as `auth`.
                if !self
                    .keys
                    .verifies(&data[HEADER_LEN..], &data[MAGIC.len()..HEADER_LEN], now)
                {
                    return Err(Reject::Auth);
                }
                let msg = parse_payload(&data[HEADER_LEN..]).ok_or(Reject::Malformed)?;
                (msg, Format::Plain)
            }
            Some(m) if m == MAGIC_V2 => {
                if !(MIN_DATAGRAM_V2..=MAX_DATAGRAM).contains(&data.len()) {
                    return Err(Reject::Malformed);
                }
                let payload = self.keys.open(data, now).ok_or(Reject::Auth)?;
                let msg = parse_payload(&payload).ok_or(Reject::Malformed)?;
                (msg, Format::Encrypted)
            }
            _ => return Err(Reject::Malformed),
        };
        if msg.from == self.me || msg.from != src || !self.members.contains(&msg.from) {
            return Err(Reject::Sender);
        }
        if msg.site != self.site {
            return Err(Reject::Site);
        }
        if now_ms.abs_diff(msg.ts) > MAX_SKEW_MS {
            return Err(Reject::Stale);
        }
        if let Some(last) = self.peers.get(&msg.from).and_then(|p| p.last)
            && !last.admits(&msg)
        {
            return Err(Reject::Replay);
        }
        Ok((msg, format))
    }

    /// Warn, at most once per [`WARN_EVERY`] for each source, that a member
    /// still sends `EMP1` while this daemon encrypts.
    fn warn_plaintext(&mut self, src: SocketAddr, now: Instant) {
        // Only accepted messages get here, so `src` is a member.
        if should_warn(&mut self.plain_warned, src, true, now) {
            tracing::warn!(
                peer = %src,
                "peer {src} sends plaintext messages (EMP1); set encrypt = true there"
            );
        }
    }

    fn warn_rejected(&mut self, src: SocketAddr, r: Reject, now: Instant) {
        let member = self.members.contains(&src);
        if should_warn(&mut self.warned, (src, r), member, now) {
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
    /// The check definition digest the failing check ran with.
    pub digest: String,
    pub backend: String,
    pub reason: String,
}

/// An assigned checker's own `up` or `down` result of one check, to be
/// sent to the other members as a `result` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendResult {
    pub service: Arc<str>,
    /// The check definition digest the check ran with.
    pub digest: String,
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
    pub encrypt: bool,
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

    /// Apply new members, secret file, site and `encrypt`.
    pub fn reload(&self, config: &PeersConfig, site: &str) {
        self.settings.send_replace(PeerSettings {
            members: config.members.clone(),
            secret_file: config.secret_file.clone(),
            site: site.to_string(),
            encrypt: config.encrypt,
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
    let keys = start_keys(config, key, Instant::now())?;
    Ok(start_on(
        socket,
        config,
        site,
        keys,
        targets,
        writer,
        services_off,
    ))
}

/// The secrets at start: `current`, read from `secret_file`, and with
/// `previous_secret_file` the secret it replaced, as [`Keys::resumed`]
/// decides from when `secret_file` was last modified.
fn start_keys(config: &PeersConfig, current: Vec<u8>, now: Instant) -> Result<Keys, String> {
    let Some(path) = &config.previous_secret_file else {
        return Ok(Keys::new(current));
    };
    let previous = read_secret(path).map_err(|e| format!("peers.previous_secret_file: {e}"))?;
    let changed = std::fs::metadata(&config.secret_file)
        .and_then(|m| m.modified())
        .map_err(|e| {
            format!(
                "peers.secret_file: cannot read the modification time of {}: {e}",
                config.secret_file.display()
            )
        })?;
    // A modification time ahead of the clock counts as now.
    let changed_ago = changed.elapsed().unwrap_or(Duration::ZERO);
    let keys = Keys::resumed(current, Some(previous), changed_ago, now);
    if keys.previous.is_some() {
        tracing::info!(
            "peer secret file changed {}s ago; messages are signed with the previous secret (previous_secret_file) until {}s and it is accepted until {}s after that change",
            changed_ago.as_secs(),
            SIGN_PREVIOUS_FOR.as_secs(),
            ACCEPT_PREVIOUS_FOR.as_secs()
        );
    }
    Ok(keys)
}

/// Start the peer task on a bound socket. `services_off` are the services
/// with `peers = false`, shown in the first `meta/peers`.
pub fn start_on(
    socket: UdpSocket,
    config: &PeersConfig,
    site: &str,
    keys: Keys,
    targets: Targets,
    writer: Option<mpsc::UnboundedSender<WriterMsg>>,
    services_off: Vec<String>,
) -> PeersHandle {
    let stats = Arc::new(PeerStats::default());
    let quorum = Arc::new(Quorum::new(config.listen));
    quorum.set_off(services_off);
    let boot = rand::rng().random::<u64>();
    let mut group = Group::new(
        config.listen,
        site.to_string(),
        config.members.clone(),
        keys,
        boot,
        stats.clone(),
    );
    group.set_encrypt(config.encrypt);
    let settings = PeerSettings {
        members: config.members.clone(),
        secret_file: config.secret_file.clone(),
        site: site.to_string(),
        encrypt: config.encrypt,
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

/// Re-read the secret file, off the async workers (it is read every
/// second, and a slow file system must not hold up a worker thread), and
/// apply it as [`apply_secret`] does.
async fn refresh_secret(
    group: &mut Group,
    path: &Path,
    warned: &mut Option<Instant>,
    now: Instant,
) {
    let path = path.to_path_buf();
    let read = tokio::task::spawn_blocking(move || read_secret(&path))
        .await
        .unwrap_or_else(|e| Err(format!("cannot read the secret file: {e}")));
    apply_secret(group, read, warned, now);
}

/// Apply a secret just read: a changed valid one becomes current; a missing
/// or invalid one keeps the last valid secret, warning at most once a
/// minute.
fn apply_secret(
    group: &mut Group,
    read: Result<Vec<u8>, String>,
    warned: &mut Option<Instant>,
    now: Instant,
) {
    match read {
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
        Kind::Heartbeat(tags) => quorum.set_member_tags(msg.from, tags),
        Kind::Unknown(_) => {}
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
                refresh_secret(&mut group, &secret_file, &mut secret_warned, now).await;
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
                let data = group.encode(Kind::Heartbeat(quorum.off_tags()), unix_ms(), now);
                send_all(&socket, &group.others(), &data).await;
            }
            Some(ev) = out.recv() => {
                let (service, digest) = match &ev {
                    PeerOut::Down(d) => (&d.service, &d.digest),
                    PeerOut::Result(r) => (&r.service, &r.digest),
                };
                // The digest is the one the check ran with, stamped by the
                // backend task: a reload since then must not relabel it.
                // No digest: the service was not known here yet.
                if digest.is_empty() || targets.digest(service).is_none() {
                    continue; // service removed meanwhile
                }
                let (kind, counter) = match ev {
                    PeerOut::Down(d) => (
                        Kind::Down(Down {
                            service: d.service.to_string(),
                            digest: d.digest,
                            backend: d.backend,
                            reason: d.reason,
                        }),
                        &group.stats().sent,
                    ),
                    PeerOut::Result(r) => (
                        Kind::Result(ResultMsg {
                            service: r.service.to_string(),
                            digest: r.digest,
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
                group.set_encrypt(s.encrypt);
                if s.secret_file != secret_file {
                    secret_file = s.secret_file;
                    refresh_secret(&mut group, &secret_file, &mut secret_warned, Instant::now()).await;
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
            Kind::Heartbeat(Vec::new()),
            down("connection refused"),
            result(Outcome::Up),
            result(Outcome::Down("connect: Connection refused".into())),
        ] {
            let m = msg(B, 0xaaaa_aaaa_aaaa_aaaa, 42, 1_763_000_000_123, kind);
            let data = encode(&m, KEY);
            assert_eq!(&data[..4], MAGIC);
            assert_eq!(decode(&data), Some(m));
        }
        let m = msg("[2001:db8::1]:5302", 1, 1, 1, Kind::Heartbeat(Vec::new()));
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
            let secret = Secret::new(KEY.to_vec());
            let data = encode_encrypted(&m, &secret);
            assert!(data.len() <= MAX_DATAGRAM, "EMP2: {} bytes", data.len());
            assert!(
                open_with(&data, KEY)
                    .and_then(|p| parse_payload(&p))
                    .is_some(),
                "the shortened EMP2 message still decrypts and parses"
            );
        }
    }

    /// Spec peer-group "heartbeat `off` field" table.
    #[test]
    fn heartbeat_off_field() {
        let now = Instant::now();
        let t = 1_763_000_000_000u64;
        let head = format!(
            r#""from":"{B}","site":"dc1","boot":"0000000000000001","seq":1,"ts":{t},"kind":"heartbeat""#
        );
        let cases: [(&str, Result<Vec<u32>, Reject>); 7] = [
            ("", Ok(vec![])),
            (r#","off":[]"#, Ok(vec![])),
            (
                r#","off":["a513a047","fa6f63b4"]"#,
                Ok(vec![0xa513_a047, 0xfa6f_63b4]),
            ),
            (r#","off":["A513A047"]"#, Err(Reject::Malformed)),
            (r#","off":["a513a04"]"#, Err(Reject::Malformed)),
            (r#","off":"a513a047""#, Err(Reject::Malformed)),
            (r#","off":[42]"#, Err(Reject::Malformed)),
        ];
        for (tail, want) in cases {
            let mut g = group(now);
            let got = g
                .accept(&raw(&format!("{{{head}{tail}}}")), addr(B), t, now)
                .map(|m| m.kind);
            assert_eq!(got, want.map(Kind::Heartbeat), "{tail}");
        }
    }

    /// Spec peer-group "Heartbeat announces services left out": tags are
    /// written as 8 lowercase hex digits, and no `off` without tags.
    #[test]
    fn heartbeat_off_on_the_wire() {
        let json = |kind| {
            let data = encode(&msg(B, 1, 1, 1, kind), KEY);
            String::from_utf8(data[HEADER_LEN..].to_vec()).expect("the payload is UTF-8 JSON")
        };
        assert!(!json(Kind::Heartbeat(Vec::new())).contains("off"));
        assert!(
            json(Kind::Heartbeat(vec![0x0000_00ff, 0xa513_a047]))
                .ends_with(r#""kind":"heartbeat","off":["000000ff","a513a047"]}"#)
        );
    }

    /// Spec peer-group "More services left out than fit": the largest
    /// heartbeat, with [`crate::quorum::MAX_OFF_TAGS`] tags, fits one
    /// datagram.
    #[test]
    fn largest_heartbeat_fits_one_datagram() {
        let from = SocketAddr::V6(std::net::SocketAddrV6::new(
            "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"
                .parse()
                .expect("a valid IPv6 address"),
            65535,
            0,
            u32::MAX,
        ));
        let tags = vec![u32::MAX; crate::quorum::MAX_OFF_TAGS];
        let m = Message {
            from,
            site: "s".repeat(63),
            boot: u64::MAX,
            seq: u64::MAX,
            ts: u64::MAX,
            kind: Kind::Heartbeat(tags),
        };
        let data = encode(&m, KEY);
        assert!(data.len() <= MAX_DATAGRAM, "{} bytes", data.len());
        assert_eq!(decode(&data), Some(m.clone()));
        let data = encode_encrypted(&m, &Secret::new(KEY.to_vec()));
        assert!(data.len() <= MAX_DATAGRAM, "EMP2: {} bytes", data.len());
        assert_eq!(open_v2(&data, KEY), Some(m));
    }

    /// The message in an `EMP2` datagram encrypted with `key`.
    fn open_v2(data: &[u8], key: &[u8]) -> Option<Message> {
        Keys::new(key.to_vec())
            .open(data, Instant::now())
            .and_then(|p| parse_payload(&p))
    }

    /// Spec peer-group "Encrypted heartbeat": every kind survives the trip,
    /// and the payload is not readable on the wire.
    #[test]
    fn encrypted_round_trip() {
        let secret = Secret::new(KEY.to_vec());
        for kind in [
            Kind::Heartbeat(vec![1, 0xdeadbeef]),
            down("refused"),
            result(Outcome::Up),
            Kind::Unknown("assign".into()),
        ] {
            let m = msg(B, 0xabc, 7, 1_763_000_000_000, kind);
            let data = encode_encrypted(&m, &secret);
            assert_eq!(&data[..4], MAGIC_V2);
            assert!(!data.windows(3).any(|w| w == b"dc1"), "site in plaintext");
            assert_eq!(open_v2(&data, KEY), Some(m));
        }
        // A new nonce for every message.
        let m = msg(B, 1, 1, 1, Kind::Heartbeat(Vec::new()));
        assert_ne!(encode_encrypted(&m, &secret), encode_encrypted(&m, &secret));
    }

    /// `EMP2` bytes for a fixed secret, nonce and payload, computed apart
    /// from this code by `crates/emfor-daemon/tests/emp2_kat.py`: a
    /// plain-Python HKDF (RFC 5869) and ChaCha20-Poly1305 (RFC 8439), itself
    /// checked against the RFCs' test vectors:
    /// key = HKDF-SHA256(salt "emfor peers", ikm 32 bytes of 7,
    /// info "EMP2 chacha20-poly1305"), nonce 01..0c, AAD "EMP2".
    #[test]
    fn encrypted_known_answer() {
        let secret = Secret::new(KEY.to_vec());
        assert_eq!(
            hex(&secret.aead),
            "1a8cfa78ce5c9f17376137c9bca6b8c67423e5ac4e5d4f0b23dde76d2e0d6e5c"
        );
        let payload = br#"{"from":"10.0.0.6:5302","site":"dc1","boot":"0000000000000001","seq":1,"ts":1763000000000,"kind":"heartbeat"}"#;
        let nonce = Nonce::from([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        let data = seal(payload, &secret, &nonce);
        assert_eq!(
            hex(&data),
            "454d50320102030405060708090a0b0c39106b94d7eceaad0107bea6d91c09c5e1a12774471db156617651d0910ffdb4d8a720346df809e69b5c6de9736f8985eccdfe454f6199f8fe5a600538f728575a067ca2018869b008b196b0526fd0b74119683f48abc64bfef0e2b589b740fa74add1e9a84795dc9ff89f18f5110df7d5f677fcab9f39121be20ab015"
        );
        let mut g = group(Instant::now());
        assert!(
            g.accept(&data, addr(B), 1_763_000_000_000, Instant::now())
                .is_ok()
        );
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// Spec peer-group "acceptance checks", the `EMP2` rows.
    #[test]
    fn encrypted_acceptance_checks() {
        let now = Instant::now();
        let t = 1_763_000_000_000u64;
        let secret = Secret::new(KEY.to_vec());
        let hb = |seq| encode_encrypted(&msg(B, 1, seq, t, Kind::Heartbeat(Vec::new())), &secret);
        let flip = |mut d: Vec<u8>, i: usize| {
            d[i] ^= 1;
            d
        };
        let sized = |n: usize| {
            let mut d = MAGIC_V2.to_vec();
            d.resize(n, 0x41);
            d
        };
        let nonce = Nonce::from([9; 12]);
        let not_json = seal(b"not json at all", &secret, &nonce);
        let no_aad = {
            let sealed = secret
                .cipher()
                .encrypt(&nonce, br#"{"from":"10.0.0.6:5302"}"#.as_ref())
                .expect("encrypting a short payload works");
            [MAGIC_V2.as_slice(), nonce.as_slice(), &sealed].concat()
        };
        let mut emp3 = hb(1);
        emp3[3] = b'3';
        let cases: Vec<(&str, Vec<u8>, Result<(), Reject>)> = vec![
            ("valid", hb(1), Ok(())),
            ("33 bytes", sized(33), Err(Reject::Malformed)),
            ("34 bytes of garbage", sized(34), Err(Reject::Auth)),
            ("1200 bytes of garbage", sized(1200), Err(Reject::Auth)),
            ("1201 bytes", sized(1201), Err(Reject::Malformed)),
            ("nonce changed", flip(hb(1), 4), Err(Reject::Auth)),
            ("ciphertext changed", flip(hb(1), 20), Err(Reject::Auth)),
            (
                "tag changed",
                {
                    let d = hb(1);
                    let last = d.len() - 1;
                    flip(d, last)
                },
                Err(Reject::Auth),
            ),
            ("sealed without the magic as AAD", no_aad, Err(Reject::Auth)),
            (
                "other secret",
                encode_encrypted(
                    &msg(B, 1, 1, t, Kind::Heartbeat(Vec::new())),
                    &Secret::new(vec![8u8; 32]),
                ),
                Err(Reject::Auth),
            ),
            ("decrypts to non-JSON", not_json, Err(Reject::Malformed)),
            ("EMP3", emp3, Err(Reject::Malformed)),
        ];
        for (name, data, want) in cases {
            let mut g = group(now);
            assert_eq!(g.accept(&data, addr(B), t, now).map(|_| ()), want, "{name}");
        }
    }

    /// The replaced secret is accepted for 120 seconds in `EMP2` too.
    #[test]
    fn previous_secret_decrypts_for_120_seconds() {
        let start = Instant::now();
        let t = 1_763_000_000_000u64;
        let mut g = group(start);
        assert!(g.keys.rotate(vec![9u8; 32], start));
        let old = Secret::new(KEY.to_vec());
        let hb =
            |seq, s: &Secret| encode_encrypted(&msg(B, 1, seq, t, Kind::Heartbeat(Vec::new())), s);
        for (secs, seq) in [(30, 1), (90, 2)] {
            let at = start + Duration::from_secs(secs);
            assert!(g.accept(&hb(seq, &old), addr(B), t, at).is_ok(), "{secs}s");
        }
        let at130 = start + Duration::from_secs(130);
        assert_eq!(
            g.accept(&hb(3, &old), addr(B), t, at130).map(|_| ()),
            Err(Reject::Auth)
        );
        assert!(
            g.accept(&hb(4, &Secret::new(vec![9u8; 32])), addr(B), t, at130)
                .is_ok()
        );
    }

    /// Every seed of the `peers` fuzz target (`fuzz/seeds/peers.py`) is a
    /// valid message in the form its first byte picks, so the fuzzer starts
    /// past the tag and the decryption.
    #[test]
    fn fuzz_seeds_are_valid_messages() {
        let secret = [0x5au8; 32];
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/peers");
        let mut seen = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("fuzz/seeds/peers exists") {
            let path = entry.expect("the seed directory can be listed").path();
            let data = std::fs::read(&path).expect("a seed can be read");
            let (mode, rest) = data.split_first().expect("a seed is not empty");
            let payload = match mode {
                0 => {
                    assert!(decode(rest).is_some(), "{}", path.display());
                    open_with(rest, &secret)
                }
                1 => open_with(rest, &secret),
                _ => Some(rest.to_vec()),
            };
            assert!(
                payload.and_then(|p| parse_payload(&p)).is_some(),
                "{}",
                path.display()
            );
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                seen.push(name.to_string());
            }
        }
        for kind in ["heartbeat", "down", "result"] {
            for form in ["emp1", "emp2", "payload"] {
                let name = format!("{kind}-{form}");
                assert!(seen.contains(&name), "seed {name} is missing");
            }
        }
    }

    /// `encrypt` picks the format sent.
    #[test]
    fn encrypt_picks_the_format_sent() {
        let now = Instant::now();
        let mut g = group(now);
        assert_eq!(
            &g.encode(Kind::Heartbeat(Vec::new()), 1, now)[..4],
            MAGIC_V2
        );
        g.set_encrypt(false);
        let data = g.encode(Kind::Heartbeat(Vec::new()), 1, now);
        assert_eq!(&data[..4], MAGIC);
        assert!(decode(&data).is_some());
    }

    /// Spec peer-group "Mixed formats during an upgrade": `EMP1` is still
    /// accepted while encrypting, with one warning a minute per source.
    #[test]
    #[tracing_test::traced_test]
    fn plaintext_accepted_and_warned_once_a_minute() {
        let start = Instant::now();
        let t = 1_763_000_000_000u64;
        let mut g = group(start);
        for (i, s) in [0u64, 10, 59, 61].into_iter().enumerate() {
            let data = encode(
                &msg(B, 1, i as u64 + 1, t, Kind::Heartbeat(Vec::new())),
                KEY,
            );
            assert!(
                g.accept(&data, addr(B), t, start + Duration::from_secs(s))
                    .is_ok()
            );
        }
        g.set_encrypt(false);
        let data = encode(&msg(B, 1, 9, t, Kind::Heartbeat(Vec::new())), KEY);
        assert!(
            g.accept(&data, addr(B), t, start + Duration::from_secs(200))
                .is_ok()
        );
        logs_assert(|lines: &[&str]| {
            match lines
                .iter()
                .filter(|l| l.contains("sends plaintext messages (EMP1)"))
                .count()
            {
                2 => Ok(()),
                n => Err(format!("{n} warnings")),
            }
        });
    }

    /// Spec peer-group "Kept tags follow the latest heartbeat": only
    /// heartbeats replace a member's tags, and a member that stops being
    /// live forgets them.
    #[test]
    fn kept_tags_follow_the_latest_heartbeat() {
        let now = Instant::now();
        let g = group(now);
        let targets = Targets::default();
        let quorum = Quorum::new(addr(ME));
        let from = addr(B);
        quorum.set_live([from]);
        let tag = 0xa513_a047;
        let steps = [
            (Kind::Heartbeat(vec![tag]), vec![tag]),
            (result(Outcome::Up), vec![tag]),
            (down("refused"), vec![tag]),
            (Kind::Unknown("assign".into()), vec![tag]),
            (Kind::Heartbeat(Vec::new()), vec![]),
            (Kind::Heartbeat(vec![tag]), vec![tag]),
        ];
        for (seq, (kind, want)) in steps.into_iter().enumerate() {
            let m = msg(B, 1, seq as u64 + 1, 1, kind);
            received(&g, &targets, &quorum, &m, now);
            assert_eq!(quorum.member_tags(from), want, "{:?}", m.kind);
        }
        quorum.set_live([]);
        assert!(quorum.member_tags(from).is_empty());
    }

    /// Spec peer-group "acceptance checks" table.
    #[test]
    fn acceptance_checks() {
        let now = Instant::now();
        let t = 1_763_000_000_000u64;
        let hb = |seq| msg(B, 1, seq, t, Kind::Heartbeat(Vec::new()));
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
                encode(
                    &msg("10.0.0.8:5302", 1, 1, t, Kind::Heartbeat(Vec::new())),
                    KEY,
                ),
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
        let own = encode(&msg(ME, 1, 1, t, Kind::Heartbeat(Vec::new())), KEY);
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
        let old = msg(
            B,
            0xaaaa_aaaa_aaaa_aaaa,
            900,
            t,
            Kind::Heartbeat(Vec::new()),
        );
        assert!(g.accept(&encode(&old, KEY), addr(B), t, now).is_ok());
        let new = msg(
            B,
            0xbbbb_bbbb_bbbb_bbbb,
            1,
            t + 10,
            Kind::Heartbeat(Vec::new()),
        );
        assert!(g.accept(&encode(&new, KEY), addr(B), t + 10, now).is_ok());
        // The old boot replayed afterwards, still within 5s: rejected.
        let replayed = msg(
            B,
            0xaaaa_aaaa_aaaa_aaaa,
            901,
            t + 5,
            Kind::Heartbeat(Vec::new()),
        );
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
        let old = |seq| encode(&msg(B, 1, seq, t, Kind::Heartbeat(Vec::new())), KEY);
        for (secs, seq) in [(30, 1), (90, 2)] {
            let at = start + Duration::from_secs(secs);
            assert!(g.accept(&old(seq), addr(B), t, at).is_ok(), "{secs}s");
        }
        let at130 = start + Duration::from_secs(130);
        assert_eq!(
            g.accept(&old(3), addr(B), t, at130).map(|_| ()),
            Err(Reject::Auth)
        );
        let new = encode(&msg(B, 1, 4, t, Kind::Heartbeat(Vec::new())), &[9u8; 32]);
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
            let from_a = a.encode(Kind::Heartbeat(Vec::new()), t, now);
            assert!(
                b.accept(&from_a, addr(ME), t, now).is_ok(),
                "A to B at {secs}s"
            );
            let from_b = b.encode(Kind::Heartbeat(Vec::new()), t, now);
            assert!(
                a.accept(&from_b, addr(B), t, now).is_ok(),
                "B to A at {secs}s"
            );
        }
        // Signing switches to the new secret 60 seconds after the change.
        let mine = a.encode(
            Kind::Heartbeat(Vec::new()),
            t,
            start + Duration::from_secs(59),
        );
        assert!(Keys::new(KEY.to_vec()).open(&mine, start).is_some());
        let mine = a.encode(
            Kind::Heartbeat(Vec::new()),
            t,
            start + Duration::from_secs(60),
        );
        assert!(Keys::new(new).open(&mine, start).is_some());
    }

    /// Spec peer-group "Restart during a secret change": with
    /// `previous_secret_file`, a daemon restarted 30 seconds after its
    /// file changed signs and accepts as if it had not restarted, so a
    /// member whose file has not changed yet keeps accepting it.
    #[test]
    fn previous_secret_file_survives_a_restart() {
        let new = vec![9u8; 32];
        let (_d, new_path) = secret_file(&new, 0o600);
        let (_d2, old_path) = secret_file(KEY, 0o600);
        let changed = |ago: u64| {
            std::fs::File::options()
                .write(true)
                .open(&new_path)
                .expect("the temporary secret file can be opened")
                .set_modified(std::time::SystemTime::now() - Duration::from_secs(ago))
                .expect("the temporary secret file's time can be set");
        };
        let conf = |previous: Option<PathBuf>| PeersConfig {
            listen: addr(ME),
            members: vec![addr(ME), addr(B)],
            secret_file: new_path.clone(),
            previous_secret_file: previous,
            encrypt: true,
        };
        let now = Instant::now();
        let at = |s| now + Duration::from_secs(s);
        changed(30);
        let keys = start_keys(&conf(Some(old_path.clone())), new.clone(), now)
            .expect("both secret files are valid");
        // A restarted with the new secret, B not changed yet: both ways.
        let t = 1_763_000_000_000u64;
        let mut a = Group::new(
            addr(ME),
            "dc1".into(),
            vec![addr(ME), addr(B)],
            keys.clone(),
            1,
            Arc::new(PeerStats::default()),
        );
        let from_b = encode(&msg(B, 1, 1, t, Kind::Heartbeat(Vec::new())), KEY);
        assert!(a.accept(&from_b, addr(B), t, now).is_ok(), "B to A");
        let from_a = a.encode(Kind::Heartbeat(Vec::new()), t, now);
        let to_b = Group::new(
            addr(B),
            "dc1".into(),
            vec![addr(ME), addr(B)],
            Keys::new(KEY.to_vec()),
            2,
            Arc::new(PeerStats::default()),
        )
        .accept(&from_a, addr(ME), t, now)
        .map(|_| ());
        assert_eq!(to_b, Ok(()), "A to B");
        // Signed with the old secret until 60s after the change, accepted
        // until 120s after it.
        assert_eq!(keys.signing(at(29)).raw(), KEY);
        assert_eq!(keys.signing(at(30)).raw(), &new[..]);
        assert!(keys.accepted(at(89)).any(|k| k.raw() == KEY));
        assert!(!keys.accepted(at(90)).any(|k| k.raw() == KEY));
        // A change long ago, or no previous_secret_file: the new one only.
        changed(200);
        let keys = start_keys(&conf(Some(old_path)), new.clone(), now)
            .expect("both secret files are valid");
        assert_eq!(keys.accepted(now).count(), 1);
        changed(30);
        let keys = start_keys(&conf(None), new.clone(), now).expect("no previous file to read");
        assert_eq!(keys.accepted(now).count(), 1);
        assert_eq!(keys.signing(now).raw(), &new[..]);
    }

    #[test]
    fn secret_file_reload() {
        let start = Instant::now();
        let (_d, path) = secret_file(&[1u8; 40], 0o600);
        let mut g = group(start);
        let mut warned = None;
        apply_secret(&mut g, read_secret(&path), &mut warned, start);
        assert_eq!(g.keys.current(), &[1u8; 40]);
        std::fs::remove_file(&path).expect("the temporary secret file can be removed");
        apply_secret(
            &mut g,
            read_secret(&path),
            &mut warned,
            start + Duration::from_secs(1),
        );
        assert_eq!(g.keys.current(), &[1u8; 40], "keeps the last valid secret");
        assert_eq!(warned, Some(start + Duration::from_secs(1)));
        apply_secret(
            &mut g,
            read_secret(&path),
            &mut warned,
            start + Duration::from_secs(30),
        );
        assert_eq!(
            warned,
            Some(start + Duration::from_secs(1)),
            "one warning a minute"
        );
        apply_secret(
            &mut g,
            read_secret(&path),
            &mut warned,
            start + Duration::from_secs(61),
        );
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

    /// Spec peer-group "acceptance checks": the `EMP1` tag is checked
    /// before the payload is parsed, so a wrong tag on a payload that is not
    /// JSON counts as `auth`, and a right tag on one counts as `malformed`.
    #[test]
    fn emp1_tag_is_checked_before_the_payload() {
        let now = Instant::now();
        let mut bad_tag = MAGIC.to_vec();
        bad_tag.extend_from_slice(&[0u8; TAG_LEN]);
        bad_tag.extend_from_slice(b"not json at all");
        let mut g = group(now);
        assert_eq!(
            g.accept(&bad_tag, addr(B), 0, now).map(|_| ()),
            Err(Reject::Auth)
        );
        assert_eq!(g.stats.rejected(Reject::Auth), 1);
        assert_eq!(g.stats.rejected(Reject::Malformed), 0);
        let mut g = group(now);
        assert_eq!(
            g.accept(&raw("not json at all"), addr(B), 0, now)
                .map(|_| ()),
            Err(Reject::Malformed)
        );
        // Exactly the header: signed empty payload, malformed; 35 bytes:
        // too short.
        let mut g = group(now);
        assert_eq!(
            g.accept(&raw(""), addr(B), 0, now).map(|_| ()),
            Err(Reject::Malformed)
        );
        assert_eq!(
            g.accept(&raw("")[..HEADER_LEN - 1], addr(B), 0, now)
                .map(|_| ()),
            Err(Reject::Malformed)
        );
        assert_eq!(g.stats.rejected(Reject::Auth), 0);
    }

    /// Spec peer-group "Secret changed twice": a second change while the
    /// daemon still signs with the first secret keeps signing with the
    /// first one, for 60 seconds after the second change, and every
    /// secret replaced in the last 120 seconds is accepted.
    #[test]
    fn second_rotation_keeps_signing_with_the_first_secret() {
        let start = Instant::now();
        let at = |s| start + Duration::from_secs(s);
        let (k1, k2) = (vec![8u8; 32], vec![9u8; 32]);
        let mut keys = Keys::new(KEY.to_vec());
        assert!(keys.rotate(k1.clone(), at(0)));
        assert!(keys.rotate(k2.clone(), at(30)));
        assert_eq!(keys.current(), &k2[..]);
        for s in [31, 89] {
            assert_eq!(keys.signing(at(s)).raw(), KEY, "{s}s");
        }
        assert_eq!(keys.signing(at(90)).raw(), &k2[..]);
        let accepted = |keys: &Keys, s| {
            keys.accepted(at(s))
                .map(|k| k.raw().to_vec())
                .collect::<Vec<_>>()
        };
        for want in [KEY.to_vec(), k1.clone(), k2.clone()] {
            assert!(accepted(&keys, 119).contains(&want), "{want:?} at 119s");
        }
        // Both replaced secrets are accepted until 120s after the second
        // change.
        assert_eq!(
            accepted(&keys, 149),
            vec![k2.clone(), KEY.to_vec(), k1.clone()]
        );
        assert_eq!(accepted(&keys, 150), vec![k2.clone()]);
        // A change after signing switched: the secret signed with last
        // is the one kept.
        let mut keys = Keys::new(KEY.to_vec());
        keys.rotate(k1.clone(), at(0));
        keys.rotate(k2.clone(), at(70));
        assert_eq!(keys.signing(at(71)).raw(), &k1[..]);
        assert!(accepted(&keys, 110).contains(&KEY.to_vec()));
        assert!(!accepted(&keys, 121).contains(&KEY.to_vec()));
    }

    /// Spec peer-group: dropped messages are logged once a minute per
    /// source and reason; a flood from many other source addresses does
    /// not use up the room kept for members.
    #[test]
    #[tracing_test::traced_test]
    fn flood_of_sources_does_not_hide_member_warnings() {
        let now = Instant::now();
        let mut g = group(now);
        for port in 0..MAX_WARN_ENTRIES as u16 + 10 {
            let src = SocketAddr::from(([10, 0, 0, 99], 10_000 + port));
            let _ = g.accept(b"short", src, 0, now);
        }
        assert_eq!(
            g.accept(
                &encode(&msg(B, 1, 1, 0, Kind::Heartbeat(Vec::new())), &[8u8; 32]),
                addr(B),
                0,
                now
            )
            .map(|_| ()),
            Err(Reject::Auth)
        );
        logs_assert(|lines: &[&str]| {
            let member = format!("dropped peer message from {B} (auth)");
            match lines.iter().filter(|l| l.contains(&member)).count() {
                1 => Ok(()),
                n => Err(format!("{n} warnings for the member")),
            }
        });
    }

    /// Spec peer-group "Messages arrive out of order": a message with a
    /// lower `seq` than the last accepted one of the same `boot` is
    /// accepted once while it is among the 64 latest.
    #[test]
    fn reordered_messages_are_accepted_once() {
        let now = Instant::now();
        let t = 1_763_000_000_000u64;
        let hb = |seq| encode(&msg(B, 1, seq, t, Kind::Heartbeat(Vec::new())), KEY);
        let mut g = group(now);
        let mut send = |seq| g.accept(&hb(seq), addr(B), t, now).map(|_| ());
        assert_eq!(send(3), Ok(()));
        assert_eq!(send(1), Ok(()), "arrives after 3");
        assert_eq!(send(1), Err(Reject::Replay));
        assert_eq!(send(3), Err(Reject::Replay));
        assert_eq!(send(2), Ok(()));
        assert_eq!(send(70), Ok(()));
        assert_eq!(send(6), Err(Reject::Replay), "64 behind");
        assert_eq!(send(7), Ok(()), "63 behind");
        assert_eq!(send(7), Err(Reject::Replay));
        assert_eq!(send(200), Ok(()));
        assert_eq!(send(70), Err(Reject::Replay));
        assert_eq!(send(199), Ok(()));
    }

    /// Spec peer-group: fields of a kind the receiver does not know are
    /// not checked, even when a known kind uses the same name.
    #[test]
    fn unknown_kind_fields_are_not_checked() {
        let now = Instant::now();
        let t = 1_763_000_000_000u64;
        let json = format!(
            r#"{{"from":"{B}","site":"dc1","boot":"0000000000000001","seq":1,"ts":{t},"kind":"assign","service":42,"off":"x","interval_ms":"soon"}}"#
        );
        let mut g = group(now);
        assert_eq!(
            g.accept(&raw(&json), addr(B), t, now).map(|m| m.kind),
            Ok(Kind::Unknown("assign".into()))
        );
        // A known kind still checks them.
        let json = json.replace("assign", "heartbeat");
        let mut g = group(now);
        assert_eq!(
            g.accept(&raw(&json), addr(B), t, now).map(|_| ()),
            Err(Reject::Malformed)
        );
    }

    #[test]
    fn liveness_and_members() {
        let start = Instant::now();
        let t = 1_763_000_000_000u64;
        let mut g = group(start);
        assert_eq!(g.stats.up(), vec![(addr(B), false)]);
        assert!(
            g.accept(
                &encode(&msg(B, 1, 1, t, Kind::Heartbeat(Vec::new())), KEY),
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
            previous_secret_file: None,
            encrypt: true,
        };
        let ha = start_on(
            sa,
            &conf(a),
            "dc1",
            Keys::new(vec![3u8; 32]),
            targets_a,
            None,
            Vec::new(),
        );
        let hb = start_on(
            sb,
            &conf(b),
            "dc1",
            Keys::new(vec![3u8; 32]),
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
                previous_secret_file: None,
                encrypt: true,
            },
            "dc1",
        );
        let mut buf = [0u8; 1500];
        let (n, from) = tokio::time::timeout(Duration::from_secs(2), sc.recv_from(&mut buf))
            .await
            .expect("c receives a heartbeat within 2s")
            .expect("receive works");
        assert_eq!(from, ha.listen);
        assert_eq!(&buf[..4], MAGIC_V2, "encrypt is on by default");
        assert!(matches!(
            Keys::new(vec![3u8; 32])
                .open(&buf[..n], Instant::now())
                .and_then(|p| parse_payload(&p)),
            Some(Message {
                kind: Kind::Heartbeat(_),
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
                previous_secret_file: None,
                encrypt: true,
            },
            "dc1",
        );
        let ha = start_on(
            sa,
            &PeersConfig {
                listen: a,
                members: vec![a, hb.listen],
                secret_file: path,
                previous_secret_file: None,
                encrypt: true,
            },
            "dc1",
            Keys::new(vec![3u8; 32]),
            ta,
            None,
            Vec::new(),
        );
        ha.out
            .send(PeerOut::Down(BackendDown {
                service: "api.svc.emfor".into(),
                digest: DIGEST.into(),
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
                    digest: DIGEST.into(),
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
                digest: "f".repeat(32),
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
                digest: DIGEST.into(),
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
