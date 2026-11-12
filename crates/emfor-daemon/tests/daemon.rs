//! End-to-end tests: run the `emfor` binary and read its LMDB store.

use std::collections::BTreeMap;
use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use emfor_format::{HealthState, ServiceRecord};
use heed::types::Str;
use heed::{Database, Env, EnvFlags, EnvOpenOptions, WithoutTls};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

/// Read-only view of a store, opened the way a client would.
struct Reader {
    env: Env<WithoutTls>,
    db: Database<Str, Str>,
}

impl Reader {
    fn open(path: &Path) -> Reader {
        let env = unsafe {
            let mut o = EnvOpenOptions::new().read_txn_without_tls();
            o.flags(EnvFlags::NO_SUB_DIR | EnvFlags::READ_ONLY);
            o.open(path).unwrap()
        };
        let rtxn = env.read_txn().unwrap();
        let db = env.open_database(&rtxn, None).unwrap().unwrap();
        rtxn.commit().unwrap();
        Reader { env, db }
    }

    fn dump(&self) -> BTreeMap<String, String> {
        let rtxn = self.env.read_txn().unwrap();
        self.db
            .iter(&rtxn)
            .unwrap()
            .map(|r| r.unwrap())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn get(&self, key: &str) -> Option<String> {
        self.dump().remove(key)
    }

    fn service(&self, name: &str) -> Option<ServiceRecord> {
        self.get(&format!("svc/{name}"))
            .map(|j| serde_json::from_str(&j).unwrap())
    }

    fn svc_keys(&self) -> Vec<String> {
        self.dump()
            .into_keys()
            .filter(|k| k.starts_with("svc/"))
            .collect()
    }
}

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
}

impl Daemon {
    fn store(&self) -> PathBuf {
        self.dir.path().join("state.mdb")
    }
    fn config(&self) -> PathBuf {
        self.dir.path().join("emfor.toml")
    }
    fn signal(&self, s: Signal) {
        kill(Pid::from_raw(self.child.id() as i32), s).unwrap();
    }
    /// Stop with SIGTERM and return (exit code, stderr).
    fn stop(mut self) -> (i32, String) {
        self.signal(Signal::SIGTERM);
        let status = self.child.wait().unwrap();
        let mut err = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        (status.code().unwrap_or(-1), err)
    }
    fn running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn store_header(dir: &Path) -> String {
    format!(
        "[store]\npath = \"{}\"\nmap_size = \"4MiB\"\n\n",
        dir.join("state.mdb").display()
    )
}

fn tcp_service(name: &str, addr: &str) -> String {
    format!(
        "[services.\"{name}\"]\ncheck = \"tcp\"\nbackends = [\"{addr}\"]\ninterval = \"200ms\"\ntimeout = \"100ms\"\nrise = 2\nfall = 2\njitter = \"0%\"\n\n"
    )
}

fn start(services: &str) -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(&cfg, format!("{}{services}", store_header(dir.path()))).unwrap();
    let child = Command::new(cargo_bin("emfor"))
        .arg("--config")
        .arg(&cfg)
        .env_remove("RUST_LOG")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let d = Daemon { child, dir };
    wait_for(|| {
        d.store().exists()
            && std::fs::metadata(d.store())
                .map(|m| m.len() > 0)
                .unwrap_or(false)
    });
    d
}

fn wait_for(mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("condition not met within 10s");
}

fn listener() -> (TcpListener, String) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap().to_string();
    (l, a)
}

#[test]
fn sighup_with_new_service_adds_its_record() {
    let (_l, addr) = listener();
    let d = start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    wait_for(|| r.service("api.svc.emfor").is_some());
    let cfg = format!(
        "{}{}{}",
        store_header(d.dir.path()),
        tcp_service("api.svc.emfor", &addr),
        tcp_service("web.svc.emfor", &addr)
    );
    std::fs::write(d.config(), cfg).unwrap();
    d.signal(Signal::SIGHUP);
    wait_for(|| r.service("web.svc.emfor").is_some());
    wait_for(|| r.service("web.svc.emfor").unwrap().backends[0].state == HealthState::Healthy);
}

