//! The peer task: binds the socket, sends heartbeats and what backend
//! tasks hand over, receives and dispatches messages, and follows reloads.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use rand::RngExt;
use tokio::net::UdpSocket;
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::crypto::{ACCEPT_PREVIOUS_FOR, Keys, SIGN_PREVIOUS_FOR};
use super::group::{Group, Outgoing, WARN_EVERY};
use super::record;
use super::stats::PeerStats;
use super::targets::{DIGEST_DIFFERS, Targets};
use super::watch::SecretWatch;
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

/// Messages backend tasks can queue for the peer task. Past that, while
/// the peer task cannot keep up (its socket buffer full), new ones are
/// dropped and counted rather than held in memory without bound; a check
/// never waits for the peer task.
pub const PEER_QUEUE: usize = 4096;

/// Where backend tasks queue messages for the peer task.
#[derive(Debug, Clone)]
pub struct PeerSender {
    tx: mpsc::Sender<PeerOut>,
    stats: Arc<PeerStats>,
}

impl PeerSender {
    /// A queue of [`PEER_QUEUE`] messages, counting drops in `stats`.
    pub fn channel(stats: Arc<PeerStats>) -> (PeerSender, mpsc::Receiver<PeerOut>) {
        PeerSender::with_capacity(PEER_QUEUE, stats)
    }

    pub(crate) fn with_capacity(
        capacity: usize,
        stats: Arc<PeerStats>,
    ) -> (PeerSender, mpsc::Receiver<PeerOut>) {
        let (tx, rx) = mpsc::channel(capacity);
        (PeerSender { tx, stats }, rx)
    }

