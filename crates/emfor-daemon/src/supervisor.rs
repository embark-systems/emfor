//! Owns the per-backend check tasks and keeps them, and the store layout, in
//! line with the current config on start and on every reload.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::config::{Config, ServiceConfig, Settings};
use crate::peers::{PeerHooks, Recheck, ServiceTargets, Targets};
use crate::schedule::{PeerLink, spawn_backend};
use crate::writer::{ServiceLayout, WriterMsg};

struct Running {
    config: ServiceConfig,
    generation: u64,
    settings: watch::Sender<Settings>,
    tasks: HashMap<String, JoinHandle<()>>,
    /// "Recheck now" channel of each backend task, by backend id; kept when
    /// a task is started again.
    rechecks: HashMap<String, Arc<Recheck>>,
    /// Restarts of check tasks that stopped, by backend id.
    restarts: HashMap<String, Restarts>,
}

/// How often one backend's check task was started again, and when last.
#[derive(Clone, Copy)]
struct Restarts {
    count: u32,
    at: tokio::time::Instant,
}

/// Longest wait between two restarts of one backend's check task.
const MAX_RESTART_DELAY: Duration = Duration::from_secs(60);
/// A task that ran this long since its last restart starts the count over.
const RESTART_RESET: Duration = Duration::from_secs(300);

/// Least time after the `count`-th restart before the next one: none after
/// the first stop, then 1s, 2s, 4s and so on up to [`MAX_RESTART_DELAY`].
fn restart_delay(count: u32) -> Duration {
    match count {
        0 => Duration::ZERO,
        n => Duration::from_secs(1u64 << (n - 1).min(6)).min(MAX_RESTART_DELAY),
    }
}

impl Running {
    fn abort_all(&mut self) {
        for (_, t) in self.tasks.drain() {
            t.abort();
        }
        self.rechecks.clear();
    }
}

pub struct Supervisor {
    writer: mpsc::UnboundedSender<WriterMsg>,
    services: BTreeMap<Arc<str>, Running>,
    next_generation: u64,
    /// Every backend's recheck channel, for the peer task.
    targets: Targets,
    /// What backend tasks use of the peer group; `None` without `[peers]`.
    peers: Option<PeerHooks>,
}

impl Supervisor {
    /// Fresh start: the store drops every previous `svc/*` record and all
    /// backends start `unknown`.
    pub fn start(config: &Config, writer: mpsc::UnboundedSender<WriterMsg>) -> Self {
        Self::start_with_peers(config, writer, Targets::default(), None)
    }

    /// [`Supervisor::start`] for a daemon in a peer group: every backend's
    /// recheck channel is kept in `targets`, and backend tasks get `peers`.
    pub fn start_with_peers(
        config: &Config,
        writer: mpsc::UnboundedSender<WriterMsg>,
        targets: Targets,
        peers: Option<PeerHooks>,
    ) -> Self {
        let mut s = Supervisor {
            writer,
            services: BTreeMap::new(),
            next_generation: 1,
            targets,
            peers,
        };
        s.sync(config, true);
        s
    }

    /// Publish every service's digest and recheck channels to the peer task,
    /// and drop the votes and disagreements of removed services and backends.
    fn publish_targets(&self) {
        if let Some(p) = &self.peers {
            let keep = self
                .services
                .iter()
                .map(|(name, r)| {
                    let ids = r.config.backends.iter().map(|b| b.id.clone()).collect();
                    (name.to_string(), ids)
                })
                .collect();
            p.quorum.retain(&keep);
        }
        let services = self
            .services
            .iter()
            .map(|(name, r)| {
                (
                    name.to_string(),
                    ServiceTargets {
                        digest: r.config.digest.clone(),
                        backends: r.rechecks.clone(),
                    },
                )
            })
            .collect();
        self.targets.replace(services);
    }

    /// Apply a validated reload.
    pub fn reload(&mut self, config: &Config) {
        self.sync(config, false);
    }

