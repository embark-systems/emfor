//! The peer group as this daemon sees it (spec peer-group "Peer message
//! acceptance" and "Peer heartbeats and liveness"): the acceptance checks,
//! replay state and member liveness, without I/O.

use std::collections::HashMap;
use std::hash::Hash;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::time::Instant;

use super::crypto::{Keys, encode, encode_encrypted};
use super::stats::{PeerStats, Reject};
use super::wire::{
    Format, HEADER_LEN, Kind, MAGIC, MAGIC_V2, MAX_DATAGRAM, MIN_DATAGRAM_V2, Message,
    parse_payload,
};
use crate::quorum::Quorum;

/// A member is live while a message from it was accepted this recently.
pub const LIVE_WINDOW: Duration = Duration::from_secs(3);
/// Largest accepted difference between a message's `ts` and our clock.
pub const MAX_SKEW_MS: u64 = 5000;

/// At most one warning per source address and reason in this time.
pub(super) const WARN_EVERY: Duration = Duration::from_secs(60);
/// Rate-limit entries kept before old ones are dropped.
const MAX_WARN_ENTRIES: usize = 1024;

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
/// and liveness. No I/O; [`run`](super::task::run) drives it.
///
/// The live members are kept here only: whenever a member becomes live or
/// stops being live, or the members change, [`Group::publish_live`] hands
/// them to [`PeerStats`] (for `/metrics`) and to the [`Quorum`] given with
/// [`Group::share_live_with`] (for the assigned checkers).
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
    /// Where the live members go besides `stats`; see [`Group::share_live_with`].
    quorum: Option<Arc<Quorum>>,
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
            quorum: None,
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
        self.publish_live();
    }

    /// Keep the live members of `quorum` equal to this group's from now
    /// on.
    pub fn share_live_with(&mut self, quorum: Arc<Quorum>) {
        self.quorum = Some(quorum);
        self.publish_live();
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
                        self.publish_live();
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
            self.publish_live();
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

    /// Hand the members' liveness to everything that keeps a copy: the
    /// only place they are updated from.
    fn publish_live(&self) {
        self.stats
            .set_up(self.peers.iter().map(|(a, p)| (*a, p.live)));
        if let Some(q) = &self.quorum {
            q.set_live(self.live_members());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peers::crypto::{Secret, seal};
    use crate::peers::test_support::*;
    use crate::peers::wire::{TAG_LEN, decode};
    use chacha20poly1305::Nonce;
    use chacha20poly1305::aead::Aead;

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

    /// The quorum's live members follow the group's: when one becomes
    /// live, stops being live, or is removed on reload.
    #[test]
    fn quorum_follows_liveness() {
        let start = Instant::now();
        let t = 1_763_000_000_000u64;
        let c = "10.0.0.7:5302";
        let mut g = Group::new(
            addr(ME),
            "dc1".into(),
            vec![addr(ME), addr(B), addr(c)],
            Keys::new(KEY.to_vec()),
            1,
            Arc::new(PeerStats::default()),
        );
        let quorum = Arc::new(Quorum::new(addr(ME)));
        quorum.set_live([addr(B)]);
        g.share_live_with(quorum.clone());
        let checkers = || quorum.checkers("api.svc.emfor", "10.0.0.11:8080");
        assert!(checkers().is_empty(), "nobody live yet");
        for from in [B, c] {
            let hb = encode(&msg(from, 1, 1, t, Kind::Heartbeat(Vec::new())), KEY);
            assert!(g.accept(&hb, addr(from), t, start).is_ok());
        }
        let mut all = checkers();
        all.sort();
        assert_eq!(all, vec![addr(ME), addr(B), addr(c)]);
        g.set_members(vec![addr(ME), addr(B)]);
        assert!(checkers().is_empty(), "C removed");
        g.set_members(vec![addr(ME), addr(B), addr(c)]);
        let hb = encode(&msg(c, 1, 2, t, Kind::Heartbeat(Vec::new())), KEY);
        assert!(g.accept(&hb, addr(c), t, start).is_ok());
        assert_eq!(checkers().len(), 3);
        g.check_liveness(start + LIVE_WINDOW + Duration::from_millis(1));
        assert!(checkers().is_empty(), "neither is live");
    }
}