    /// Queue `ev`, or drop and count it when the queue is full. The peer
    /// task is gone only while the daemon stops; then `ev` is dropped.
    pub fn send(&self, ev: PeerOut) {
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(ev) {
            self.stats.count_queue_dropped(Instant::now());
        }
    }
}

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
    pub accept_plaintext: bool,
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

    /// Apply new members, secret file, site, `encrypt` and `accept_plaintext`.
    pub fn reload(&self, config: &PeersConfig, site: &str) {
        self.settings.send_replace(PeerSettings {
            members: config.members.clone(),
            secret_file: config.secret_file.clone(),
            site: site.to_string(),
            encrypt: config.encrypt,
            accept_plaintext: config.accept_plaintext,
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
    store_path: &Path,
    site: &str,
    targets: Targets,
    writer: Option<mpsc::UnboundedSender<WriterMsg>>,
    services_off: Vec<String>,
) -> Result<PeersHandle, String> {
    let socket = UdpSocket::bind(config.listen)
        .await
        .map_err(|e| format!("cannot bind peer address {}: {e}", config.listen))?;
    let key = read_secret(&config.secret_file).map_err(|e| format!("peers.secret_file: {e}"))?;
    let keys = start_keys(config, &record::path_of(store_path), key, Instant::now())?;
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
/// `previous_secret_file` the secret it replaced, when the record of the
/// secrets this daemon used (`record_path`, see [`record`]) says that it
/// replaced that one less than [`ACCEPT_PREVIOUS_FOR`] ago.
fn start_keys(
    config: &PeersConfig,
    record_path: &Path,
    current: Vec<u8>,
    now: Instant,
) -> Result<Keys, String> {
    let previous = match &config.previous_secret_file {
        Some(path) => {
            Some(read_secret(path).map_err(|e| format!("peers.previous_secret_file: {e}"))?)
        }
        None => None,
    };
    let modified = std::fs::metadata(&config.secret_file)
        .and_then(|m| m.modified())
        .map_err(|e| {
            format!(
                "peers.secret_file: cannot read the modification time of {}: {e}",
                config.secret_file.display()
            )
        })?
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let loaded = record::Record::load(record_path);
    let resumed = record::resume(
        loaded.clone(),
        &current,
        previous.as_deref(),
        modified,
        record::unix_now(),
    );
    let file = record::RecordFile::new(record_path.to_path_buf(), resumed.record.clone());
    if loaded.as_ref() != Some(&resumed.record) {
        file.write();
    }
    let changed_ago = resumed.previous.unwrap_or(Duration::ZERO);
    let keys =
        Keys::resumed(current, resumed.previous.and(previous), changed_ago, now).with_record(file);
    if resumed.previous.is_some() {
        tracing::info!(
            "peer secret changed {}s ago; messages are signed with the previous secret (previous_secret_file) until {}s and it is accepted until {}s after that change",
            changed_ago.as_secs(),
            SIGN_PREVIOUS_FOR.as_secs(),
            ACCEPT_PREVIOUS_FOR.as_secs()
        );
    }
    match resumed.ignored {
        Some(record::Ignored::NoRecord) => tracing::info!(
            "peers.previous_secret_file is ignored: there is no record of the secrets this daemon used ({}), as at the first start of this version; it is used from the next change on",
            record_path.display()
        ),
        Some(record::Ignored::NotReplaced) => tracing::info!(
            "peers.previous_secret_file is ignored: it is not the secret this daemon replaced less than {}s ago",
            ACCEPT_PREVIOUS_FOR.as_secs()
        ),
        None => {}
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
    start_with_reader(
        socket,
        config,
        site,
        keys,
        targets,
        writer,
        services_off,
        SecretIo::system(),
    )
}

/// [`start_on`] with the functions that read and watch the secret file; the
/// tests give others.
#[allow(clippy::too_many_arguments)]
fn start_with_reader(
    socket: UdpSocket,
    config: &PeersConfig,
    site: &str,
    keys: Keys,
    targets: Targets,
    writer: Option<mpsc::UnboundedSender<WriterMsg>>,
    services_off: Vec<String>,
    io: SecretIo,
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
    group.set_accept_plaintext(config.accept_plaintext);
    group.share_live_with(quorum.clone());
    let settings = PeerSettings {
        members: config.members.clone(),
        secret_file: config.secret_file.clone(),
        site: site.to_string(),
        encrypt: config.encrypt,
        accept_plaintext: config.accept_plaintext,
    };
    let (settings_tx, settings_rx) = watch::channel(settings);
    let (out_tx, out_rx) = PeerSender::channel(stats.clone());
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
        SecretSource::new(config.secret_file.clone(), io),
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

/// How long a tick waits for a read of the secret file it just started.
/// On a file system that answers, the read is done long before and the new
/// secret is applied on the same tick, before the heartbeats; past this the
/// read stays in flight and the tick goes on with the last valid secret.
const SECRET_READ_WAIT: Duration = Duration::from_millis(200);

/// A read of the secret file in flight for longer than this is warned
/// about (at most once a minute, like any other problem with the file).
const SECRET_READ_SLOW: Duration = Duration::from_secs(5);

/// Reads a secret file; [`read_secret`] outside the tests.
type ReadSecret = fn(&Path) -> Result<Vec<u8>, String>;

/// A read of the secret file on a blocking thread, and when it started.
type ReadInFlight = (JoinHandle<Result<Vec<u8>, String>>, Instant);

/// The re-reading of the secret file, which never holds up the peer loop.
///
/// The file is read on a blocking thread, which cannot be cancelled: a read
/// that does not return (a file on a network file system that does not
/// answer) keeps its thread until it does. So at most one read is in flight,
/// and a new one starts only after the last has finished; a tick waits for
/// a read it starts for [`SECRET_READ_WAIT`] at most.
struct SecretReader {
    read: ReadSecret,
    /// The read in flight and when it started.
    pending: Option<ReadInFlight>,
}

impl SecretReader {
    fn new(read: ReadSecret) -> Self {
        Self {
            read,
            pending: None,
        }
    }

    /// Whether a read is in flight (its result is applied by the next
    /// [`refresh`](Self::refresh)).
    fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    /// Apply the result of the read in flight if it has finished, then
    /// start a new read of `path` unless one is still in flight. A late
    /// result is applied as any other; `path` is the current one each time
    /// a read starts, so a path changed by a reload is read as soon as the
    /// old read is over. Returns whether a read was started: false when the
    /// one in flight is still running.
    async fn refresh(
        &mut self,
        group: &mut Group,
        path: &Path,
        warned: &mut Option<Instant>,
        now: Instant,
    ) -> bool {
        if let Some((handle, started)) = self.pending.take() {
            if !handle.is_finished() {
                let ran = now.duration_since(started);
                if ran > SECRET_READ_SLOW
                    && warned.is_none_or(|w| now.duration_since(w) >= WARN_EVERY)
                {
                    *warned = Some(now);
                    tracing::warn!(
                        "peers.secret_file: the read has not finished after {}s; keeping the last valid secret",
                        ran.as_secs()
                    );
                }
                self.pending = Some((handle, started));
                return false;
            }
            apply_secret(group, joined(handle.await), warned, now);
        }
        let read = self.read;
        let path = path.to_path_buf();
        let mut handle = tokio::task::spawn_blocking(move || read(&path));
        match tokio::time::timeout(SECRET_READ_WAIT, &mut handle).await {
            Ok(done) => apply_secret(group, joined(done), warned, now),
            Err(_) => self.pending = Some((handle, now)),
        }
        true
    }
}

/// The result of a read; a read that panicked or was cancelled is a failed
/// read.
fn joined(
    done: Result<Result<Vec<u8>, String>, tokio::task::JoinError>,
) -> Result<Vec<u8>, String> {
    done.unwrap_or_else(|e| Err(format!("cannot read the secret file: {e}")))
}

/// While the secret file is watched, it is read again this often anyway,
/// so that an event that was lost (a queue that overflowed, a file replaced
/// at the wrong moment) or never made (a network file system) delays a
/// change by this much at most.
const SECRET_SAFETY_READ: Duration = Duration::from_secs(30);

/// An event of the watch is read at once unless the last read started less
/// than this long ago (or is still running); the next heartbeat reads it
/// then. A directory with many events (the watch is of the directory) cannot
/// make a read for each of them.
const SECRET_EVENT_GAP: Duration = Duration::from_millis(100);

/// Starts the watch of a secret file; [`SecretWatch::start`] outside the
/// tests.
type StartWatch = fn(&Path) -> Result<SecretWatch, String>;

/// What reads and what watches the secret file; the tests replace them.
#[derive(Clone, Copy)]
struct SecretIo {
    read: ReadSecret,
    watch: StartWatch,
}

impl SecretIo {
    fn system() -> Self {
        Self {
            read: read_secret,
            watch: SecretWatch::start,
        }
    }
}

/// Keeps the secret of a group current: reads the file when the watch says
/// it may have changed, and on every heartbeat when it is not watched (the
/// watch could not be set up, or stopped), and reads it every
/// [`SECRET_SAFETY_READ`] while it is watched.
struct SecretSource {
    reader: SecretReader,
    start_watch: StartWatch,
    path: PathBuf,
    watch: Option<SecretWatch>,
    /// The watch being set up: opening the directory can block (a network
    /// file system), so it is not waited for longer than
    /// [`SECRET_READ_WAIT`]; until it is ready the file is read on every
    /// heartbeat.
    starting: Option<tokio::task::JoinHandle<Result<SecretWatch, String>>>,
    warned: Option<Instant>,
    last_read: Option<Instant>,
    /// A change was reported but no read has started since: the next
    /// heartbeat reads.
    wanted: bool,
}

impl SecretSource {
    /// Nothing is watched until [`begin`](Self::begin).
    fn new(path: PathBuf, io: SecretIo) -> Self {
        Self {
            reader: SecretReader::new(io.read),
            start_watch: io.watch,
            path,
            watch: None,
            starting: None,
            warned: None,
            last_read: None,
            wanted: false,
        }
    }

    /// Start the watch, before the first read, so that a change between the
    /// two is not missed.
    async fn begin(&mut self) {
        self.watch_path().await;
    }

    /// Watch `self.path`, on a blocking thread that is waited for at most
    /// [`SECRET_READ_WAIT`].
    async fn watch_path(&mut self) {
        self.watch = None;
        // A start still running for the old path is left to finish; its
        // watch is dropped, which stops it.
        self.starting = None;
        let start = self.start_watch;
        let path = self.path.clone();
        let mut handle = tokio::task::spawn_blocking(move || start(&path));
        match tokio::time::timeout(SECRET_READ_WAIT, &mut handle).await {
            Ok(done) => self.watch_started(done),
            Err(_) => {
                tracing::info!(
                    "peers.secret_file: the watch of {} is not ready after {}ms; reading it before every heartbeat until it is",
                    self.path.display(),
                    SECRET_READ_WAIT.as_millis()
                );
                self.starting = Some(handle);
            }
        }
    }

    fn watch_started(&mut self, done: Result<Result<SecretWatch, String>, tokio::task::JoinError>) {
        match done.map_err(|e| e.to_string()).and_then(|r| r) {
            Ok(w) => self.watch = Some(w),
            Err(e) => {
                self.watch = None;
                tracing::info!(
                    "peers.secret_file: cannot watch {}: {e}; reading it before every heartbeat instead",
                    self.path.display()
                );
            }
        }
    }

    /// Take the watch that was being set up, when it is ready.
    async fn finish_watch(&mut self) {
        if self.starting.as_ref().is_some_and(|h| h.is_finished())
            && let Some(handle) = self.starting.take()
        {
            self.watch_started(handle.await);
            // A change before the watch was ready made no event.
            self.wanted = self.wanted || self.watch.is_some();
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Notified when the file may have changed; `None` when it is not
    /// watched.
    fn notifier(&self) -> Option<Arc<Notify>> {
        self.watch.as_ref().map(SecretWatch::notifier)
    }

    /// Read now; whether a read was started (not when one is still running).
    async fn read(&mut self, group: &mut Group, now: Instant) -> bool {
        let started = self
            .reader
            .refresh(group, &self.path, &mut self.warned, now)
            .await;
        if started {
            self.last_read = Some(now);
            self.wanted = false;
        }
        started
    }

    /// The watch made an event (or stopped working).
    async fn on_event(&mut self, group: &mut Group, now: Instant) {
        if let Some(why) = self.watch.as_ref().and_then(SecretWatch::failure) {
            self.watch = None;
            tracing::info!(
                "peers.secret_file: the watch of {} stopped working: {why}; reading it before every heartbeat instead",
                self.path.display()
            );
        }
        if self
            .last_read
            .is_some_and(|t| now.duration_since(t) < SECRET_EVENT_GAP)
            || !self.read(group, now).await
        {
            self.wanted = true;
        }
    }

    /// A heartbeat is due.
    async fn on_tick(&mut self, group: &mut Group, now: Instant) {
        self.finish_watch().await;
        let due = self.watch.is_none()
            || self.wanted
            || self.reader.in_flight()
            || self
                .last_read
                .is_none_or(|t| now.duration_since(t) >= SECRET_SAFETY_READ);
        if due {
            self.read(group, now).await;
        }
    }

    /// A reload changed `secret_file`.
    async fn on_path(&mut self, path: PathBuf, group: &mut Group, now: Instant) {
        self.path = path;
        self.watch_path().await;
        self.read(group, now).await;
    }
}

/// Wait for the watch of the secret file; for ever when it is not watched.
async fn watch_event(notifier: &Option<Arc<Notify>>) {
    match notifier {
        Some(n) => n.notified().await,
        None => std::future::pending().await,
    }
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

/// Send each datagram of a message to its members.
async fn send_all(socket: &UdpSocket, batches: &[Outgoing]) {
    for batch in batches {
        for addr in &batch.to {
            if let Err(e) = socket.send_to(&batch.data, addr).await {
                tracing::debug!("cannot send peer message to {addr}: {e}");
            }
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

#[allow(clippy::too_many_arguments)]
async fn run(
    socket: UdpSocket,
    mut group: Group,
    mut settings: watch::Receiver<PeerSettings>,
    mut out: mpsc::Receiver<PeerOut>,
    targets: Targets,
    quorum: Arc<Quorum>,
    mut state: StateOut,
    mut secret: SecretSource,
) {
    secret.begin().await;
    let mut tick = tokio::time::interval(HEARTBEAT_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // One byte more than accepted, so an oversized datagram is seen as such.
    let mut buf = vec![0u8; MAX_DATAGRAM + 1];
    loop {
        let secret_events = secret.notifier();
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
            _ = watch_event(&secret_events) => {
                secret.on_event(&mut group, Instant::now()).await;
            }
            _ = tick.tick() => {
                let now = Instant::now();
                secret.on_tick(&mut group, now).await;
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
                let batches = group.encode(Kind::Heartbeat(quorum.off_tags()), unix_ms(), now);
                send_all(&socket, &batches).await;
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
                counter.fetch_add(group.others().len() as u64, Ordering::Relaxed);
                let batches = group.encode(kind, unix_ms(), Instant::now());
                send_all(&socket, &batches).await;
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
                group.set_accept_plaintext(s.accept_plaintext);
                if s.secret_file != secret.path() {
                    secret
                        .on_path(s.secret_file, &mut group, Instant::now())
                        .await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued_down(n: u32) -> PeerOut {
        PeerOut::Down(BackendDown {
            service: "api.svc.emfor".into(),
            digest: "d".into(),
            backend: format!("10.0.0.{n}:8080"),
            reason: "refused".into(),
        })
    }

    /// Spec peer-group "Peer queue": a full queue drops new messages and
    /// counts them, logging at most once a minute; the queued ones stay.
    #[tokio::test(start_paused = true)]
    #[tracing_test::traced_test]
    async fn full_queue_drops_and_counts() {
        let stats = Arc::new(PeerStats::default());
        let (tx, mut rx) = PeerSender::with_capacity(2, stats.clone());
        for n in 1..=4 {
            tx.send(queued_down(n));
        }
        assert_eq!(stats.queue_dropped(), 2);
        assert_eq!(rx.recv().await, Some(queued_down(1)));
        assert_eq!(rx.recv().await, Some(queued_down(2)));
        assert!(rx.try_recv().is_err(), "the dropped ones are gone");
        tx.send(queued_down(5));
        assert_eq!(
            rx.recv().await,
            Some(queued_down(5)),
            "room again once read"
        );
        tx.send(queued_down(6));
        tx.send(queued_down(7));
        // Full again: dropped, but logged only once a minute.
        tx.send(queued_down(8));
        assert_eq!(stats.queue_dropped(), 3);
        tokio::time::advance(Duration::from_secs(61)).await;
        tx.send(queued_down(9));
        assert_eq!(stats.queue_dropped(), 4);
        logs_assert(|lines: &[&str]| {
            match lines
                .iter()
                .filter(|l| l.contains("the peer task is behind"))
                .count()
            {
                2 => Ok(()),
                n => Err(format!("{n} warnings")),
            }
        });
        assert_eq!(PEER_QUEUE, 4096);
    }
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

    /// Spec peer-group "Restart during a secret change" and "Stop, change
    /// the secret, start": with `previous_secret_file`, a daemon that last
    /// ran with the old secret and is restarted 30 seconds after its file
    /// changed signs and accepts as if it had not restarted, so a member
    /// whose file has not changed yet keeps accepting it.
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
            accept_plaintext: true,
        };
        let now = Instant::now();
        let at = |s| now + Duration::from_secs(s);
        // The record of a daemon that last ran with the old secret.
        let rec_dir = tempfile::tempdir().expect("a temporary directory");
        let rec = rec_dir.path().join("state.mdb-peers");
        let ran_with_old = || {
            record::Record {
                current: record::fingerprint(KEY),
                replaced: None,
            }
            .save(&rec)
            .expect("the record can be written");
        };
        ran_with_old();
        changed(30);
        let keys = start_keys(&conf(Some(old_path.clone())), &rec, new.clone(), now)
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
        let from_a = a.encode_one(Kind::Heartbeat(Vec::new()), t, now);
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
        ran_with_old();
        changed(200);
        let keys = start_keys(&conf(Some(old_path.clone())), &rec, new.clone(), now)
            .expect("both secret files are valid");
        assert_eq!(keys.accepted(now).count(), 1);
        ran_with_old();
        changed(30);
        let keys =
            start_keys(&conf(None), &rec, new.clone(), now).expect("no previous file to read");
        assert_eq!(keys.accepted(now).count(), 1);
        assert_eq!(keys.signing(now).raw(), &new[..]);
        // No record (the first start of this version): the new one only.
        std::fs::remove_file(&rec).expect("the record can be removed");
        let keys = start_keys(&conf(Some(old_path)), &rec, new.clone(), now)
            .expect("both secret files are valid");
        assert_eq!(keys.accepted(now).count(), 1);
        assert!(rec.exists(), "and the record is written");
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

    /// A running daemon whose secret file changes owner to someone else keeps
    /// the last valid secret and warns, as for a file it cannot read; the new
    /// content is taken once the owner is trusted again. Only root can give a
    /// file away, so this runs as root only (the Docker test run is root).
    #[test]
    fn secret_file_owner_change_keeps_the_last_secret() {
        if nix::unistd::geteuid().as_raw() != 0 {
            return;
        }
        let start = Instant::now();
        let (_d, path) = secret_file(&[1u8; 40], 0o640);
        let mut g = group(start);
        let mut warned = None;
        apply_secret(&mut g, read_secret(&path), &mut warned, start);
        assert_eq!(g.keys.current(), &[1u8; 40]);
        assert_eq!(warned, None);

        // The file gets new content and an owner that is neither root nor the
        // user running the test.
        std::fs::write(&path, [2u8; 40]).expect("the temporary secret file can be rewritten");
        std::os::unix::fs::chown(&path, Some(4242), None).expect("root can chown the file");
        let now = start + Duration::from_secs(1);
        let read = read_secret(&path);
        assert!(
            read.as_ref()
                .is_err_and(|e| e.contains("owned by uid 4242")),
            // Only the error: an Ok value would be the secret itself.
            "expected an owner error, got {:?}",
            read.as_ref().err()
        );
        apply_secret(&mut g, read, &mut warned, now);
        assert_eq!(g.keys.current(), &[1u8; 40], "keeps the last valid secret");
        assert_eq!(warned, Some(now), "and warns");

        // Owned by root again: the new content is accepted.
        std::os::unix::fs::chown(&path, Some(0), None).expect("root can chown the file");
        apply_secret(
            &mut g,
            read_secret(&path),
            &mut warned,
            start + Duration::from_secs(2),
        );
        assert_eq!(g.keys.current(), &[2u8; 40]);
    }

    /// Two peer tasks on 127.0.0.1 (real time).
    async fn pair(
        targets_a: Targets,
        targets_b: Targets,
    ) -> (PeersHandle, PeersHandle, tempfile::TempDir, PathBuf) {
        pair_with(targets_a, targets_b, read_secret).await
    }

    /// [`pair`] where A reads its secret file with `read_a` (and watches it).
    async fn pair_with(
        targets_a: Targets,
        targets_b: Targets,
        read_a: ReadSecret,
    ) -> (PeersHandle, PeersHandle, tempfile::TempDir, PathBuf) {
        pair_with_io(
            targets_a,
            targets_b,
            SecretIo {
                read: read_a,
                watch: SecretWatch::start,
            },
        )
        .await
    }

    /// [`pair`] where A reads and watches its secret file with `io`.
    async fn pair_with_io(
        targets_a: Targets,
        targets_b: Targets,
        io_a: SecretIo,
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
            accept_plaintext: true,
        };
        let ha = start_with_reader(
            sa,
            &conf(a),
            "dc1",
            Keys::new(vec![3u8; 32]),
            targets_a,
            None,
            Vec::new(),
            io_a,
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
                accept_plaintext: true,
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
                accept_plaintext: true,
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
                accept_plaintext: true,
            },
            "dc1",
            Keys::new(vec![3u8; 32]),
            ta,
            None,
            Vec::new(),
        );
        ha.out.send(PeerOut::Down(BackendDown {
            service: "api.svc.emfor".into(),
            digest: DIGEST.into(),
            backend: "10.0.0.11:8080".into(),
            reason: "refused".into(),
        }));
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
            ha.out.send(PeerOut::Result(BackendResult {
                service: service.into(),
                digest: DIGEST.into(),
                backend: "10.0.0.11:8080".into(),
                outcome: Outcome::Down("refused".into()),
                interval: Duration::from_secs(2),
            }));
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
        ha.out.send(PeerOut::Down(BackendDown {
            service: "api.svc.emfor".into(),
            digest: "f".repeat(32),
            backend: "10.0.0.11:8080".into(),
            reason: "refused".into(),
        }));
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
        ha.out.send(PeerOut::Result(BackendResult {
            service: "api.svc.emfor".into(),
            digest: DIGEST.into(),
            backend: "10.0.0.11:8080".into(),
            outcome: Outcome::Up,
            interval: Duration::from_secs(2),
        }));
        let a = ha.listen.to_string();
        wait_until("mismatch", Duration::from_secs(2), || {
            hb.quorum
                .summary(&[], Instant::now())
                .contains(&format!(r#"{{"service":"api.svc.emfor","member":"{a}"}}"#))
        })
        .await;
    }

    // The functions below stand for a read of the secret file that does not
    // return until the test lets it. A function pointer cannot capture, so
    // each test has its own statics.

    /// Block until `release` is set, as a read on a hung file system does.
    fn block_until(release: &std::sync::atomic::AtomicBool) {
        while !release.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Wait (real time) until the read in flight has finished.
    async fn read_finished(reader: &SecretReader) {
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while !reader
            .pending
            .as_ref()
            .is_some_and(|(h, _)| h.is_finished())
        {
            assert!(std::time::Instant::now() < end, "the read did not finish");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn warnings_about_the_read(lines: &[&str]) -> usize {
        lines
            .iter()
            .filter(|l| {
                l.contains("has not finished") && l.contains("keeping the last valid secret")
            })
            .count()
    }

    static HANG_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static HANG_RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    fn hang(_: &Path) -> Result<Vec<u8>, String> {
        HANG_CALLS.fetch_add(1, Ordering::SeqCst);
        block_until(&HANG_RELEASE);
        Ok(vec![9u8; 40])
    }

    /// Spec peer-group "Read of the secret file hangs" (the reader):
    /// one read however many ticks pass, a warning after 5 seconds and not
    /// again within a minute, and the result applied when it returns.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn hung_read_stays_one_and_is_warned_about() {
        let start = Instant::now();
        let (_d, path) = secret_file(&[1u8; 40], 0o600);
        let mut g = group(start);
        let mut warned = None;
        let mut reader = SecretReader::new(hang);
        for s in 0..=5 {
            reader
                .refresh(&mut g, &path, &mut warned, start + Duration::from_secs(s))
                .await;
        }
        assert_eq!(HANG_CALLS.load(Ordering::SeqCst), 1, "one read in flight");
        assert!(
            warned.is_none(),
            "not warned until the read ran more than 5s"
        );
        reader
            .refresh(&mut g, &path, &mut warned, start + Duration::from_secs(6))
            .await;
        assert_eq!(warned, Some(start + Duration::from_secs(6)));
        for s in 7..=65 {
            reader
                .refresh(&mut g, &path, &mut warned, start + Duration::from_secs(s))
                .await;
        }
        assert_eq!(
            warned,
            Some(start + Duration::from_secs(6)),
            "not within a minute"
        );
        reader
            .refresh(&mut g, &path, &mut warned, start + Duration::from_secs(66))
            .await;
        assert_eq!(warned, Some(start + Duration::from_secs(66)));
        logs_assert(|lines: &[&str]| match warnings_about_the_read(lines) {
            2 => Ok(()),
            n => Err(format!("{n} warnings")),
        });
        assert_eq!(HANG_CALLS.load(Ordering::SeqCst), 1);
        assert_ne!(
            g.keys.current(),
            &[9u8; 40],
            "the secret is the old one so far"
        );
        HANG_RELEASE.store(true, Ordering::SeqCst);
        read_finished(&reader).await;
        reader
            .refresh(&mut g, &path, &mut warned, start + Duration::from_secs(67))
            .await;
        assert_eq!(g.keys.current(), &[9u8; 40], "the late result is applied");
    }

    static SLOW_RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    fn slow(_: &Path) -> Result<Vec<u8>, String> {
        block_until(&SLOW_RELEASE);
        Ok(vec![8u8; 40])
    }

    /// Spec peer-group "Slow read of the secret file": no warning before
    /// 5 seconds; the secret is applied when the read returns.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn slow_read_is_applied_without_a_warning() {
        let start = Instant::now();
        let (_d, path) = secret_file(&[1u8; 40], 0o600);
        let mut g = group(start);
        let mut warned = None;
        let mut reader = SecretReader::new(slow);
        reader.refresh(&mut g, &path, &mut warned, start).await;
        reader
            .refresh(&mut g, &path, &mut warned, start + Duration::from_secs(3))
            .await;
        assert_ne!(g.keys.current(), &[8u8; 40]);
        SLOW_RELEASE.store(true, Ordering::SeqCst);
        read_finished(&reader).await;
        reader
            .refresh(&mut g, &path, &mut warned, start + Duration::from_secs(3))
            .await;
        assert_eq!(g.keys.current(), &[8u8; 40]);
        logs_assert(|lines: &[&str]| match warnings_about_the_read(lines) {
            0 => Ok(()),
            n => Err(format!("{n} warnings")),
        });
    }

    fn at_once(_: &Path) -> Result<Vec<u8>, String> {
        Ok(vec![7u8; 40])
    }

    /// Spec peer-group "Normal read applied at once": a read that returns
    /// in time is applied by the same call, before the heartbeats.
    #[tokio::test]
    async fn quick_read_is_applied_by_the_same_call() {
        let start = Instant::now();
        let (_d, path) = secret_file(&[1u8; 40], 0o600);
        let mut g = group(start);
        let mut warned = None;
        let mut reader = SecretReader::new(at_once);
        reader.refresh(&mut g, &path, &mut warned, start).await;
        assert_eq!(g.keys.current(), &[7u8; 40]);
        assert!(reader.pending.is_none(), "nothing is left in flight");
    }

    static PANIC_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn panics(_: &Path) -> Result<Vec<u8>, String> {
        PANIC_CALLS.fetch_add(1, Ordering::SeqCst);
        panic!("the injected read panics");
    }

    /// A read that panics is a failed read: the last valid secret stays,
    /// the warning is the usual one, and the next tick reads again.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn read_that_panics_keeps_the_secret_and_is_retried() {
        let start = Instant::now();
        let (_d, path) = secret_file(&[1u8; 40], 0o600);
        let mut g = group(start);
        let before = g.keys.current().to_vec();
        let mut warned = None;
        let mut reader = SecretReader::new(panics);
        reader.refresh(&mut g, &path, &mut warned, start).await;
        assert_eq!(g.keys.current(), &before[..]);
        assert_eq!(warned, Some(start));
        reader
            .refresh(&mut g, &path, &mut warned, start + Duration::from_secs(1))
            .await;
        assert_eq!(PANIC_CALLS.load(Ordering::SeqCst), 2, "read again");
        assert!(logs_contain("keeping the last valid secret"));
    }

    static PATHS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    static PATH_RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    fn record_path(p: &Path) -> Result<Vec<u8>, String> {
        PATHS
            .lock()
            .expect("the recorded paths are not poisoned")
            .push(p.to_path_buf());
        block_until(&PATH_RELEASE);
        Ok(vec![6u8; 40])
    }

    /// A reload that changes `secret_file` while a read of the old path is
    /// in flight starts no second read; the new path is read after the old
    /// read is over.
    #[tokio::test]
    async fn changed_path_is_read_after_the_read_in_flight() {
        let start = Instant::now();
        let (_d1, one) = secret_file(&[1u8; 40], 0o600);
        let (_d2, two) = secret_file(&[2u8; 40], 0o600);
        let mut g = group(start);
        let mut warned = None;
        let mut reader = SecretReader::new(record_path);
        reader.refresh(&mut g, &one, &mut warned, start).await;
        reader
            .refresh(&mut g, &two, &mut warned, start + Duration::from_secs(1))
            .await;
        let seen = |n: usize| {
            let paths = PATHS.lock().expect("the recorded paths are not poisoned");
            assert_eq!(paths.len(), n);
            paths.clone()
        };
        assert_eq!(seen(1), vec![one.clone()], "no second read yet");
        PATH_RELEASE.store(true, Ordering::SeqCst);
        read_finished(&reader).await;
        reader
            .refresh(&mut g, &two, &mut warned, start + Duration::from_secs(2))
            .await;
        assert_eq!(seen(2), vec![one, two], "the new path after the old read");
    }

    static LOOP_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static LOOP_RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    fn hang_in_the_loop(_: &Path) -> Result<Vec<u8>, String> {
        LOOP_CALLS.fetch_add(1, Ordering::SeqCst);
        block_until(&LOOP_RELEASE);
        Err("released".into())
    }

    /// Spec peer-group "Read of the secret file hangs": the peer loop of A
    /// goes on while its read of the secret file never returns. B stays
    /// live at A (A handles B's heartbeats) and A at B (A sends its own),
    /// and the file is read once however many ticks pass.
    #[tokio::test]
    async fn heartbeats_go_on_while_the_secret_read_hangs() {
        let (ha, hb, _dir, _path) =
            pair_with(Targets::default(), Targets::default(), hang_in_the_loop).await;
        let (a, b) = (ha.listen, hb.listen);
        wait_until("b live at a", Duration::from_millis(3000), || {
            ha.stats.up() == vec![(b, true)]
        })
        .await;
        wait_until("a live at b", Duration::from_millis(3000), || {
            hb.stats.up() == vec![(a, true)]
        })
        .await;
        // Several more ticks: both stay live, and no second read starts.
        tokio::time::sleep(Duration::from_millis(3500)).await;
        assert_eq!(ha.stats.up(), vec![(b, true)]);
        assert_eq!(hb.stats.up(), vec![(a, true)]);
        assert_eq!(LOOP_CALLS.load(Ordering::SeqCst), 1);
        LOOP_RELEASE.store(true, Ordering::SeqCst);
    }

    // The secret source: what reads the file when. The read functions count
    // their calls; each test has its own counter.

    fn no_watch(_: &Path) -> Result<SecretWatch, String> {
        Err("the test has no watch".into())
    }

    fn count_to(counter: &std::sync::atomic::AtomicUsize) -> Result<Vec<u8>, String> {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(vec![5u8; 40])
    }

    /// Wait (real time) for the watch of `source` to notify.
    async fn watched_change(source: &SecretSource) {
        let notifier = source.notifier().expect("the file is watched");
        tokio::time::timeout(Duration::from_secs(3), notifier.notified())
            .await
            .expect("the change is noticed within 3s");
    }

    /// Replace `path` by another file with a valid secret.
    fn replace_secret(path: &Path, key: u8) {
        use std::os::unix::fs::PermissionsExt;
        let new = path.with_extension("new");
        std::fs::write(&new, [key; 40]).expect("the new file can be written");
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o600))
            .expect("the new file can be made private");
        std::fs::rename(&new, path).expect("the new file can replace the old one");
    }

    static UNWATCHED_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn count_unwatched(_: &Path) -> Result<Vec<u8>, String> {
        count_to(&UNWATCHED_CALLS)
    }

    /// Spec peer-group "Watch cannot be set up": the file is read before
    /// each round of heartbeats, and one `info` line says why.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn unwatched_file_is_read_on_every_tick() {
        let start = Instant::now();
        let (_d, path) = secret_file(&[1u8; 40], 0o600);
        let mut g = group(start);
        let mut source = SecretSource::new(
            path,
            SecretIo {
                read: count_unwatched,
                watch: no_watch,
            },
        );
        source.begin().await;
        assert!(source.notifier().is_none());
        for s in 0..3 {
            source.on_tick(&mut g, start + Duration::from_secs(s)).await;
        }
        assert_eq!(UNWATCHED_CALLS.load(Ordering::SeqCst), 3, "a read per tick");
        assert!(logs_contain("reading it before every heartbeat instead"));
        assert!(logs_contain("the test has no watch"));
    }

    static WATCHED_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn count_watched(_: &Path) -> Result<Vec<u8>, String> {
        count_to(&WATCHED_CALLS)
    }

    /// While the file is watched it is read when the watch makes an event
    /// and every 30 seconds, and not on the other ticks.
    #[tokio::test]
    async fn watched_file_is_read_on_events_and_every_30_seconds() {
        let start = Instant::now();
        let (_d, path) = secret_file(&[1u8; 40], 0o600);
        let mut g = group(start);
        let mut source = SecretSource::new(
            path.clone(),
            SecretIo {
                read: count_watched,
                watch: SecretWatch::start,
            },
        );
        source.begin().await;
        for s in 0..30 {
            source.on_tick(&mut g, start + Duration::from_secs(s)).await;
        }
        assert_eq!(
            WATCHED_CALLS.load(Ordering::SeqCst),
            1,
            "the first tick only"
        );
        source
            .on_tick(&mut g, start + Duration::from_secs(30))
            .await;
        assert_eq!(WATCHED_CALLS.load(Ordering::SeqCst), 2, "30s later again");
        replace_secret(&path, 6);
        watched_change(&source).await;
        source
            .on_event(&mut g, start + Duration::from_secs(31))
            .await;
        assert_eq!(
            WATCHED_CALLS.load(Ordering::SeqCst),
            3,
            "a read for the event"
        );
    }

    static MOVED_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn count_moved(_: &Path) -> Result<Vec<u8>, String> {
        count_to(&MOVED_CALLS)
    }

    /// A reload that changes `secret_file` watches the new path and reads
    /// it at once.
    #[tokio::test]
    async fn changed_path_is_watched() {
        let start = Instant::now();
        let (_d1, one) = secret_file(&[1u8; 40], 0o600);
        let (_d2, two) = secret_file(&[2u8; 40], 0o600);
        let mut g = group(start);
        let mut source = SecretSource::new(
            one,
            SecretIo {
                read: count_moved,
                watch: SecretWatch::start,
            },
        );
        source.begin().await;
        source.on_path(two.clone(), &mut g, start).await;
        assert_eq!(
            MOVED_CALLS.load(Ordering::SeqCst),
            1,
            "the new path is read"
        );
        assert_eq!(source.path(), two);
        replace_secret(&two, 7);
        watched_change(&source).await;
    }

    static GONE_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn count_gone(_: &Path) -> Result<Vec<u8>, String> {
        count_to(&GONE_CALLS)
    }

    /// A watch that stops working (its directory was removed) is dropped,
    /// with one `info` line, and the file is read on every tick after it.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn watch_that_stops_working_falls_back_to_reading_every_tick() {
        let start = Instant::now();
        let dir = tempfile::tempdir().expect("a temporary directory");
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).expect("the directory can be created");
        let path = sub.join("secret");
        std::fs::write(&path, [1u8; 40]).expect("the secret file can be written");
        let mut g = group(start);
        let mut source = SecretSource::new(
            path,
            SecretIo {
                read: count_gone,
                watch: SecretWatch::start,
            },
        );
        source.begin().await;
        std::fs::remove_dir_all(&sub).expect("the directory can be removed");
        // The file is removed before its directory: the first notification
        // can be for that, and the failure comes with a later one.
        for _ in 0..5 {
            if source.notifier().is_none() {
                break;
            }
            watched_change(&source).await;
            source.on_event(&mut g, start).await;
        }
        assert!(source.notifier().is_none(), "no longer watched");
        assert!(logs_contain("stopped working"));
        let before = GONE_CALLS.load(Ordering::SeqCst);
        source.on_tick(&mut g, start + Duration::from_secs(1)).await;
        source.on_tick(&mut g, start + Duration::from_secs(2)).await;
        assert_eq!(GONE_CALLS.load(Ordering::SeqCst), before + 2);
    }

    static LOOP_WATCHED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn count_in_the_loop(_: &Path) -> Result<Vec<u8>, String> {
        LOOP_WATCHED.fetch_add(1, Ordering::SeqCst);
        Err("a read to count".into())
    }

    /// Spec peer-group "Secret file replaced by a rename", through the peer
    /// loop: while the file is watched the ticks do not read it, and a file
    /// renamed over it is read at once, not at the next poll.
    #[tokio::test]
    async fn loop_reads_a_replaced_secret_file_at_once() {
        let (_ha, _hb, _dir, path) =
            pair_with(Targets::default(), Targets::default(), count_in_the_loop).await;
        wait_until("the first read", Duration::from_secs(2), || {
            LOOP_WATCHED.load(Ordering::SeqCst) >= 1
        })
        .await;
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert_eq!(
            LOOP_WATCHED.load(Ordering::SeqCst),
            1,
            "no read on the ticks while the file is watched"
        );
        replace_secret(&path, 4);
        wait_until("the read for the rename", Duration::from_secs(1), || {
            LOOP_WATCHED.load(Ordering::SeqCst) >= 2
        })
        .await;
    }

    static GAP_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn count_gap(_: &Path) -> Result<Vec<u8>, String> {
        count_to(&GAP_CALLS)
    }

    /// An event less than 100 ms after the last read is not read at once
    /// (a busy directory makes many); the next heartbeat reads it.
    #[tokio::test]
    async fn event_soon_after_a_read_is_left_to_the_next_tick() {
        let start = Instant::now();
        let (_d, path) = secret_file(&[1u8; 40], 0o600);
        let mut g = group(start);
        let mut source = SecretSource::new(
            path,
            SecretIo {
                read: count_gap,
                watch: SecretWatch::start,
            },
        );
        source.begin().await;
        source.on_tick(&mut g, start).await;
        assert_eq!(GAP_CALLS.load(Ordering::SeqCst), 1);
        source
            .on_event(&mut g, start + Duration::from_millis(10))
            .await;
        assert_eq!(GAP_CALLS.load(Ordering::SeqCst), 1, "no read for the event");
        source.on_tick(&mut g, start + Duration::from_secs(1)).await;
        assert_eq!(GAP_CALLS.load(Ordering::SeqCst), 2, "the tick reads it");
        source.on_tick(&mut g, start + Duration::from_secs(2)).await;
        assert_eq!(GAP_CALLS.load(Ordering::SeqCst), 2, "and only once");
    }

    static BUSY_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static BUSY_RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    fn slow_first_busy(_: &Path) -> Result<Vec<u8>, String> {
        if BUSY_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            block_until(&BUSY_RELEASE);
        }
        Ok(vec![5u8; 40])
    }

    /// An event that comes while a read is running is not lost: once that
    /// read is over the next heartbeat applies it and reads again.
    #[tokio::test]
    async fn event_during_a_running_read_is_read_after_it() {
        let start = Instant::now();
        let (_d, path) = secret_file(&[1u8; 40], 0o600);
        let mut g = group(start);
        let mut source = SecretSource::new(
            path,
            SecretIo {
                read: slow_first_busy,
                watch: SecretWatch::start,
            },
        );
        source.begin().await;
        source.on_tick(&mut g, start).await;
        assert!(source.reader.in_flight(), "the first read is stuck");
        source
            .on_event(&mut g, start + Duration::from_secs(1))
            .await;
        assert_eq!(BUSY_CALLS.load(Ordering::SeqCst), 1, "no second read");
        BUSY_RELEASE.store(true, Ordering::SeqCst);
        read_finished(&source.reader).await;
        source.on_tick(&mut g, start + Duration::from_secs(2)).await;
        assert_eq!(BUSY_CALLS.load(Ordering::SeqCst), 2, "read again after it");
    }

    static SLOW_WATCH_RELEASE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    static SLOW_WATCH_READS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    fn slow_watch(path: &Path) -> Result<SecretWatch, String> {
        block_until(&SLOW_WATCH_RELEASE);
        SecretWatch::start(path)
    }

    fn count_slow_watch(_: &Path) -> Result<Vec<u8>, String> {
        count_to(&SLOW_WATCH_READS)
    }

    /// A watch that takes long to set up (a directory on a file system that
    /// does not answer) is not waited for: the file is read on every tick
    /// meanwhile, and watched, with one more read, when the watch is ready.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn slow_watch_start_does_not_hold_the_loop() {
        let start = Instant::now();
        let (_d, path) = secret_file(&[1u8; 40], 0o600);
        let mut g = group(start);
        let mut source = SecretSource::new(
            path,
            SecretIo {
                read: count_slow_watch,
                watch: slow_watch,
            },
        );
        let began = std::time::Instant::now();
        source.begin().await;
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "begin does not wait"
        );
        assert!(source.notifier().is_none());
        assert!(logs_contain("is not ready after"));
        source.on_tick(&mut g, start).await;
        source.on_tick(&mut g, start + Duration::from_secs(1)).await;
        assert_eq!(
            SLOW_WATCH_READS.load(Ordering::SeqCst),
            2,
            "a read per tick"
        );
        SLOW_WATCH_RELEASE.store(true, Ordering::SeqCst);
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while !source.starting.as_ref().is_some_and(|h| h.is_finished()) {
            assert!(std::time::Instant::now() < end, "the watch did not start");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        source.on_tick(&mut g, start + Duration::from_secs(2)).await;
        assert!(source.notifier().is_some(), "watched now");
        assert_eq!(
            SLOW_WATCH_READS.load(Ordering::SeqCst),
            3,
            "one read for what changed before the watch was ready"
        );
    }
}

#[cfg(test)]
mod bench;
