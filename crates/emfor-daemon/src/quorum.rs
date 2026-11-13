//! Region verdict of a peer group (spec peer-group, "Assigned checkers and
//! check results" and "Region verdict and combined result").
//!
//! Every backend has up to three assigned checkers, chosen by rendezvous
//! hashing among this daemon and the members that are live. Assigned
//! checkers send their own results; after each of its own checks a daemon
//! counts the fresh votes and combines them with its own result: two `down`
//! votes make the backend down here as well, whatever this daemon's own
//! check says. With too few candidates or no fresh vote the daemon's own
//! result stands unchanged.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use md5::{Digest, Md5};
use serde::Serialize;
use tokio::time::Instant;

use crate::checker::{CheckResult, normalize_reason};
use crate::peers::{Outcome, ResultMsg};

/// Assigned checkers per backend.
pub const ASSIGNED: usize = 3;
/// `down` votes that make the region verdict down: a majority of
/// [`ASSIGNED`].
pub const REGION_DOWN_VOTES: u32 = 2;
/// A kept result is a vote for this many of its sender's intervals.
pub const FRESH_INTERVALS: u32 = 3;
/// A digest mismatch is shown until no message showing it arrived for this
/// long.
pub const MISMATCH_KEEP: Duration = Duration::from_secs(60);
/// Most service tags one heartbeat announces (spec peer-group, "Peer
/// heartbeats and liveness"): with the longest `from`, `site` and `seq`
/// the datagram then still fits in [`crate::peers::MAX_DATAGRAM`].
pub const MAX_OFF_TAGS: usize = 80;

/// The service tag of `service`: the first 4 bytes, big endian, of
/// SHA-256 over its name. Heartbeats write it as 8 lowercase hex digits.
pub fn service_tag(service: &str) -> u32 {
    let d = sha2::Sha256::digest(service.as_bytes());
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}

/// Rendezvous value of `member` for one backend: the first 4 bytes, big
/// endian, of MD5(`<service>/<backend>/<member>`), as sticky does.
pub fn rendezvous(service: &str, backend: &str, member: SocketAddr) -> u32 {
    let mut h = Md5::new();
    h.update(format!("{service}/{backend}/{member}").as_bytes());
    let d = h.finalize();
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}

/// Rank order of two candidates given as (rendezvous value, listen
/// string): larger values first, ties to the smaller listen string.
fn rank_order(a: (u32, &str), b: (u32, &str)) -> std::cmp::Ordering {
    b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1))
}

/// The assigned checkers of a backend in rank order: the [`ASSIGNED`]
/// candidates (`me` and the live members) with the largest rendezvous
/// values, ties going to the smaller listen string. Empty with fewer
/// candidates.
pub fn assigned(
    service: &str,
    backend: &str,
    me: SocketAddr,
    live: &BTreeSet<SocketAddr>,
) -> Vec<SocketAddr> {
    let mut candidates: Vec<(u32, String, SocketAddr)> = std::iter::once(me)
        .chain(live.iter().copied().filter(|m| *m != me))
        .map(|m| (rendezvous(service, backend, m), m.to_string(), m))
        .collect();
    if candidates.len() < ASSIGNED {
        return Vec::new();
    }
    candidates.sort_by(|a, b| rank_order((a.0, &a.1), (b.0, &b.1)));
    candidates
        .into_iter()
        .take(ASSIGNED)
        .map(|(_, _, m)| m)
        .collect()
}

/// What one check of a backend amounts to in a peer group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combined {
    /// The result that takes the place of this daemon's own result.
    pub result: CheckResult,
    /// This daemon is an assigned checker of the backend, so it sends its
    /// own `up` or `down` result to the other members.
    pub assigned: bool,
    /// `(down votes, fresh votes)` when the region verdict is down while
    /// this daemon's own result was `up`.
    pub disagreement: Option<(u32, u32)>,
}

/// The fresh votes of a backend's assigned checkers other than this daemon
/// (spec peer-group, "Split checks").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Votes {
    /// The backend has assigned checkers.
    pub has_checkers: bool,
    /// This daemon is one of them.
    pub assigned: bool,
    /// Fresh votes of the other assigned checkers.
    pub fresh: u32,
    /// Of those, `down` votes.
    pub down: u32,
    /// Of those, `up` votes.
    pub up: u32,
    /// The reason of the highest-ranked `down` vote.
    pub down_reason: Option<String>,
}

impl Votes {
    /// The region verdict when the votes alone decide it: `down` with at
    /// least [`REGION_DOWN_VOTES`] down votes, `up` with as many up votes,
    /// `None` otherwise.
    pub fn decided(&self) -> Option<CheckResult> {
        if self.down >= REGION_DOWN_VOTES {
            let reason = self.down_reason.as_deref().unwrap_or_default();
            Some(region_down(self.down, self.fresh, reason))
        } else if self.up >= REGION_DOWN_VOTES {
            Some(CheckResult::Up)
        } else {
            None
        }
    }
}

/// `down` with reason `peers <down>/<fresh>: <reason>`.
fn region_down(down: u32, fresh: u32, reason: &str) -> CheckResult {
    CheckResult::Down(normalize_reason(&format!("peers {down}/{fresh}: {reason}")))
}