#[test]
fn invalid_reload_keeps_previous_config() {
    let (_l, addr) = listener();
    let mut d = start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.service("api.svc.emfor")
            .is_some_and(|s| s.backends[0].state == HealthState::Healthy)
    });
    let before = r.svc_keys();
    let bad = format!("{}{}", store_header(d.dir.path()), tcp_service("api.svc.emfor", &addr))
        .replace("timeout = \"100ms\"", "timeout = \"5s\"")
        .replace("interval = \"200ms\"", "interval = \"2s\"\n[services.\"x.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:1\"]");
    std::fs::write(d.config(), bad).unwrap();
    d.signal(Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(500));
    assert!(d.running());
    assert_eq!(r.svc_keys(), before);
    assert_eq!(
        r.service("api.svc.emfor").unwrap().backends[0].state,
        HealthState::Healthy
    );
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    assert!(err.contains("timeout must not exceed interval"), "{err}");
    assert!(err.contains("reload rejected"), "{err}");
}

#[test]
fn store_change_on_reload_requires_restart() {
    let (_l, addr) = listener();
    let d = start(&tcp_service("api.svc.emfor", &addr));
    let cfg = format!(
        "[store]\npath = \"{}\"\n\n{}",
        d.dir.path().join("other.mdb").display(),
        tcp_service("api.svc.emfor", &addr)
    );
    std::fs::write(d.config(), cfg).unwrap();
    d.signal(Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(300));
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    assert!(err.contains("store settings require restart"), "{err}");
}

#[test]
fn sigterm_exits_zero_and_zeroes_heartbeat() {
    let (_l, addr) = listener();
    let d = start(&tcp_service("api.svc.emfor", &addr));
    let store = d.store();
    let r = Reader::open(&store);
    wait_for(|| r.get("meta/heartbeat").is_some_and(|h| h != "0"));
    let (code, _) = d.stop();
    assert_eq!(code, 0);
    assert_eq!(r.get("meta/heartbeat").as_deref(), Some("0"));
}

#[test]
fn second_daemon_on_same_store_exits_one() {
    let (_l, addr) = listener();
    let mut d = start(&tcp_service("api.svc.emfor", &addr));
    let out = Command::new(cargo_bin("emfor"))
        .arg("--config")
        .arg(d.config())
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!("{}.writer", d.store().display())),
        "{err}"
    );
    assert!(d.running());
}

#[test]
fn check_config_valid_and_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(
        &cfg,
        "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\", \"10.0.0.2:80\"]\n\n[services.\"db.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.1.1:5432\", \"10.0.1.2:5432\", \"[2001:db8::1]:5432\"]\n",
    )
    .unwrap();
    let out = Command::new(cargo_bin("emfor"))
        .args(["--check-config", "--config"])
        .arg(&cfg)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "config ok: 2 services, 5 backends"
    );

    std::fs::write(&cfg, "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\nintervall = \"1s\"\n").unwrap();
    let out = Command::new(cargo_bin("emfor"))
        .args(["--check-config", "--config"])
        .arg(&cfg)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("services.\"api.svc.emfor\".intervall"));
}

#[test]
fn invalid_config_at_startup_exits_two_without_store() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    let store = dir.path().join("state.mdb");
    std::fs::write(
        &cfg,
        format!(
            "{}[services.a]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\nrise = 0\n",
            store_header(dir.path())
        ),
    )
    .unwrap();
    let out = Command::new(cargo_bin("emfor"))
        .arg("--config")
        .arg(&cfg)
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("rise must be at least 1"));
    assert!(!store.exists());
}

#[test]
fn example_config_is_valid() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/emfor.toml");
    let text = std::fs::read_to_string(&example).unwrap();
    for line in ["[dns]", "balance = \"failover\"", "dns_ttl = "] {
        assert!(
            text.lines().any(|l| l.starts_with(line)),
            "example shows {line} uncommented"
        );
    }
    let out = Command::new(cargo_bin("emfor"))
        .args(["--check-config", "--config"])
        .arg(&example)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "config ok: 2 services, 5 backends"
    );
}