    /// Start again every check task that ended on its own. A task only ends
    /// by a panic while the daemon runs (or when the writer is gone, at
    /// shutdown), and without this its backend would keep its last state
    /// for good. The new task starts from `unknown`. A task that keeps
    /// stopping is started again after 1s, 2s, 4s and so on, up to 60s, so
    /// a checker that always panics does not fill the log; the wait starts
    /// over once a task ran 5 minutes. Returns how many were restarted.
    pub fn restart_finished(&mut self) -> usize {
        let now = tokio::time::Instant::now();
        let mut restarted = 0;
        let peers = self.peers.clone();
        for (name, r) in &mut self.services {
            for b in &r.config.backends {
                let Some(t) = r.tasks.get_mut(&b.id) else {
                    continue;
                };
                if !t.is_finished() {
                    continue;
                }
                let count = match r.restarts.get(&b.id) {
                    Some(last) if now.duration_since(last.at) < RESTART_RESET => {
                        if now.duration_since(last.at) < restart_delay(last.count) {
                            continue; // too soon after the last restart
                        }
                        last.count
                    }
                    _ => 0,
                };
                let next = restart_delay(count + 1);
                tracing::error!(
                    "check task of backend {} of {name} stopped; starting it again from unknown (restart {}; the next one no sooner than {}s after this)",
                    b.id,
                    count + 1,
                    next.as_secs()
                );
                let recheck = r.rechecks.entry(b.id.clone()).or_default().clone();
                *t = spawn_backend(
                    name.clone(),
                    b.clone(),
                    r.config.checker.clone(),
                    r.generation,
                    r.settings.subscribe(),
                    self.writer.clone(),
                    PeerLink {
                        recheck,
                        peers: peers.clone(),
                    },
                );
                r.restarts.insert(
                    b.id.clone(),
                    Restarts {
                        count: count + 1,
                        at: now,
                    },
                );
                restarted += 1;
            }
        }
        restarted
    }

    /// Stop every check task.
    pub fn stop(&mut self) {
        for r in self.services.values_mut() {
            r.abort_all();
        }
        self.services.clear();
    }

