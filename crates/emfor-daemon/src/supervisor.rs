//! Owns the per-backend check tasks and keeps them, and the store layout, in
//! line with the current config on start and on every reload.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::config::{Config, ServiceConfig, Settings};
use crate::schedule::spawn_backend;
use crate::writer::{ServiceLayout, WriterMsg};

struct Running {
    config: ServiceConfig,
    generation: u64,
    settings: watch::Sender<Settings>,
    tasks: HashMap<String, JoinHandle<()>>,
}

impl Running {
    fn abort_all(&mut self) {
        for (_, t) in self.tasks.drain() {
            t.abort();
        }
    }
}

pub struct Supervisor {
    writer: mpsc::UnboundedSender<WriterMsg>,
    services: BTreeMap<Arc<str>, Running>,
    next_generation: u64,
}

impl Supervisor {
    /// Fresh start: the store drops every previous `svc/*` record and all
    /// backends start `unknown`.
    pub fn start(config: &Config, writer: mpsc::UnboundedSender<WriterMsg>) -> Self {
        let mut s = Supervisor {
            writer,
            services: BTreeMap::new(),
            next_generation: 1,
        };
        s.sync(config, true);
        s
    }

    /// Apply a validated reload.
    pub fn reload(&mut self, config: &Config) {
        self.sync(config, false);
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
                });
            let checker = r.config.checker.clone(); // unchanged check: keep the running checker
            r.config = ServiceConfig {
                checker: checker.clone(),
                ..new.clone()
            };
            r.settings.send_replace(new.settings);
            for b in &new.backends {
                if !r.tasks.contains_key(&b.id) {
                    let t = spawn_backend(
                        layout.name.clone(),
                        b.clone(),
                        checker.clone(),
                        layout.generation,
                        r.settings.subscribe(),
                        self.writer.clone(),
                    );
                    r.tasks.insert(b.id.clone(), t);
                }
            }
        }
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
}