/// Acceptance: a backend that stops accepting connections becomes unhealthy
/// after `fall` checks, and healthy again after `rise` checks once it listens.
#[test]
fn backend_goes_unhealthy_and_recovers() {
    let (l, addr) = listener();
    let d = start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    let state = || r.service("api.svc.emfor").map(|s| s.backends[0].state);
    wait_for(|| state() == Some(HealthState::Healthy));
    assert!(!r.service("api.svc.emfor").unwrap().all_down);

    drop(l);
    wait_for(|| state() == Some(HealthState::Unhealthy));
    let rec = r.service("api.svc.emfor").unwrap();
    assert!(rec.all_down);
    assert!(
        rec.backends[0]
            .reason
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains("refused")
    );

    let _l2 = TcpListener::bind(&addr).unwrap();
    wait_for(|| state() == Some(HealthState::Healthy));
    assert_eq!(r.service("api.svc.emfor").unwrap().backends[0].reason, None);
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

#[test]
fn reload_reports_store_change_together_with_other_errors() {
    let (_l, addr) = listener();
    let d = start(&tcp_service("api.svc.emfor", &addr));
    let cfg = format!(
        "[store]\npath = \"{}\"\n\n{}",
        d.dir.path().join("other.mdb").display(),
        tcp_service("api.svc.emfor", &addr).replace("rise = 2", "rise = 0")
    );
    std::fs::write(d.config(), cfg).unwrap();
    d.signal(Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(300));
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    assert!(err.contains("rise must be at least 1"), "{err}");
    assert!(err.contains("store settings require restart"), "{err}");
}

#[test]
fn reload_applies_log_level() {
    let (_l, addr) = listener();
    let d = start(&tcp_service("api.svc.emfor", &addr));
    let cfg = format!(
        "{}[log]\nlevel = \"debug\"\n\n{}",
        store_header(d.dir.path()),
        tcp_service("api.svc.emfor", &addr)
    );
    std::fs::write(d.config(), cfg).unwrap();
    d.signal(Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(300));
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    assert!(err.contains("log level set to debug"), "{err}");
}

/// Spec scenario "Service record after checks": one healthy and one unhealthy
/// backend, in config order, with `all_down` false and a reason on the failed one.
#[test]
fn service_record_with_healthy_and_unhealthy_backends() {
    let (_l, up) = listener();
    let (closed, down) = listener();
    drop(closed);
    let svc = tcp_service("api.svc.emfor", &up).replace(
        &format!("backends = [\"{up}\"]"),
        &format!("backends = [\"{up}\", \"{down}\"]"),
    );
    let d = start(&svc);
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.service("api.svc.emfor").is_some_and(|s| {
            s.backends[0].state == HealthState::Healthy
                && s.backends[1].state == HealthState::Unhealthy
        })
    });
    let rec = r.service("api.svc.emfor").unwrap();
    assert!(!rec.all_down);
    assert_eq!(
        rec.backends
            .iter()
            .map(|b| b.id.as_str())
            .collect::<Vec<_>>(),
        vec![up.as_str(), down.as_str()]
    );
    assert_eq!(rec.backends[0].reason, None);
    assert!(rec.backends[1].reason.is_some());
}

#[test]
fn balance_written_from_config_and_changed_on_reload() {
    use emfor_format::Balance;
    let (_l, addr) = listener();
    let svc = tcp_service("api.svc.emfor", &addr);
    let d = start(&format!("{svc}balance = \"failover\"\n"));
    let r = Reader::open(&d.store());
    wait_for(|| r.service("api.svc.emfor").is_some());
    let rec = r.get("svc/api.svc.emfor").unwrap();
    assert!(
        rec.contains(r#""on_all_down":"open","balance":"failover","all_down""#),
        "{rec}"
    );
    std::fs::write(
        d.config(),
        format!("{}{svc}balance = \"sticky\"\n", store_header(d.dir.path())),
    )
    .unwrap();
    d.signal(Signal::SIGHUP);
    wait_for(|| r.service("api.svc.emfor").unwrap().balance == Balance::Sticky);
}