    fn sync(&mut self, config: &Config, fresh_start: bool) {
        let wanted: BTreeMap<Arc<str>, &ServiceConfig> = config
            .services
            .iter()
            .map(|s| (Arc::<str>::from(s.name.as_str()), s))
            .collect();

        // 1. Stop what goes away or must be reset, before the layout is sent.
        let names: Vec<Arc<str>> = self.services.keys().cloned().collect();
        for name in names {
            let keep = match wanted.get(&name) {
                None => false,
                Some(new) => !fresh_start && !self.services[&name].config.check_changed(new),
            };
            if !keep && let Some(mut r) = self.services.remove(&name) {
                r.abort_all();
            }
        }
        for (name, new) in &wanted {
            if let Some(r) = self.services.get_mut(name) {
                let ids: Vec<&str> = new.backends.iter().map(|b| b.id.as_str()).collect();
                let removed: Vec<_> = r
                    .config
                    .backends
                    .iter()
                    .filter(|b| !ids.contains(&b.id.as_str()))
                    .map(|b| b.addr)
                    .collect();
                r.tasks.retain(|id, t| {
                    let keep = ids.contains(&id.as_str());
                    if !keep {
                        t.abort();
                    }
                    keep
                });
                r.restarts.retain(|id, _| ids.contains(&id.as_str()));
                r.rechecks.retain(|id, _| ids.contains(&id.as_str()));
                for addr in removed {
                    r.config.checker.forget(addr);
                }
            }
        }

        // 2. Send the layout, so updates from tasks spawned below follow it.
        let mut layouts = Vec::new();
        for (name, new) in &wanted {
            let reset = !self.services.contains_key(name);
            let generation = match self.services.get(name) {
                Some(r) => r.generation,
                None => {
                    self.next_generation += 1;
                    self.next_generation
                }
            };
            layouts.push(ServiceLayout {
                name: name.clone(),
                on_all_down: new.settings.on_all_down,
                balance: new.settings.balance,
                dns_ttl: new.dns_ttl,
                backends: new.backends.clone(),
                generation,
                reset,
            });
        }
        let _ = self.writer.send(WriterMsg::SetLayout {
            fresh_start,
            site: config.site.clone(),
            services: layouts.clone(),
        });

        // 3. Update kept services and spawn missing tasks.
        for layout in layouts {
            let new = wanted[&layout.name];
            let r = self
                .services
                .entry(layout.name.clone())
                .or_insert_with(|| Running {
                    config: new.clone(),
                    generation: layout.generation,
                    settings: watch::channel(new.settings).0,
                    tasks: HashMap::new(),
                    rechecks: HashMap::new(),
                    restarts: HashMap::new(),
                });
            let checker = r.config.checker.clone(); // unchanged check: keep the running checker
            r.config = ServiceConfig {
                checker: checker.clone(),
                ..new.clone()
            };
            r.settings.send_replace(new.settings);
            for b in &new.backends {
                if !r.tasks.contains_key(&b.id) {
                    r.rechecks.entry(b.id.clone()).or_default();
                    let link = PeerLink {
                        recheck: r.rechecks[&b.id].clone(),
                        peers: self.peers.clone(),
                    };
                    let t = spawn_backend(
                        layout.name.clone(),
                        b.clone(),
                        checker.clone(),
                        layout.generation,
                        r.settings.subscribe(),
                        self.writer.clone(),
                        link,
                    );
                    r.tasks.insert(b.id.clone(), t);
                }
            }
        }
        self.publish_targets();
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::Registry;
    use crate::store::{Store, tests::cfg};
    use crate::writer;
    use emfor_format::{HealthState, ServiceRecord, service_key};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Listener that answers `GET /` with 200 and any other path with 404.
    async fn http_ok() -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let root = buf[..n].starts_with(b"GET / ");
                    let status: &[u8] = if root { b"200 OK" } else { b"404 Not Found" };
                    let resp = [
                        b"HTTP/1.1 ".as_slice(),
                        status,
                        b"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    ]
                    .concat();
                    let _ = s.write_all(&resp).await;
                });
            }
        });
        addr
    }

    /// Poll `cond` every 20ms until it holds, failing after 5s: the checks
    /// and the writer run on real time, which a loaded machine stretches.
    async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A port that refuses connections: bound but not listening, and held
    /// for as long as the socket lives so no other test can take it.
    fn closed_port() -> (tokio::net::TcpSocket, std::net::SocketAddr) {
        let s = tokio::net::TcpSocket::new_v4().unwrap();
        s.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = s.local_addr().unwrap();
        (s, addr)
    }

    fn conf(text: &str) -> Config {
        crate::config::parse(text, &Registry::builtin()).unwrap()
    }

    fn rec(store: &Store, name: &str) -> Option<ServiceRecord> {
        store
            .get(&service_key(name))
            .unwrap()
            .map(|j| serde_json::from_str(&j).unwrap())
    }

    /// Backend states of `name`; empty while its record is not written yet.
    fn states(store: &Store, name: &str) -> Vec<HealthState> {
        rec(store, name)
            .map(|r| r.backends.iter().map(|b| b.state).collect())
            .unwrap_or_default()
    }

    fn http_service(name: &str, addr: &str, path: &str, interval: &str) -> String {
        format!(
            "[services.\"{name}\"]\ncheck = \"http\"\nbackends = [\"{addr}\"]\ninterval = \"{interval}\"\ntimeout = \"{interval}\"\nrise = 1\njitter = \"0%\"\n[services.\"{name}\".check_options]\npath = \"{path}\"\n"
        )
    }

    struct Harness {
        store: Arc<Store>,
        tx: mpsc::UnboundedSender<WriterMsg>,
        _dir: tempfile::TempDir,
    }

    fn harness() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&cfg(dir.path())).unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        writer::spawn(store.clone(), rx);
        Harness {
            store,
            tx,
            _dir: dir,
        }
    }

    /// A check task that panics is started again from `unknown` and its
    /// backend goes on being checked.
    #[tokio::test]
    async fn panicked_check_task_is_started_again() {
        use crate::checker::{CheckRequest, CheckResult, Checker};
        use futures::future::BoxFuture;
        use std::sync::atomic::{AtomicU32, Ordering};

        #[derive(Debug, Default)]
        struct PanicsFirst(AtomicU32);
        impl Checker for PanicsFirst {
            fn check(&self, _: CheckRequest) -> BoxFuture<'_, CheckResult> {
                Box::pin(async move {
                    if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                        panic!("checker bug");
                    }
                    CheckResult::Up
                })
            }
        }

        let h = harness();
        let mut config = conf(
            "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"127.0.0.1:1\"]\ninterval = \"200ms\"\ntimeout = \"100ms\"\nrise = 1\njitter = \"0%\"\n",
        );
        let checker = Arc::new(PanicsFirst::default());
        config.services[0].checker = checker.clone();
        let mut sup = Supervisor::start(&config, h.tx.clone());
        let task = |sup: &Supervisor| {
            sup.services
                .values()
                .next()
                .unwrap()
                .tasks
                .values()
                .next()
                .unwrap()
                .is_finished()
        };
        wait_for("task ended by the panic", || task(&sup)).await;
        assert_eq!(sup.restart_finished(), 1);
        assert!(!task(&sup));
        wait_for("healthy after the restart", || {
            states(&h.store, "api.svc.emfor") == [HealthState::Healthy]
        })
        .await;
        assert_eq!(sup.restart_finished(), 0, "running tasks are left alone");
        assert!(checker.0.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn restart_delays_double_up_to_a_minute() {
        let secs: Vec<u64> = (0..10).map(|n| restart_delay(n).as_secs()).collect();
        assert_eq!(secs, [0, 1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(restart_delay(u32::MAX), MAX_RESTART_DELAY);
    }

    /// A check task that stops again and again is restarted at once, then
    /// after 1s, 2s, ...; after running 5 minutes the count starts over.
    #[tokio::test(start_paused = true)]
    async fn repeated_stops_are_restarted_with_growing_delays() {
        use crate::checker::{CheckRequest, CheckResult, Checker};
        use futures::future::BoxFuture;
        use tokio::time::{Instant, sleep_until};

        #[derive(Debug)]
        struct AlwaysPanics;
        impl Checker for AlwaysPanics {
            fn check(&self, _: CheckRequest) -> BoxFuture<'_, CheckResult> {
                Box::pin(async { panic!("checker bug") })
            }
        }

        let h = harness();
        let mut config = conf(
            "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"127.0.0.1:1\"]\ninterval = \"200ms\"\ntimeout = \"100ms\"\nrise = 1\njitter = \"0%\"\n",
        );
        config.services[0].checker = Arc::new(AlwaysPanics);
        let mut sup = Supervisor::start(&config, h.tx.clone());
        let finished = |sup: &Supervisor| {
            let r = sup.services.values().next().unwrap();
            r.tasks.values().next().unwrap().is_finished()
        };
        let count = |sup: &Supervisor| {
            let r = sup.services.values().next().unwrap();
            r.restarts.values().next().map_or(0, |x| x.count)
        };

        wait_for("first stop", || finished(&sup)).await;
        assert_eq!(sup.restart_finished(), 1, "the first restart is at once");
        let mut last = Instant::now();
        for (n, delay) in [(1u32, 1u64), (2, 2), (3, 4), (4, 8)] {
            wait_for("stopped again", || finished(&sup)).await;
            sleep_until(last + Duration::from_millis(delay * 1000 - 50)).await;
            assert_eq!(sup.restart_finished(), 0, "too soon after restart {n}");
            sleep_until(last + Duration::from_secs(delay)).await;
            assert_eq!(sup.restart_finished(), 1, "{delay}s after restart {n}");
            assert_eq!(count(&sup), n + 1);
            last = Instant::now();
        }
        wait_for("stopped again", || finished(&sup)).await;
        sleep_until(last + RESTART_RESET).await;
        assert_eq!(sup.restart_finished(), 1);
        assert_eq!(count(&sup), 1, "the count started over");
    }

    #[tokio::test]
    async fn old_state_discarded_on_start() {
        let h = harness();
        h.store
            .write(|txn, db| {
                db.put(txn, "svc/old.svc.emfor", r#"{"stale":true}"#)?;
                db.put(txn, "svc/api.svc.emfor", r#"{"stale":true}"#)
            })
            .unwrap();
        // Closed port: results are `down`, which cannot leave `unknown` before fall = 3.
        let (_closed, closed) = closed_port();
        let _sup = Supervisor::start(
            &conf(&format!(
                "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"{closed}\"]\n"
            )),
            h.tx.clone(),
        );
        // The start layout replaces both records in one commit; read the raw
        // value, since the stale `api` record is not a valid record.
        wait_for("old removed", || {
            h.store
                .get(&service_key("old.svc.emfor"))
                .unwrap()
                .is_none()
        })
        .await;
        let r = rec(&h.store, "api.svc.emfor").unwrap();
        assert_eq!(
            states(&h.store, "api.svc.emfor"),
            vec![HealthState::Unknown]
        );
        assert!(r.all_down);
    }

    #[tokio::test]
    async fn reload_timing_change_keeps_state() {
        let h = harness();
        let addr = http_ok().await;
        let mut sup = Supervisor::start(
            &conf(&http_service("api.svc.emfor", &addr, "/", "200ms")),
            h.tx.clone(),
        );
        wait_for("api healthy", || {
            states(&h.store, "api.svc.emfor") == vec![HealthState::Healthy]
        })
        .await;
        sup.reload(&conf(&http_service("api.svc.emfor", &addr, "/", "1s")));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            states(&h.store, "api.svc.emfor"),
            vec![HealthState::Healthy]
        );
    }

    #[tokio::test]
    async fn reload_check_options_change_resets_state() {
        let h = harness();
        let addr = http_ok().await;
        let mut sup = Supervisor::start(
            &conf(&http_service("api.svc.emfor", &addr, "/", "200ms")),
            h.tx.clone(),
        );
        wait_for("api healthy", || {
            states(&h.store, "api.svc.emfor") == vec![HealthState::Healthy]
        })
        .await;
        // `/healthz` answers 404: new checks are `down`, and fall = 3 keeps a
        // reset backend `unknown`, while a kept (not reset) one would stay `healthy`.
        sup.reload(&conf(&http_service(
            "api.svc.emfor",
            &addr,
            "/healthz",
            "200ms",
        )));
        wait_for("api reset to unknown", || {
            states(&h.store, "api.svc.emfor") == vec![HealthState::Unknown]
        })
        .await;
        let r = rec(&h.store, "api.svc.emfor").unwrap();
        assert_eq!(r.backends[0].state, HealthState::Unknown);
        assert!(
            r.backends[0]
                .reason
                .as_deref()
                .unwrap_or("")
                .contains("404")
                || r.backends[0].checked_at.is_none()
        );
    }

    #[derive(Debug, Default)]
    struct Forgetful(std::sync::Mutex<Vec<std::net::SocketAddr>>);
    impl crate::checker::Checker for Forgetful {
        fn check(
            &self,
            _: crate::checker::CheckRequest,
        ) -> futures::future::BoxFuture<'_, crate::checker::CheckResult> {
            Box::pin(async { crate::checker::CheckResult::Up })
        }
        fn forget(&self, t: std::net::SocketAddr) {
            self.0.lock().unwrap().push(t);
        }
    }
    struct ForgetfulFactory(Arc<Forgetful>);
    impl crate::checker::CheckerFactory for ForgetfulFactory {
        fn kind(&self) -> &'static str {
            "forgetful"
        }
        fn validate(
            &self,
            _: &toml::Table,
        ) -> Result<Arc<dyn crate::checker::Checker>, Vec<crate::checker::OptionError>> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn reload_tells_kept_checker_to_forget_removed_backends() {
        let h = harness();
        let chk = Arc::new(Forgetful::default());
        let mut reg = Registry::builtin();
        reg.register(Arc::new(ForgetfulFactory(chk.clone())));
        let svc = |b: &str| format!("[services.a]\ncheck = \"forgetful\"\nbackends = [{b}]\n");
        let parse = |s: &str| crate::config::parse(s, &reg).unwrap();
        let mut sup = Supervisor::start(
            &parse(&svc("\"10.0.0.1:80\", \"10.0.0.2:80\"")),
            h.tx.clone(),
        );
        sup.reload(&parse(&svc("\"10.0.0.1:80\"")));
        assert_eq!(
            *chk.0.lock().unwrap(),
            vec!["10.0.0.2:80".parse::<std::net::SocketAddr>().unwrap()]
        );
    }

    #[tokio::test]
    async fn reload_removes_service_and_adds_backends_as_unknown() {
        let h = harness();
        let addr = http_ok().await;
        let two = format!(
            "{}{}",
            http_service("api.svc.emfor", &addr, "/", "200ms"),
            http_service("old.svc.emfor", &addr, "/", "200ms")
        );
        let mut sup = Supervisor::start(&conf(&two), h.tx.clone());
        wait_for("api healthy and old written", || {
            states(&h.store, "api.svc.emfor") == vec![HealthState::Healthy]
                && rec(&h.store, "old.svc.emfor").is_some()
        })
        .await;
        let (_closed, closed) = closed_port();
        // At a 1s interval the added backend stays `unknown` (fall = 3) for
        // seconds after the reload, however late it is observed.
        let one = http_service("api.svc.emfor", &addr, "/", "1s").replace(
            &format!("backends = [\"{addr}\"]"),
            &format!("backends = [\"{addr}\", \"{closed}\"]"),
        );
        sup.reload(&conf(&one));
        wait_for("old removed", || rec(&h.store, "old.svc.emfor").is_none()).await;
        assert_eq!(
            states(&h.store, "api.svc.emfor"),
            vec![HealthState::Healthy, HealthState::Unknown]
        );
    }

    /// The peer registry follows reloads: removed backends leave it, a
    /// changed check gets a new digest, and reports reach running tasks.
    #[tokio::test]
    async fn peer_targets_follow_reloads() {
        use crate::peers::Down;
        let h = harness();
        let targets = Targets::default();
        let svc = |b: &str, timeout: &str| {
            conf(&format!(
                "[services.a]\ncheck = \"tcp\"\ntimeout = \"{timeout}\"\nbackends = [{b}]\n"
            ))
        };
        let first = svc("\"10.0.0.1:80\", \"10.0.0.2:80\"", "1s");
        let (out, _out_rx) = mpsc::unbounded_channel();
        let quorum = Arc::new(crate::quorum::Quorum::new("10.0.0.5:5302".parse().unwrap()));
        let hooks = crate::peers::PeerHooks {
            out,
            quorum: quorum.clone(),
            targets: targets.clone(),
        };
        let mut sup =
            Supervisor::start_with_peers(&first, h.tx.clone(), targets.clone(), Some(hooks));
        let digest = first.services[0].digest.clone();
        let vote = |backend: &str| crate::peers::ResultMsg {
            service: "a".into(),
            digest: digest.clone(),
            backend: backend.into(),
            outcome: crate::peers::Outcome::Up,
            interval_ms: 1000,
        };
        let now = tokio::time::Instant::now();
        let six = "10.0.0.6:5302".parse().unwrap();
        quorum.record(six, &vote("10.0.0.1:80"), now);
        quorum.record(six, &vote("10.0.0.2:80"), now);
        assert_eq!(targets.digest("a"), Some(digest.clone()));
        let down = |backend: &str, digest: &str| Down {
            service: "a".into(),
            digest: digest.into(),
            backend: backend.into(),
            reason: "refused".into(),
        };
        let from = "10.0.0.6:5302".parse().unwrap();
        assert_eq!(targets.deliver(&down("10.0.0.2:80", &digest), from), Ok(()));
        // Make 10.0.0.2:80 a disagreement: .5, .6 and .7 are its assigned
        // checkers, .6 and .7 see it down, this daemon up. No await until
        // the reload, so the backend tasks cannot clear it first.
        let seven = "10.0.0.7:5302".parse().unwrap();
        quorum.set_live([six, seven]);
        let gone = crate::peers::ResultMsg {
            outcome: crate::peers::Outcome::Down("refused".into()),
            ..vote("10.0.0.2:80")
        };
        quorum.record(six, &gone, now);
        quorum.record(seven, &gone, now);
        let c = quorum.combine(
            "a",
            "10.0.0.2:80",
            &digest,
            &crate::checker::CheckResult::Up,
            now,
        );
        assert_eq!(c.disagreement, Some((2, 3)));
        assert_eq!(quorum.disagreements(), 1);
        sup.reload(&svc("\"10.0.0.1:80\"", "1s"));
        assert_eq!(quorum.disagreements(), 0, "removed backend's disagreement");
        assert_eq!(
            targets.deliver(&down("10.0.0.2:80", &digest), from),
            Err("no such backend here")
        );
        assert_eq!(targets.deliver(&down("10.0.0.1:80", &digest), from), Ok(()));
        assert!(
            quorum.kept("a", "10.0.0.2:80", six).is_none(),
            "removed backend"
        );
        assert!(quorum.kept("a", "10.0.0.1:80", six).is_some());
        let changed = svc("\"10.0.0.1:80\"", "500ms");
        sup.reload(&changed);
        assert_ne!(changed.services[0].digest, digest);
        assert_eq!(
            targets.digest("a"),
            Some(changed.services[0].digest.clone())
        );
        sup.reload(&conf(
            "[services.b]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
        ));
        assert_eq!(targets.digest("a"), None);
        assert!(
            quorum.kept("a", "10.0.0.1:80", six).is_none(),
            "removed service"
        );
        sup.stop();
    }
}