/// The latest result of one sender for one backend.
#[derive(Debug, Clone)]
struct Kept {
    outcome: Outcome,
    digest: String,
    interval: Duration,
    at: Instant,
}

type BackendKey = (String, String);

#[derive(Debug, Default)]
struct State {
    live: BTreeSet<SocketAddr>,
    votes: HashMap<BackendKey, HashMap<SocketAddr, Kept>>,
    disagreements: BTreeMap<BackendKey, (u32, u32)>,
    mismatches: BTreeMap<(String, SocketAddr), Instant>,
    /// Services with `peers = false`, sorted.
    off: Vec<String>,
    /// The tags this daemon announces: those of `off`, distinct, sorted,
    /// at most [`MAX_OFF_TAGS`].
    off_tags: Vec<u32>,
    /// The service tags of the latest heartbeat of each live member; no
    /// entry when it announced none.
    member_tags: HashMap<SocketAddr, BTreeSet<u32>>,
    /// Every service of this daemon, in or out of the peer group, sorted.
    services: Vec<String>,
}

/// Votes, liveness and the lists `meta/peers` shows; shared by the peer
/// task (which records what arrives) and the backend tasks (which combine
/// after each check).
#[derive(Debug)]
pub struct Quorum {
    me: SocketAddr,
    state: Mutex<State>,
    /// Check times decided by votes alone.
    skipped: AtomicU64,
}

/// `meta/peers` (spec state-store, "Peer group state in the store").
#[derive(Debug, Serialize)]
struct Summary<'a> {
    listen: String,
    members: Vec<SummaryMember>,
    digest_mismatch: Vec<SummaryMismatch<'a>>,
    disagreements: Vec<SummaryDisagreement<'a>>,
    services_off: &'a [String],
}

#[derive(Debug, Serialize)]
struct SummaryMember {
    addr: String,
    live: bool,
    /// This daemon's services whose tag the member announces.
    services_off: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SummaryMismatch<'a> {
    service: &'a str,
    member: String,
}

#[derive(Debug, Serialize)]
struct SummaryDisagreement<'a> {
    service: &'a str,
    backend: &'a str,
    down: u32,
    fresh: u32,
}

/// The vote of `member`'s kept result for a backend: `Some(None)` for up,
/// `Some(Some(reason))` for down, `None` when it has none that is fresh
/// and has `digest`.
fn kept_vote(
    st: &State,
    key: &BackendKey,
    member: SocketAddr,
    digest: &str,
    now: Instant,
) -> Option<Option<String>> {
    let k = st.votes.get(key)?.get(&member)?;
    let fresh = now.duration_since(k.at) < k.interval.saturating_mul(FRESH_INTERVALS);
    if k.digest != digest || !fresh {
        return None;
    }
    Some(match &k.outcome {
        Outcome::Up => None,
        Outcome::Down(r) => Some(r.clone()),
    })
}

/// The live members that are candidates for the assigned checkers of
/// `service`'s backends: those that do not announce its service tag (spec
/// peer-group, "Assigned checkers and check results").
fn candidates<'a>(st: &'a State, service: &str) -> std::borrow::Cow<'a, BTreeSet<SocketAddr>> {
    if st.member_tags.is_empty() {
        return std::borrow::Cow::Borrowed(&st.live);
    }
    let tag = service_tag(service);
    let announces = |m: &SocketAddr| st.member_tags.get(m).is_some_and(|t| t.contains(&tag));
    std::borrow::Cow::Owned(st.live.iter().copied().filter(|m| !announces(m)).collect())
}

