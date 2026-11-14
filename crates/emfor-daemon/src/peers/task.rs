//! The peer task: binds the socket, sends heartbeats and what backend
//! tasks hand over, receives and dispatches messages, and follows reloads.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use rand::RngExt;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::crypto::{ACCEPT_PREVIOUS_FOR, Keys, SIGN_PREVIOUS_FOR};
use super::group::{Group, WARN_EVERY};
use super::stats::PeerStats;
use super::targets::{DIGEST_DIFFERS, Targets};
use super::wire::{Down, Kind, MAX_DATAGRAM, Message, Outcome, ResultMsg};
use crate::config::{PeersConfig, read_secret};
use crate::quorum::Quorum;
use crate::schedule::unix_ms;
use crate::writer::WriterMsg;

pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(1);

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
    group.share_live_with(quorum.clone());
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
            let Some(found) = targets.lookup(&r.service, &r.digest, &r.backend) else {
                return;
            };
            if !found.same_digest {
                quorum.note_mismatch(&r.service, msg.from, now);
                tracing::debug!(
                    peer = %msg.from,
                    "result for backend {} of {} from peer {} has another check definition; not counted",
                    r.backend, r.service, msg.from,
                );
            }
            if found.recheck.is_some() {
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
                    // An accepted message makes its sender live, which the
                    // group hands to the quorum itself.
                    if let Ok(msg) = group.accept(&buf[..n], src, unix_ms(), now) {
                        received(&group, &targets, &quorum, &msg, now);
                    }
                }
                Err(e) => tracing::debug!("peer socket receive error: {e}"),
            },
            _ = tick.tick() => {
                let now = Instant::now();
                refresh_secret(&mut group, &secret_file, &mut secret_warned, now).await;
                group.check_liveness(now);
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
    use crate::peers::crypto::encode;
    use crate::peers::targets::ServiceTargets;
    use crate::peers::test_support::*;
    use crate::peers::wire::{MAGIC_V2, parse_payload};
    use std::collections::HashMap;

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