impl Quorum {
    /// `me` is this daemon's `[peers] listen`.
    pub fn new(me: SocketAddr) -> Quorum {
        Quorum {
            me,
            state: Mutex::new(State::default()),
            skipped: AtomicU64::new(0),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The services left out of the peer group (`peers = false`), shown
    /// in `meta/peers` as `services_off`.
    pub fn set_off(&self, mut names: Vec<String>) {
        names.sort();
        names.dedup();
        let mut st = self.state();
        if names == st.off {
            return;
        }
        let mut tags: Vec<u32> = names.iter().map(|n| service_tag(n)).collect();
        tags.sort_unstable();
        tags.dedup();
        if tags.len() > MAX_OFF_TAGS {
            let left = tags.len() - MAX_OFF_TAGS;
            let not_sent = if left == 1 {
                "1 tag is not sent".to_string()
            } else {
                format!("{left} tags are not sent")
            };
            tracing::warn!(
                "{} services have peers = false but heartbeats announce at most {MAX_OFF_TAGS} service tags; \
                 {not_sent}, so other members still count this daemon as an assigned checker candidate for those services",
                names.len(),
            );
            tags.truncate(MAX_OFF_TAGS);
        }
        st.off = names;
        st.off_tags = tags;
    }

    /// Every service of this daemon, for naming what members announce in
    /// `meta/peers`.
    pub fn set_services(&self, mut names: Vec<String>) {
        names.sort();
        names.dedup();
        self.state().services = names;
    }

    /// The service tags to announce in heartbeats, sorted.
    pub fn off_tags(&self) -> Vec<u32> {
        self.state().off_tags.clone()
    }

    /// The members other than this daemon that are live now.
    pub fn set_live(&self, live: impl IntoIterator<Item = SocketAddr>) {
        let me = self.me;
        let mut st = self.state();
        st.live = live.into_iter().filter(|m| *m != me).collect();
        // A member that stops being live forgets what it announced.
        let State {
            live, member_tags, ..
        } = &mut *st;
        member_tags.retain(|m, _| live.contains(m));
    }

    /// Keep `tags`, from an accepted heartbeat of `member`, in place of what
    /// it announced before (spec peer-group, "Peer heartbeats and
    /// liveness").
    pub fn set_member_tags(&self, member: SocketAddr, tags: &[u32]) {
        let mut st = self.state();
        if tags.is_empty() {
            st.member_tags.remove(&member);
        } else {
            st.member_tags
                .insert(member, tags.iter().copied().collect());
        }
    }

    /// The service tags kept for `member`, sorted.
    #[cfg(test)]
    pub fn member_tags(&self, member: SocketAddr) -> Vec<u32> {
        let st = self.state();
        st.member_tags
            .get(&member)
            .map(|t| t.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Keep `r` as the latest result of `from` for its backend. The caller
    /// checked that this daemon has the service and backend.
    pub fn record(&self, from: SocketAddr, r: &ResultMsg, now: Instant) {
        let kept = Kept {
            outcome: r.outcome.clone(),
            digest: r.digest.clone(),
            interval: Duration::from_millis(r.interval_ms),
            at: now,
        };
        self.state()
            .votes
            .entry((r.service.clone(), r.backend.clone()))
            .or_default()
            .insert(from, kept);
    }

    /// `from` sent a message for `service` with a digest other than ours.
    pub fn note_mismatch(&self, service: &str, from: SocketAddr, now: Instant) {
        self.state()
            .mismatches
            .insert((service.to_string(), from), now);
    }

    /// Combine this daemon's own result of a check of `backend` with the
    /// votes of its assigned checkers. `digest` is this daemon's current
    /// digest of `service`.
    pub fn combine(
        &self,
        service: &str,
        backend: &str,
        digest: &str,
        own: &CheckResult,
        now: Instant,
    ) -> Combined {
        let mut st = self.state();
        let key = (service.to_string(), backend.to_string());
        let ranked = assigned(service, backend, self.me, &candidates(&st, service));
        // Votes in rank order: `None` for a down vote's reason means up.
        let votes: Vec<Option<String>> = ranked
            .iter()
            .filter_map(|m| {
                if *m == self.me {
                    return match own {
                        CheckResult::Up => Some(None),
                        CheckResult::Down(r) => Some(Some(r.clone())),
                        CheckResult::Error(_) => None,
                    };
                }
                kept_vote(&st, &key, *m, digest, now)
            })
            .collect();
        let fresh = u32::try_from(votes.len()).unwrap_or(u32::MAX);
        let down = u32::try_from(votes.iter().filter(|v| v.is_some()).count()).unwrap_or(u32::MAX);
        let is_assigned = ranked.contains(&self.me);
        let (result, disagreement) = if fresh == 0 {
            (own.clone(), None)
        } else if down >= REGION_DOWN_VOTES {
            let reason = votes.iter().flatten().next().cloned().unwrap_or_default();
            let result = region_down(down, fresh, &reason);
            let disagreement = matches!(own, CheckResult::Up).then_some((down, fresh));
            (result, disagreement)
        } else {
            let result = match own {
                CheckResult::Down(r) => CheckResult::Down(normalize_reason(&format!("local: {r}"))),
                other => other.clone(),
            };
            (result, None)
        };
        match disagreement {
            Some(counts) => {
                if st.disagreements.insert(key, counts).is_none() {
                    tracing::info!(
                        service = %service,
                        backend = %backend,
                        "backend {backend} of {service} is down at {}/{} peers but up here",
                        counts.0, counts.1,
                    );
                }
            }
            None => {
                st.disagreements.remove(&key);
            }
        }
        Combined {
            result,
            assigned: is_assigned,
            disagreement,
        }
    }

    /// The fresh votes of the assigned checkers of `backend` other than
    /// this daemon, for deciding a check time without checking. Unlike
    /// [`Quorum::combine`] this changes nothing.
    pub fn votes(&self, service: &str, backend: &str, digest: &str, now: Instant) -> Votes {
        let st = self.state();
        let key = (service.to_string(), backend.to_string());
        let ranked = assigned(service, backend, self.me, &candidates(&st, service));
        let votes: Vec<Option<String>> = ranked
            .iter()
            .filter(|m| **m != self.me)
            .filter_map(|m| kept_vote(&st, &key, *m, digest, now))
            .collect();
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let down = count(votes.iter().filter(|v| v.is_some()).count());
        Votes {
            has_checkers: !ranked.is_empty(),
            assigned: ranked.contains(&self.me),
            fresh: count(votes.len()),
            down,
            up: count(votes.len()) - down,
            down_reason: votes.into_iter().flatten().next(),
        }
    }

    /// The assigned checkers of a backend now, in rank order.
    #[cfg(test)]
    pub fn checkers(&self, service: &str, backend: &str) -> Vec<SocketAddr> {
        let st = self.state();
        assigned(service, backend, self.me, &candidates(&st, service))
    }

    /// Count one check time decided by votes alone.
    pub fn skip(&self) {
        self.skipped.fetch_add(1, Ordering::Relaxed);
    }

    /// Check times decided by votes alone since the daemon started.
    pub fn skipped(&self) -> u64 {
        self.skipped.load(Ordering::Relaxed)
    }

    /// Drop everything about services and backends not in `keep`
    /// (service name → backend ids).
    pub fn retain(&self, keep: &HashMap<String, HashSet<String>>) {
        let has = |(s, b): &BackendKey| keep.get(s).is_some_and(|bs| bs.contains(b));
        let mut st = self.state();
        st.votes.retain(|k, _| has(k));
        st.disagreements.retain(|k, _| has(k));
        st.mismatches.retain(|(s, _), _| keep.contains_key(s));
    }

    /// Drop the votes and mismatches of members no longer in `members`.
    pub fn retain_members(&self, members: &[SocketAddr]) {
        let mut st = self.state();
        for v in st.votes.values_mut() {
            v.retain(|m, _| members.contains(m));
        }
        st.mismatches.retain(|(_, m), _| members.contains(m));
        st.live.retain(|m| members.contains(m));
        st.member_tags.retain(|m, _| members.contains(m));
    }

    /// The kept result of `from` for a backend.
    #[cfg(test)]
    pub fn kept(&self, service: &str, backend: &str, from: SocketAddr) -> Option<Outcome> {
        let st = self.state();
        let v = st.votes.get(&(service.to_string(), backend.to_string()))?;
        v.get(&from).map(|k| k.outcome.clone())
    }

    /// Backends that are currently a disagreement.
    pub fn disagreements(&self) -> usize {
        self.state().disagreements.len()
    }

    /// The JSON value of `meta/peers`. `members` are the other members in
    /// config order with whether they are live. Mismatches older than
    /// [`MISMATCH_KEEP`] are dropped first.
    pub fn summary(&self, members: &[(SocketAddr, bool)], now: Instant) -> String {
        let mut st = self.state();
        st.mismatches
            .retain(|_, at| now.duration_since(*at) < MISMATCH_KEEP);
        let mut mismatches: Vec<(&str, String)> = st
            .mismatches
            .keys()
            .map(|(s, m)| (s.as_str(), m.to_string()))
            .collect();
        mismatches.sort();
        let summary = Summary {
            listen: self.me.to_string(),
            members: members
                .iter()
                .map(|(a, live)| SummaryMember {
                    addr: a.to_string(),
                    live: *live,
                    services_off: st.member_tags.get(a).map_or_else(Vec::new, |tags| {
                        st.services
                            .iter()
                            .filter(|s| tags.contains(&service_tag(s)))
                            .cloned()
                            .collect()
                    }),
                })
                .collect(),
            digest_mismatch: mismatches
                .into_iter()
                .map(|(service, member)| SummaryMismatch { service, member })
                .collect(),
            disagreements: st
                .disagreements
                .iter()
                .map(|((service, backend), (down, fresh))| SummaryDisagreement {
                    service,
                    backend,
                    down: *down,
                    fresh: *fresh,
                })
                .collect(),
            services_off: &st.off,
        };
        serde_json::to_string(&summary)
            .expect("a struct of strings, integers and booleans always serializes to JSON")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVC: &str = "api.svc.emfor";
    const DIGEST: &str = "0123456789abcdef0123456789abcdef";

    fn addr(s: &str) -> SocketAddr {
        s.parse()
            .expect("test addresses are valid socket addresses")
    }

    fn members(range: std::ops::RangeInclusive<u8>) -> BTreeSet<SocketAddr> {
        range.map(|i| addr(&format!("10.0.0.{i}:5302"))).collect()
    }

    /// Spec peer-group "rendezvous values for api.svc.emfor".
    #[test]
    fn rendezvous_values() {
        let rows = [
            ("10.0.0.11:8080", "10.0.0.5:5302", 3974634306u32),
            ("10.0.0.11:8080", "10.0.0.8:5302", 3444584285),
            ("10.0.0.11:8080", "10.0.0.6:5302", 1483479382),
            ("10.0.0.11:8080", "10.0.0.7:5302", 1161570282),
            ("10.0.0.11:8080", "10.0.0.9:5302", 367848677),
            ("10.0.0.12:8080", "10.0.0.9:5302", 4163305238),
            ("10.0.0.12:8080", "10.0.0.6:5302", 3934586029),
            ("10.0.0.12:8080", "10.0.0.7:5302", 2645478105),
            ("10.0.0.12:8080", "10.0.0.8:5302", 2352845107),
            ("10.0.0.12:8080", "10.0.0.5:5302", 729780139),
        ];
        for (backend, member, want) in rows {
            assert_eq!(
                rendezvous(SVC, backend, addr(member)),
                want,
                "{backend} {member}"
            );
        }
    }

    /// Spec peer-group "Assigned checkers among five live members", "A
    /// member stops being live" and "Two-member group".
    #[test]
    fn assigned_checkers() {
        let me = addr("10.0.0.5:5302");
        let all = members(6..=9);
        assert_eq!(
            assigned(SVC, "10.0.0.11:8080", me, &all),
            vec![me, addr("10.0.0.8:5302"), addr("10.0.0.6:5302")]
        );
        let mut without_8 = all.clone();
        without_8.remove(&addr("10.0.0.8:5302"));
        assert_eq!(
            assigned(SVC, "10.0.0.11:8080", me, &without_8),
            vec![me, addr("10.0.0.6:5302"), addr("10.0.0.7:5302")]
        );
        assert!(!assigned(SVC, "10.0.0.12:8080", me, &all).contains(&me));
        assert!(assigned(SVC, "10.0.0.11:8080", me, &members(6..=6)).is_empty());
    }

    #[test]
    fn equal_values_go_to_the_smaller_listen_string() {
        // Ties cannot be produced on demand from MD5; check the order
        // assigned() sorts by instead.
        use std::cmp::Ordering::Less;
        assert_eq!(
            rank_order((7, "10.0.0.10:5302"), (7, "10.0.0.9:5302")),
            Less
        );
        assert_eq!(
            rank_order((8, "10.0.0.9:5302"), (7, "10.0.0.10:5302")),
            Less
        );
    }

    const BACKEND: &str = "10.0.0.11:8080";

    /// A quorum at 10.0.0.5 with 10.0.0.6–9 live; for backend
    /// 10.0.0.11:8080 the assigned checkers are .5 (this daemon), .8, .6.
    fn quorum() -> Quorum {
        let q = Quorum::new(addr("10.0.0.5:5302"));
        q.set_live(members(6..=9));
        q
    }

    fn result(outcome: Outcome) -> ResultMsg {
        ResultMsg {
            service: SVC.into(),
            digest: DIGEST.into(),
            backend: BACKEND.into(),
            outcome,
            interval_ms: 2000,
        }
    }

    fn down(r: &str) -> Outcome {
        Outcome::Down(r.into())
    }

    /// Spec peer-group "combined result" table, row by row. When this daemon
    /// (.5) is assigned the backend is 10.0.0.11:8080 (assigned .5, .8, .6);
    /// otherwise 10.0.0.12:8080 (assigned .9, .6, .7). Kept votes come from
    /// the other assigned checkers in rank order.
    #[test]
    fn combined_result_table() {
        let now = Instant::now();
        struct Row {
            live: std::ops::RangeInclusive<u8>,
            me_assigned: bool,
            kept: Vec<Outcome>,
            own: CheckResult,
            want: CheckResult,
        }
        let refused = || down("refused");
        let row = |me_assigned, kept, own, want| Row {
            live: 6..=9,
            me_assigned,
            kept,
            own,
            want,
        };
        let rows = [
            // — (2 candidates, no assigned checkers)
            Row {
                live: 6..=6,
                me_assigned: true,
                kept: vec![refused()],
                own: CheckResult::Down("timeout".into()),
                want: CheckResult::Down("timeout".into()),
            },
            row(
                false,
                vec![],
                CheckResult::Down("timeout".into()),
                CheckResult::Down("timeout".into()),
            ),
            row(
                true,
                vec![refused(), refused()],
                CheckResult::Up,
                CheckResult::Down("peers 2/3: refused".into()),
            ),
            row(
                false,
                vec![refused(), down("reset")],
                CheckResult::Up,
                CheckResult::Down("peers 2/2: refused".into()),
            ),
            row(
                true,
                vec![refused(), down("reset")],
                CheckResult::Error("plugin exited".into()),
                CheckResult::Down("peers 2/2: refused".into()),
            ),
            row(
                true,
                vec![Outcome::Up, Outcome::Up],
                CheckResult::Down("timeout".into()),
                CheckResult::Down("local: timeout".into()),
            ),
            row(true, vec![refused()], CheckResult::Up, CheckResult::Up),
            row(false, vec![refused()], CheckResult::Up, CheckResult::Up),
            row(
                false,
                vec![refused(), refused(), Outcome::Up],
                CheckResult::Error("plugin exited".into()),
                CheckResult::Down("peers 2/3: refused".into()),
            ),
            row(
                true,
                vec![Outcome::Up, Outcome::Up],
                CheckResult::Error("plugin exited".into()),
                CheckResult::Error("plugin exited".into()),
            ),
        ];
        for (i, row) in rows.into_iter().enumerate() {
            let (backend, others): (&str, &[&str]) = if row.me_assigned {
                (BACKEND, &["10.0.0.8:5302", "10.0.0.6:5302"])
            } else {
                (
                    "10.0.0.12:8080",
                    &["10.0.0.9:5302", "10.0.0.6:5302", "10.0.0.7:5302"],
                )
            };
            let q = Quorum::new(addr("10.0.0.5:5302"));
            q.set_live(members(row.live));
            for (from, outcome) in others.iter().zip(row.kept) {
                let msg = ResultMsg {
                    backend: backend.into(),
                    ..result(outcome)
                };
                q.record(addr(from), &msg, now);
            }
            let got = q.combine(SVC, backend, DIGEST, &row.own, now);
            assert_eq!(got.result, row.want, "row {i}");
        }
    }

    /// Not assigned: the region verdict is a disagreement and `assigned` is
    /// false.
    #[test]
    fn not_assigned_counts_kept_results_only() {
        let now = Instant::now();
        // For 10.0.0.12:8080 the assigned checkers are .9, .6, .7.
        let backend = "10.0.0.12:8080";
        let q = quorum();
        let r = |o| ResultMsg {
            backend: backend.into(),
            ..result(o)
        };
        q.record(addr("10.0.0.9:5302"), &r(down("refused")), now);
        q.record(addr("10.0.0.6:5302"), &r(down("reset")), now);
        let got = q.combine(SVC, backend, DIGEST, &CheckResult::Up, now);
        assert!(!got.assigned);
        assert_eq!(got.result, CheckResult::Down("peers 2/2: refused".into()));
        assert_eq!(got.disagreement, Some((2, 2)));
    }

    /// Spec peer-group "what this daemon does at one check time": the
    /// votes column, for backend 10.0.0.12:8080 (assigned .9, .6, .7; this
    /// daemon .5 is not one).
    #[test]
    fn split_votes_table() {
        let now = Instant::now();
        let backend = "10.0.0.12:8080";
        let senders = ["10.0.0.9:5302", "10.0.0.6:5302", "10.0.0.7:5302"];
        let refused = || down("refused");
        let rows: [(Vec<Outcome>, u32, u32, Option<CheckResult>); 4] = [
            (
                vec![Outcome::Up, Outcome::Up, Outcome::Up],
                3,
                0,
                Some(CheckResult::Up),
            ),
            (
                vec![refused(), refused(), Outcome::Up],
                1,
                2,
                Some(CheckResult::Down("peers 2/3: refused".into())),
            ),
            (vec![Outcome::Up, refused()], 1, 1, None),
            (vec![], 0, 0, None),
        ];
        for (i, (kept, up, dn, decided)) in rows.into_iter().enumerate() {
            let q = quorum();
            for (from, outcome) in senders.iter().zip(kept) {
                let msg = ResultMsg {
                    backend: backend.into(),
                    ..result(outcome)
                };
                q.record(addr(from), &msg, now);
            }
            let v = q.votes(SVC, backend, DIGEST, now);
            assert!(v.has_checkers && !v.assigned, "row {i}");
            assert_eq!((v.up, v.down, v.fresh), (up, dn, up + dn), "row {i}");
            assert_eq!(v.decided(), decided, "row {i}");
        }
        // This daemon is assigned for 10.0.0.11:8080; with one live member
        // there are no assigned checkers.
        assert!(quorum().votes(SVC, BACKEND, DIGEST, now).assigned);
        let q = Quorum::new(addr("10.0.0.5:5302"));
        q.set_live(members(6..=6));
        let v = q.votes(SVC, BACKEND, DIGEST, now);
        assert!(!v.has_checkers && !v.assigned);
        // Votes change nothing; skips are counted.
        assert_eq!(q.disagreements(), 0);
        q.skip();
        q.skip();
        assert_eq!(q.skipped(), 2);
    }

    /// Spec peer-group "which kept results are votes".
    #[test]
    fn which_kept_results_are_votes() {
        let start = Instant::now();
        // Own result down: with one other down vote the verdict is down, so
        // the verdict tells whether .8's kept result was a vote.
        let counts = |q: &Quorum, now| {
            q.combine(SVC, BACKEND, DIGEST, &CheckResult::Down("x".into()), now)
                .result
        };
        let region_down = CheckResult::Down("peers 2/2: x".into());
        let local = CheckResult::Down("local: x".into());
        let at = |s| start + Duration::from_secs(s);
        let cases: [(&str, ResultMsg, u64, bool, CheckResult); 5] = [
            (
                "1s, 2000",
                result(down("y")),
                1,
                true,
                CheckResult::Down("peers 2/2: x".into()),
            ),
            ("7s, 2000", result(down("y")), 7, true, local.clone()),
            (
                "7s, 5000",
                ResultMsg {
                    interval_ms: 5000,
                    ..result(down("y"))
                },
                7,
                true,
                region_down.clone(),
            ),
            (
                "other digest",
                ResultMsg {
                    digest: "f".repeat(32),
                    ..result(down("y"))
                },
                1,
                true,
                local.clone(),
            ),
            (
                "sender not live",
                result(down("y")),
                1,
                false,
                local.clone(),
            ),
        ];
        for (what, r, secs, live, want) in cases {
            let q = Quorum::new(addr("10.0.0.5:5302"));
            if live {
                q.set_live(members(6..=9));
            } else {
                // .8 not live: .5, .6, .7 are assigned and hold no result.
                q.set_live([6, 7, 9].map(|i| addr(&format!("10.0.0.{i}:5302"))));
            }
            q.record(addr("10.0.0.8:5302"), &r, start);
            assert_eq!(counts(&q, at(secs)), want, "{what}");
        }
    }

    #[test]
    fn prefixed_reasons_are_bounded() {
        let q = quorum();
        let own = CheckResult::Down("a".repeat(600));
        q.record(addr("10.0.0.8:5302"), &result(Outcome::Up), Instant::now());
        let got = q.combine(SVC, BACKEND, DIGEST, &own, Instant::now());
        let CheckResult::Down(r) = got.result else {
            panic!("own down with an up vote stays down");
        };
        assert!(r.starts_with("local: "));
        assert_eq!(r.chars().count(), 512);
    }

    /// Disagreements start and end with the combined results, and are
    /// dropped with their backend; mismatches expire after 60 seconds and
    /// go with their member or service.
    #[test]
    #[tracing_test::traced_test]
    fn disagreements_and_mismatches() {
        let start = Instant::now();
        let q = quorum();
        q.record(addr("10.0.0.8:5302"), &result(down("refused")), start);
        q.record(addr("10.0.0.6:5302"), &result(down("refused")), start);
        let got = q.combine(SVC, BACKEND, DIGEST, &CheckResult::Up, start);
        assert_eq!(got.disagreement, Some((2, 3)));
        assert_eq!(q.disagreements(), 1);
        // Again: still one, logged once.
        q.combine(SVC, BACKEND, DIGEST, &CheckResult::Up, start);
        assert!(logs_contain(
            "backend 10.0.0.11:8080 of api.svc.emfor is down at 2/3 peers but up here"
        ));
        logs_assert(|lines: &[&str]| {
            match lines.iter().filter(|l| l.contains("but up here")).count() {
                1 => Ok(()),
                n => Err(format!("{n} lines")),
            }
        });
        // Own down: the verdict is still down but it is no disagreement.
        q.combine(SVC, BACKEND, DIGEST, &CheckResult::Down("t".into()), start);
        assert_eq!(q.disagreements(), 0);
        q.combine(SVC, BACKEND, DIGEST, &CheckResult::Up, start);
        assert_eq!(q.disagreements(), 1);
        // Backend removed.
        q.retain(&HashMap::from([(
            SVC.to_string(),
            HashSet::from(["10.0.0.12:8080".to_string()]),
        )]));
        assert_eq!(q.disagreements(), 0);

        let peers = [(addr("10.0.0.6:5302"), true)];
        let has_mismatch = |q: &Quorum, now| q.summary(&peers, now).contains("10.0.0.7:5302");
        q.note_mismatch(SVC, addr("10.0.0.7:5302"), start);
        assert!(has_mismatch(&q, start + Duration::from_secs(59)));
        assert!(
            !has_mismatch(&q, start + Duration::from_secs(60)),
            "expired"
        );
        q.note_mismatch(SVC, addr("10.0.0.7:5302"), start);
        q.retain_members(&[addr("10.0.0.5:5302"), addr("10.0.0.6:5302")]);
        assert!(!has_mismatch(&q, start), "member removed");
        q.note_mismatch(SVC, addr("10.0.0.6:5302"), start);
        q.retain(&HashMap::new());
        assert!(
            !q.summary(&peers, start).contains("10.0.0.6:5302\"}"),
            "service removed"
        );
    }

    /// Spec state-store "Peer state written at start" and the shape with
    /// both lists filled.
    #[test]
    fn summary_json() {
        let now = Instant::now();
        let q = Quorum::new(addr("127.0.0.1:5302"));
        assert_eq!(
            q.summary(&[(addr("127.0.0.1:5303"), false)], now),
            r#"{"listen":"127.0.0.1:5302","members":[{"addr":"127.0.0.1:5303","live":false,"services_off":[]}],"digest_mismatch":[],"disagreements":[],"services_off":[]}"#
        );
        let q = quorum();
        q.record(addr("10.0.0.8:5302"), &result(down("refused")), now);
        q.record(addr("10.0.0.6:5302"), &result(down("refused")), now);
        q.combine(SVC, BACKEND, DIGEST, &CheckResult::Up, now);
        q.note_mismatch("db.svc.emfor", addr("10.0.0.9:5302"), now);
        q.note_mismatch(SVC, addr("10.0.0.7:5302"), now);
        let peers = [
            (addr("10.0.0.6:5302"), true),
            (addr("[2001:db8::7]:5302"), false),
        ];
        assert_eq!(
            q.summary(&peers, now),
            concat!(
                r#"{"listen":"10.0.0.5:5302","members":[{"addr":"10.0.0.6:5302","live":true,"services_off":[]},{"addr":"[2001:db8::7]:5302","live":false,"services_off":[]}],"#,
                r#""digest_mismatch":[{"service":"api.svc.emfor","member":"10.0.0.7:5302"},{"service":"db.svc.emfor","member":"10.0.0.9:5302"}],"#,
                r#""disagreements":[{"service":"api.svc.emfor","backend":"10.0.0.11:8080","down":2,"fresh":3}],"services_off":[]}"#
            )
        );
        // Spec state-store "Services left out are listed": sorted, no repeats.
        q.set_off(vec![
            "web.svc.emfor".into(),
            "db.svc.emfor".into(),
            "web.svc.emfor".into(),
        ]);
        assert!(
            q.summary(&peers, now)
                .ends_with(r#","services_off":["db.svc.emfor","web.svc.emfor"]}"#)
        );
        q.set_off(vec!["db.svc.emfor".into()]);
        assert!(
            q.summary(&peers, now)
                .ends_with(r#","services_off":["db.svc.emfor"]}"#)
        );
    }

    /// Spec peer-group "service tags".
    #[test]
    fn service_tags() {
        for (name, tag) in [
            ("api.svc.emfor", "a513a047"),
            ("db.svc.emfor", "fa6f63b4"),
            ("web.svc.emfor", "99e6433f"),
        ] {
            assert_eq!(format!("{:08x}", service_tag(name)), tag, "{name}");
        }
    }

    /// Spec peer-group "Heartbeat announces services left out" and "More
    /// services left out than fit": distinct sorted tags, at most 80.
    #[test]
    fn off_tags_sorted_and_limited() {
        let q = Quorum::new(addr("10.0.0.5:5302"));
        assert!(q.off_tags().is_empty());
        q.set_off(vec![
            "db.svc.emfor".into(),
            "api.svc.emfor".into(),
            "api.svc.emfor".into(),
        ]);
        assert_eq!(q.off_tags(), vec![0xa513_a047, 0xfa6f_63b4]);
        let names: Vec<String> = (0..81).map(|i| format!("s{i}.svc.emfor")).collect();
        let mut all: Vec<u32> = names.iter().map(|n| service_tag(n)).collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 81, "the test names have distinct tags");
        q.set_off(names);
        assert_eq!(q.off_tags(), all[..MAX_OFF_TAGS].to_vec());
        q.set_off(Vec::new());
        assert!(q.off_tags().is_empty());
    }

    /// Spec peer-group "A member announces a service left out".
    #[test]
    fn announcing_members_are_not_candidates() {
        let me = addr("10.0.0.5:5302");
        let q = Quorum::new(me);
        q.set_live(members(6..=9));
        let m = |i: u8| addr(&format!("10.0.0.{i}:5302"));
        q.set_member_tags(m(8), &[service_tag(SVC)]);
        assert_eq!(q.checkers(SVC, "10.0.0.11:8080"), vec![me, m(6), m(7)]);
        assert_eq!(q.checkers(SVC, "10.0.0.12:8080"), vec![m(9), m(6), m(7)]);
        // Another service is not affected.
        assert_eq!(
            q.checkers("db.svc.emfor", "10.0.0.11:8080"),
            assigned("db.svc.emfor", "10.0.0.11:8080", me, &members(6..=9))
        );
        // The votes see the same checkers: this daemon is assigned now.
        assert!(
            q.votes(SVC, "10.0.0.11:8080", DIGEST, Instant::now())
                .assigned
        );
        // Two of the four others announcing it leave too few candidates.
        q.set_member_tags(m(6), &[service_tag(SVC)]);
        q.set_member_tags(m(7), &[service_tag(SVC)]);
        assert!(q.checkers(SVC, "10.0.0.11:8080").is_empty());
        // Announcing nothing makes it a candidate again.
        q.set_member_tags(m(6), &[]);
        q.set_member_tags(m(7), &[]);
        q.set_member_tags(m(8), &[]);
        assert_eq!(q.checkers(SVC, "10.0.0.11:8080"), vec![me, m(8), m(6)]);
    }

    /// Two services with the same tag: a member that announces it is left
    /// out of the candidates of both.
    #[test]
    fn same_tag_leaves_the_member_out_of_both() {
        let (a, b) = ("c48024.svc.emfor", "c99378.svc.emfor");
        assert_eq!(service_tag(a), service_tag(b));
        let me = addr("10.0.0.5:5302");
        let q = Quorum::new(me);
        q.set_live(members(6..=8));
        q.set_member_tags(addr("10.0.0.8:5302"), &[service_tag(a)]);
        for s in [a, b] {
            assert!(
                !q.checkers(s, "10.0.0.11:8080")
                    .contains(&addr("10.0.0.8:5302")),
                "{s}"
            );
        }
    }

    /// Spec state-store "Services another member leaves out": names of
    /// this daemon's services only, and nothing once the member is not live.
    #[test]
    fn summary_names_what_members_announce() {
        let now = Instant::now();
        let q = Quorum::new(addr("127.0.0.1:5302"));
        let other = addr("127.0.0.1:5303");
        q.set_services(vec!["db.svc.emfor".into(), "api.svc.emfor".into()]);
        q.set_live([other]);
        q.set_member_tags(
            other,
            &[service_tag("api.svc.emfor"), service_tag("web.svc.emfor")],
        );
        assert!(q.summary(&[(other, true)], now).contains(
            r#""members":[{"addr":"127.0.0.1:5303","live":true,"services_off":["api.svc.emfor"]}]"#
        ));
        q.set_live([]);
        assert!(
            q.summary(&[(other, false)], now).contains(
                r#""members":[{"addr":"127.0.0.1:5303","live":false,"services_off":[]}]"#
            )
        );
    }
}
