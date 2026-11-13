//! End-to-end tests: run the `emfor` binary and read its LMDB store.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use emfor_format::{HealthState, ServiceHistory, ServiceRecord};
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
    /// The daemon's stderr, read as it comes so a long log never fills the
    /// pipe and blocks the daemon.
    log: std::sync::Arc<std::sync::Mutex<String>>,
    log_reader: Option<std::thread::JoinHandle<()>>,
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
        let code = self.wait();
        (code, self.log())
    }
    /// Wait for the daemon to exit and for its stderr to be read.
    fn wait(&mut self) -> i32 {
        let status = self.child.wait().unwrap();
        if let Some(r) = self.log_reader.take() {
            r.join().unwrap();
        }
        status.code().unwrap_or(-1)
    }
    fn log(&self) -> String {
        self.log.lock().unwrap().clone()
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
    start_in(tempfile::tempdir().unwrap(), services)
}

fn start_in(dir: tempfile::TempDir, services: &str) -> Daemon {
    start_with_env(dir, services, &[])
}

/// `start_in` with extra environment variables for the daemon.
fn start_with_env(dir: tempfile::TempDir, services: &str, env: &[(&str, &Path)]) -> Daemon {
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(&cfg, format!("{}{services}", store_header(dir.path()))).unwrap();
    let mut child = Command::new(cargo_bin("emfor"))
        .arg("server")
        .arg("--config")
        .arg(&cfg)
        .envs(env.iter().copied())
        .env_remove("RUST_LOG")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let log = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let mut stderr = child.stderr.take().unwrap();
    let sink = log.clone();
    let log_reader = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    let d = Daemon {
        child,
        dir,
        log,
        log_reader: Some(log_reader),
    };
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

/// A state writer that panics while the daemon runs (a store commit that
/// panics) makes the daemon log it and exit with status 70, which the
/// systemd unit restarts, instead of running on with a stale store.
#[test]
fn daemon_exits_70_when_the_writer_panics() {
    let dir = tempfile::tempdir().unwrap();
    let trigger = dir.path().join("panic-now");
    let mut d = start_with_env(
        dir,
        &tcp_service("api.svc.emfor", "127.0.0.1:1"),
        &[("EMFOR_TEST_COMMIT_PANIC_FILE", &trigger)],
    );
    wait_for(|| d.log().contains("emfor started"));
    assert!(d.running(), "daemon stopped before the commit panicked");
    std::fs::write(&trigger, "").unwrap();
    wait_for(|| !d.running());
    let code = d.wait();
    let log = d.log();
    assert_eq!(code, 70, "{log}");
    assert!(log.contains("test hook: a commit panics"), "{log}");
    assert!(log.contains("state writer panicked; exiting"), "{log}");
}

/// A port that is free for UDP and TCP on 127.0.0.1 at the time of the call.
fn free_port() -> u16 {
    loop {
        let u = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let p = u.local_addr().unwrap().port();
        if TcpListener::bind(("127.0.0.1", p)).is_ok() {
            return p;
        }
    }
}

/// Start with a `[dns]` table on a free port. Tests run in parallel, so
/// another test can take the port between `free_port` and the daemon's bind;
/// then try again with a new port.
fn start_dns(services: &str) -> (Daemon, String) {
    for _ in 0..5 {
        let dns = format!("[dns]\nlisten = [\"127.0.0.1:{}\"]\n\n", free_port());
        let mut d = start(&format!("{dns}{services}"));
        std::thread::sleep(Duration::from_millis(200));
        if d.running() {
            return (d, dns);
        }
        d.wait();
        let err = d.log();
        assert!(err.contains("Address already in use"), "{err}");
    }
    panic!("no free DNS port after 5 tries");
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

/// Spec "Damaged store is replaced at start": a store file whose meta
/// pages are zero bytes, as after a power loss with `MDB_NOSYNC`.
#[test]
fn damaged_store_is_replaced_at_start() {
    use std::io::Write;
    let (_l, addr) = listener();
    // A good store from a run that stopped, then its two meta pages zeroed.
    let mut d = start(&tcp_service("api.svc.emfor", &addr));
    let (path, page) = {
        let r = Reader::open(&d.store());
        wait_for(|| r.service("api.svc.emfor").is_some());
        (d.store(), u64::from(r.env.stat().page_size))
    };
    let dir = std::mem::replace(&mut d.dir, tempfile::tempdir().unwrap());
    let (code, err) = d.stop();
    assert_eq!(code, 0, "{err}");
    let len = std::fs::metadata(&path).unwrap().len();
    assert!(len > 2 * page, "store has data pages");
    let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.write_all(&vec![0u8; 2 * page as usize]).unwrap();
    drop(f);
    let corrupt = dir.path().join("state.mdb.corrupt");
    let d = start_in(dir, &tcp_service("api.svc.emfor", &addr));
    wait_for(|| corrupt.exists());
    let r = Reader::open(&d.store());
    wait_for(|| r.get("meta/heartbeat").is_some_and(|h| h != "0"));
    assert!(r.service("api.svc.emfor").is_some());
    assert_eq!(std::fs::metadata(&corrupt).unwrap().len(), len);
    let store = d.store().display().to_string();
    let (code, err) = d.stop();
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("is damaged") && err.contains(&store), "{err}");
}

/// Spec "Reasons are bounded and printable": an escape sequence from a
/// check script reaches the store only escaped.
#[test]
fn escape_sequence_in_a_reason_is_stored_escaped() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("check.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf 'bad\\033[2Jstate\\n'\nexit 1\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let svc = format!(
        "[services.\"x.svc.emfor\"]\ncheck = \"exec\"\nbackends = [\"127.0.0.1:1\"]\ninterval = \"200ms\"\ntimeout = \"100ms\"\njitter = \"0%\"\n[services.\"x.svc.emfor\".check_options]\ncommand = [\"{}\"]\n\n",
        script.display()
    );
    let d = start_in(dir, &svc);
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.service("x.svc.emfor")
            .is_some_and(|s| s.backends[0].reason.is_some())
    });
    let reason = r.service("x.svc.emfor").unwrap().backends[0].reason.clone();
    assert_eq!(reason.as_deref(), Some("bad\\u{1b}[2Jstate"));
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

/// Spec "Check scheduling with jitter and no overlap": checks run at the
/// fast pace while a backend's state is unknown.
#[test]
fn backend_known_within_a_second_of_start() {
    let (_l, addr) = listener();
    let svc = format!(
        "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"{addr}\"]\ninterval = \"2s\"\ntimeout = \"1s\"\nrise = 2\njitter = \"0%\"\n\n"
    );
    let d = start(&svc);
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.service("api.svc.emfor")
            .is_some_and(|s| s.backends[0].state == HealthState::Healthy)
    });
    // From the daemon's own start time to the check that made it healthy.
    let started: u64 = r.get("meta/started_at").unwrap().parse().unwrap();
    let healthy_at = r.service("api.svc.emfor").unwrap().backends[0]
        .checked_at
        .unwrap();
    let took = healthy_at.saturating_sub(started);
    assert!(took < 1000, "healthy {took} ms after the start");
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

#[test]
fn second_daemon_on_same_store_exits_one() {
    let (_l, addr) = listener();
    let mut d = start(&tcp_service("api.svc.emfor", &addr));
    let out = Command::new(cargo_bin("emfor"))
        .arg("server")
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
    assert!(!PathBuf::from(format!("{}.corrupt", d.store().display())).exists());
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
        .args(["check-config", "--config"])
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
        .args(["check-config", "--config"])
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
        .arg("server")
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
    for line in [
        "[dns]",
        "site = ",
        "balance = \"failover\"",
        "balance = \"weighted\"",
        "balance = \"locality\"",
        "dns_ttl = ",
        "expect = ",
        "tls = true",
    ] {
        assert!(
            text.lines().any(|l| l.starts_with(line)),
            "example shows {line} uncommented"
        );
    }
    let out = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--config"])
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
        "config ok: 7 services, 17 backends"
    );
}

/// Write `pairs` into a store at `path` (created if missing) before the
/// daemon runs, as a previous run would have left them.
fn seed_store(path: &Path, pairs: &[(&str, &str)]) {
    let env = unsafe {
        let mut o = EnvOpenOptions::new().read_txn_without_tls();
        o.flags(EnvFlags::NO_SUB_DIR).map_size(4 << 20);
        o.open(path).unwrap()
    };
    let mut wtxn = env.write_txn().unwrap();
    let db: Database<Str, Str> = env.create_database(&mut wtxn, None).unwrap();
    for (k, v) in pairs {
        db.put(&mut wtxn, k, v).unwrap();
    }
    wtxn.commit().unwrap();
}

/// Spec state-store "Backend state history", scenario "Unparseable history
/// discarded".
#[test]
fn unparseable_history_discarded_at_start() {
    let dir = tempfile::tempdir().unwrap();
    seed_store(
        &dir.path().join("state.mdb"),
        &[("hist/api.svc.emfor", "not json")],
    );
    let (_l, addr) = listener();
    let d = start_in(dir, &tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.get("hist/api.svc.emfor")
            .and_then(|v| serde_json::from_str::<ServiceHistory>(&v).ok())
            .is_some_and(|h| h.backends.contains_key(&addr))
    });
    let h: ServiceHistory = serde_json::from_str(&r.get("hist/api.svc.emfor").unwrap()).unwrap();
    assert_eq!(h.backends[&addr][0].from, HealthState::Unknown);
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    assert!(err.contains("discarding hist/api.svc.emfor"), "{err}");
}

/// Acceptance: a backend that stops accepting connections becomes unhealthy
/// after `fall` checks, and healthy again after `rise` checks once it listens.
#[test]
fn backend_goes_unhealthy_and_recovers() {
    let (l, addr) = listener();
    let d = start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    let state = || r.service("api.svc.emfor").map(|s| s.backends[0].state);
    let latency = || {
        r.service("api.svc.emfor")
            .and_then(|s| s.backends[0].latency_us)
    };
    wait_for(|| state() == Some(HealthState::Healthy));
    assert!(!r.service("api.svc.emfor").unwrap().all_down);
    // Spec "Check latency": a number after up checks (committed by the next
    // heartbeat at the latest), null once the backend is unhealthy.
    wait_for(|| latency().is_some());

    drop(l);
    wait_for(|| state() == Some(HealthState::Unhealthy));
    let rec = r.service("api.svc.emfor").unwrap();
    assert!(rec.all_down);
    assert_eq!(rec.backends[0].latency_us, None);
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
    wait_for(|| latency().is_some());
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

/// Spec scenario "Mixed ports in the zone": a warning at startup naming both
/// ports, and A queries still answered.
#[test]
fn startup_reports_backends_on_different_ports() {
    use hickory_proto::op::{Message, Query};
    use hickory_proto::rr::{Name, RecordType};
    let (_l1, a1) = listener();
    let (_l2, a2) = listener();
    let svc = tcp_service("api.svc.emfor", &a1).replace(
        &format!("backends = [\"{a1}\"]"),
        &format!("backends = [\"{a1}\", \"{a2}\"]"),
    );
    let (d, dns) = start_dns(&svc);
    let dns_port: u16 = dns
        .split(':')
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let mut q = Message::query();
    q.add_query(Query::query(
        Name::from_ascii("api.svc.emfor.").unwrap(),
        RecordType::A,
    ));
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let mut answered = false;
    for _ in 0..20 {
        sock.send_to(&q.to_vec().unwrap(), ("127.0.0.1", dns_port))
            .unwrap();
        let mut buf = [0u8; 512];
        if let Ok(n) = sock.recv(&mut buf) {
            let m = Message::from_vec(&buf[..n]).unwrap();
            if m.answers.len() == 1 {
                answered = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let (code, err) = d.stop();
    assert_eq!(code, 0, "{err}");
    assert!(answered, "no A answer: {err}");
    let mut ports: Vec<u16> = [&a1, &a2]
        .iter()
        .map(|a| a.rsplit_once(':').unwrap().1.parse().unwrap())
        .collect();
    ports.sort();
    let line = err
        .lines()
        .find(|l| l.contains("different ports"))
        .unwrap_or_else(|| panic!("no port warning: {err}"));
    assert!(
        line.contains("WARN") && line.contains("api.svc.emfor"),
        "{line}"
    );
    assert!(
        line.contains(&format!("({}, {})", ports[0], ports[1])),
        "{line}"
    );
}

/// Spec scenario "Reload reports the new config": no port warning while all
/// backends share a port, a warning naming both ports after a reload adds
/// a backend on another port.
#[test]
fn reload_reports_backends_on_different_ports() {
    let (_l, addr) = listener();
    let (d, dns) = start_dns(&tcp_service("api.svc.emfor", &addr));
    let port = addr.rsplit_once(':').unwrap().1;
    let other = if port == "9090" { "9091" } else { "9090" };
    let cfg = format!(
        "{}{dns}{}",
        store_header(d.dir.path()),
        tcp_service("api.svc.emfor", &addr).replace(
            &format!("backends = [\"{addr}\"]"),
            &format!("backends = [\"{addr}\", \"127.0.0.2:{other}\"]")
        )
    );
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(d.config(), cfg).unwrap();
    d.signal(Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(500));
    let (code, err) = d.stop();
    assert_eq!(code, 0, "{err}");
    let mut ports = [port.parse::<u16>().unwrap(), other.parse().unwrap()];
    ports.sort();
    let warn_lines: Vec<&str> = err
        .lines()
        .filter(|l| l.contains("different ports"))
        .collect();
    assert_eq!(warn_lines.len(), 1, "{err}");
    let line = warn_lines[0];
    assert!(line.contains("WARN"), "{line}");
    assert!(line.contains("api.svc.emfor"), "{line}");
    assert!(
        line.contains(&format!("({}, {})", ports[0], ports[1])),
        "{line}"
    );
    let reloaded = err.find("config reloaded").expect("reload logged");
    assert!(err.find("different ports").unwrap() < reloaded, "{err}");
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

fn fastest_service(name: &str, check: &str, backends: &[&str]) -> String {
    let list: Vec<String> = backends.iter().map(|b| format!("\"{b}\"")).collect();
    format!(
        "[services.\"{name}\"]\ncheck = \"{check}\"\nbalance = \"fastest\"\nbackends = [{}]\ninterval = \"200ms\"\ntimeout = \"150ms\"\nrise = 1\nfall = 1\njitter = \"0%\"\n\n",
        list.join(", ")
    )
}

/// HTTP backend answering every request with 200 after `delay`.
fn http_backend(delay: Duration) -> String {
    use std::io::Write;
    let (l, addr) = listener();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut s) = conn else { continue };
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                std::thread::sleep(delay);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            });
        }
    });
    addr
}

#[test]
fn fastest_pool_follows_health() {
    let (l, addr) = listener();
    let d = start(&fastest_service("api.svc.emfor", "tcp", &[&addr]));
    let r = Reader::open(&d.store());
    let pool = || r.service("api.svc.emfor").and_then(|s| s.pool);
    wait_for(|| pool() == Some(vec![addr.clone()]));
    assert_eq!(
        r.service("api.svc.emfor").unwrap().balance,
        emfor_format::Balance::Fastest
    );
    drop(l);
    wait_for(|| pool() == Some(vec![]));
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

#[test]
fn fastest_pool_excludes_slow_backend() {
    let fast = http_backend(Duration::ZERO);
    let slow = http_backend(Duration::from_millis(30));
    let d = start(&fastest_service("api.svc.emfor", "http", &[&fast, &slow]));
    let r = Reader::open(&d.store());
    let ready = || {
        r.service("api.svc.emfor").is_some_and(|s| {
            s.backends
                .iter()
                .all(|b| b.state == HealthState::Healthy && b.latency_us.is_some())
        })
    };
    wait_for(ready);
    // Commits of pool-only changes follow the heartbeat; watch for 2 s.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let s = r.service("api.svc.emfor").unwrap();
        assert_eq!(s.pool, Some(vec![fast.clone()]), "{s:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

#[test]
fn help_shows_the_built_in_config_path() {
    let out = Command::new(cargo_bin("emfor"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    // Tests are normally built without EMFOR_PREFIX; with it, only the
    // suffix is known here.
    let want = match std::env::var("EMFOR_PREFIX") {
        Err(_) => "[default: /etc/emfor/emfor.toml]".to_owned(),
        Ok(_) => "etc/emfor/emfor.toml]".to_owned(),
    };
    assert!(help.contains(&want), "{help}");
}

/// Run `emfor <args>`: (exit code, stdout, stderr). Fails instead of
/// hanging when emfor has not exited within 10 s (for example, because it
/// started a daemon).
fn emfor(args: &[&str]) -> (i32, String, String) {
    let mut child = Command::new(cargo_bin("emfor"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("emfor {args:?} still running after 10s");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Service-config "Command line subcommands": without a usable subcommand,
/// emfor prints usage or the `--check-config` hint, exits 2, and leaves
/// the store alone.
#[test]
fn no_subcommand_prints_usage_and_touches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    let store = dir.path().join("state.mdb");
    std::fs::write(
        &cfg,
        format!(
            "{}[services.a]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
            store_header(dir.path())
        ),
    )
    .unwrap();
    let cfg = cfg.to_str().unwrap();
    let hint = "--check-config was replaced by \"emfor check-config\"";
    let rows: [(&[&str], &[&str]); 5] = [
        (&[], &["server", "check-config", "status"]),
        (&["--config", cfg], &["server", "check-config", "status"]),
        (&["--check-config"], &[hint]),
        (&["--check-config", "--config", cfg], &[hint]),
        (&["server", "--check-config"], &[hint]),
    ];
    for (args, want) in rows {
        let (code, out, err) = emfor(args);
        assert_eq!(code, 2, "emfor {args:?}: {err}");
        assert_eq!(out, "", "emfor {args:?}");
        for w in want {
            assert!(err.contains(w), "emfor {args:?}: no {w:?} in {err}");
        }
    }
    for f in ["state.mdb", "state.mdb-lock", "state.mdb.writer"] {
        assert!(!dir.path().join(f).exists(), "{f} was created");
    }
    assert!(!store.exists());
}

/// Service-config "Command line subcommands", scenario "No subcommand":
/// beside a running daemon, `emfor` prints usage and the daemon keeps
/// publishing heartbeats.
#[test]
fn no_subcommand_beside_a_running_daemon() {
    let (_l, addr) = listener();
    let mut d = start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    let heartbeat = || r.dump().get("meta/heartbeat").cloned().unwrap_or_default();
    wait_for(|| r.service("api.svc.emfor").is_some());
    let cfg = d.config();
    for args in [vec![], vec!["--config", cfg.to_str().unwrap()]] {
        let (code, _, err) = emfor(&args);
        assert_eq!(code, 2, "emfor {args:?}: {err}");
        assert!(err.contains("server"), "emfor {args:?}: {err}");
    }
    assert!(d.running());
    let before = heartbeat();
    wait_for(|| heartbeat() != before);
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

#[test]
fn config_option_before_or_after_the_subcommand() {
    let example = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/emfor.toml");
    let before = emfor(&["--config", example, "check-config"]);
    let after = emfor(&["check-config", "--config", example]);
    assert_eq!(before.0, 0, "{}", before.2);
    assert!(before.1.starts_with("config ok: "), "{}", before.1);
    assert_eq!(before, after);
}

#[test]
fn help_does_not_list_the_old_flag() {
    let (code, out, _) = emfor(&["--help"]);
    assert_eq!(code, 0);
    assert!(!out.contains("--check-config"), "{out}");
    for w in ["server", "check-config", "status"] {
        assert!(out.contains(w), "no {w} in {out}");
    }
}

/// Run `emfor status --config <cfg> <args>`: (exit code, stdout, stderr).
fn status(cfg: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(cargo_bin("emfor"))
        .arg("status")
        .arg("--config")
        .arg(cfg)
        .args(args)
        .env_remove("NO_COLOR")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn status_beside_a_running_daemon() {
    let (_l, addr) = listener();
    let d = start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.service("api.svc.emfor")
            .is_some_and(|s| s.backends[0].state == HealthState::Healthy)
    });

    let (code, out, err) = status(&d.config(), &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("daemon: running, heartbeat "), "{out}");
    assert!(
        out.contains("api.svc.emfor  round-robin  on_all_down=open\n"),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l.starts_with(&format!("  {addr}  healthy  "))),
        "{out}"
    );
    assert!(
        !out.contains('\x1b'),
        "escape sequences in piped output: {out:?}"
    );

    // The daemon keeps writing heartbeats after status read the store.
    let hb = r.get("meta/heartbeat").unwrap();
    wait_for(|| r.get("meta/heartbeat").is_some_and(|h| h != hb));

    let (code, out, _) = status(&d.config(), &["--json", "--service", "api.svc.emfor"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["daemon"], "running");
    assert_eq!(v["services"].as_array().unwrap().len(), 1);
    assert_eq!(v["services"][0]["name"], "api.svc.emfor");

    let (code, _, err) = status(&d.config(), &["--service", "nope.svc.emfor"]);
    assert_eq!((code, err.trim()), (2, "service nope.svc.emfor not found"));

    // A broken service table does not stop status: only [store] is read.
    let text = std::fs::read_to_string(d.config()).unwrap();
    std::fs::write(
        d.config(),
        text.replace("check = \"tcp\"", "check = \"tcp\"\nintervall = \"1s\""),
    )
    .unwrap();
    let (code, out, err) = status(&d.config(), &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("api.svc.emfor"));

    // Stop the daemon but keep its directory (Daemon::stop drops it).
    let mut d = d;
    d.signal(Signal::SIGTERM);
    assert!(d.child.wait().unwrap().success());
    let (code, out, _) = status(&d.config(), &[]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("daemon: stopped\n"), "{out}");
    assert!(out.contains("api.svc.emfor"), "{out}");
}

/// Status-command "Status text output" and "Status filtering and JSON
/// output": the daemon line and `--json` show the `[dns]` and `[metrics]`
/// addresses the running daemon wrote to `meta/dns_listen` and
/// `meta/metrics_listen`.
#[test]
fn status_shows_listen_addresses() {
    let (_l, backend) = listener();
    // Another test can take a port between picking and binding it.
    let (d, dns, metrics) = (0..5)
        .find_map(|_| {
            let dns = format!("127.0.0.1:{}", free_port());
            let metrics = format!("127.0.0.1:{}", free_port());
            let mut d = start(&format!(
                "[dns]\nlisten = [\"{dns}\"]\n\n[metrics]\nlisten = [\"{metrics}\"]\n\n{}",
                tcp_service("api.svc.emfor", &backend)
            ));
            std::thread::sleep(Duration::from_millis(200));
            d.running().then_some((d, dns, metrics))
        })
        .expect("no free ports after 5 tries");
    let r = Reader::open(&d.store());
    // Exit status 0 needs a healthy backend.
    wait_for(|| {
        r.service("api.svc.emfor")
            .is_some_and(|s| s.backends[0].state == HealthState::Healthy)
    });
    assert_eq!(r.get("meta/dns_listen").unwrap(), format!("[\"{dns}\"]"));
    assert_eq!(
        r.get("meta/metrics_listen").unwrap(),
        format!("[\"{metrics}\"]")
    );
    let (code, out, err) = status(&d.config(), &[]);
    assert_eq!(code, 0, "{out}{err}");
    let line = out.lines().nth(1).unwrap();
    assert!(line.starts_with("daemon: running, "), "{out}");
    assert!(
        line.ends_with(&format!(", dns {dns}, metrics {metrics}")),
        "{out}"
    );
    let (code, out, _) = status(&d.config(), &["--json"]);
    assert_eq!(code, 0, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["dns_listen"], serde_json::json!([dns]));
    assert_eq!(v["metrics_listen"], serde_json::json!([metrics]));
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

#[test]
fn status_without_a_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("none.mdb");
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(&cfg, format!("[store]\npath = \"{}\"\n", store.display())).unwrap();
    let (code, _, err) = status(&cfg, &[]);
    assert_eq!(code, 2);
    assert_eq!(
        err.trim(),
        format!("cannot open store {}: no such file", store.display())
    );
    assert!(!store.exists());
    assert!(!dir.path().join("none.mdb-lock").exists());
}

#[test]
fn status_refuses_another_format_version() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("state.mdb");
    {
        let env = unsafe {
            let mut o = EnvOpenOptions::new().read_txn_without_tls();
            o.flags(EnvFlags::NO_SUB_DIR);
            o.open(&store).unwrap()
        };
        let mut wtxn = env.write_txn().unwrap();
        let db: Database<Str, Str> = env.create_database(&mut wtxn, None).unwrap();
        db.put(&mut wtxn, "meta/schema_version", "2").unwrap();
        wtxn.commit().unwrap();
    }
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(&cfg, format!("[store]\npath = \"{}\"\n", store.display())).unwrap();
    let (code, _, err) = status(&cfg, &[]);
    assert_eq!(code, 2);
    assert_eq!(
        err.trim(),
        "store format version 2; this emfor reads version 1"
    );
}

#[test]
fn status_watch_needs_a_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(&cfg, "").unwrap();
    let (code, _, err) = status(&cfg, &["--watch"]);
    assert_eq!((code, err.trim()), (2, "--watch needs a terminal"));
}

/// How a test ends `status --watch`.
enum WatchStop {
    Signal(Signal),
    Keys(&'static [u8]),
}

/// Run `status --watch` on a new pseudo-terminal (its standard input and
/// output), end it with `stop`, and return its exit status, everything it
/// wrote, and the terminal's `c_lflag` afterwards.
fn watch_on_a_pty(cfg: &Path, stop: &WatchStop) -> (Option<i32>, Vec<u8>, libc::tcflag_t) {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::{Arc, Mutex};
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty fills two new descriptors, owned below.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0);
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let mut child = Command::new(cargo_bin("emfor"))
        .arg("status")
        .arg("--config")
        .arg(cfg)
        .arg("--watch")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = std::fs::File::from(master.try_clone().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reader = {
        let seen = seen.clone();
        let mut master = std::fs::File::from(master);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = master.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        })
    };
    let has = |s: &[u8]| {
        let seen = seen.lock().unwrap();
        seen.windows(s.len()).any(|w| w == s)
    };
    wait_for(|| has(b"store: "));
    match stop {
        WatchStop::Signal(sig) => kill(Pid::from_raw(child.id() as i32), *sig).unwrap(),
        WatchStop::Keys(keys) => input.write_all(keys).unwrap(),
    }
    let status = child.wait().unwrap();
    // SAFETY: tcgetattr only writes into `t`; `slave` is an open pty.
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut t) }, 0);
    // The reader ends once every slave descriptor is closed.
    drop(slave);
    drop(input);
    reader.join().unwrap();
    let out = seen.lock().unwrap().clone();
    (status.code(), out, t.c_lflag)
}

/// Spec status-command "Status watch mode", scenarios "Ending with a key"
/// and "Ending with a signal": `status --watch` gives the terminal back
/// (line wrapping, cursor, main screen, cooked mode) when it is ended by
/// `q`, Ctrl-C, `kill` or a closed ssh session.
#[test]
fn status_watch_restores_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(&cfg, store_header(dir.path())).unwrap();
    for stop in [
        WatchStop::Signal(Signal::SIGTERM),
        WatchStop::Signal(Signal::SIGHUP),
        WatchStop::Signal(Signal::SIGINT),
        WatchStop::Keys(b"q"),
        WatchStop::Keys(b"\x03"),
    ] {
        let what = match &stop {
            WatchStop::Signal(sig) => sig.to_string(),
            WatchStop::Keys(k) => format!("{k:?}"),
        };
        let (code, out, lflag) = watch_on_a_pty(&cfg, &stop);
        assert_eq!(code, Some(0), "{what}");
        assert!(
            out.ends_with(b"\x1b[?7h\x1b[?25h\x1b[?1049l"),
            "{what}: {:?}",
            String::from_utf8_lossy(&out[out.len().saturating_sub(40)..])
        );
        for flag in [libc::ICANON, libc::ECHO, libc::ISIG] {
            assert_ne!(lflag & flag, 0, "{what}: lflag {lflag:#x} lacks {flag:#x}");
        }
    }
}

/// Spec status-command "Status history output", scenario "History with
/// watch refused".
#[test]
fn status_history_refuses_watch_and_json() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(&cfg, "").unwrap();
    for other in ["--watch", "--json"] {
        let (code, _, err) = status(&cfg, &["--history", other]);
        assert_eq!(code, 2, "{err}");
        assert!(err.contains("cannot be used with"), "{err}");
    }
}

/// Spec state-store "Backend state history", scenario "History kept across
/// a restart", and status-command "Status history output".
#[test]
fn history_kept_across_a_restart() {
    let (l, addr) = listener();
    let mut d = start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    let state = || r.service("api.svc.emfor").map(|s| s.backends[0].state);
    wait_for(|| state() == Some(HealthState::Healthy));
    drop(l);
    wait_for(|| state() == Some(HealthState::Unhealthy));
    let before: ServiceHistory =
        serde_json::from_str(&r.get("hist/api.svc.emfor").unwrap()).unwrap();
    let transitions: Vec<_> = before.backends[&addr]
        .iter()
        .map(|e| (e.from, e.to))
        .collect();
    assert_eq!(
        transitions,
        [
            (HealthState::Unknown, HealthState::Healthy),
            (HealthState::Healthy, HealthState::Unhealthy)
        ]
    );
    drop(r);

    // Restart on the same directory.
    d.signal(Signal::SIGTERM);
    assert!(d.child.wait().unwrap().success());
    let dir = std::mem::replace(&mut d.dir, tempfile::tempdir().unwrap());
    let services = std::fs::read_to_string(dir.path().join("emfor.toml"))
        .unwrap()
        .split_once("\n\n")
        .unwrap()
        .1
        .to_owned();
    let d = start_in(dir, &services);
    let r = Reader::open(&d.store());
    wait_for(|| r.get("meta/heartbeat").is_some_and(|h| h != "0"));
    let after: ServiceHistory =
        serde_json::from_str(&r.get("hist/api.svc.emfor").unwrap()).unwrap();
    assert_eq!(after.backends[&addr][..2], before.backends[&addr][..]);

    let (code, out, err) = status(&d.config(), &["--history"]);
    assert_eq!(code, 1, "backend down: {out}{err}");
    let lines: Vec<&str> = out.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == format!("  {addr}"))
        .unwrap_or_else(|| panic!("{out}"));
    // Newest first; the restarted daemon may already have added
    // `unknown -> unhealthy` above the earlier changes.
    let block: Vec<&str> = lines[at + 1..]
        .iter()
        .take_while(|l| l.starts_with("    "))
        .copied()
        .collect();
    let down = block
        .iter()
        .position(|l| l.contains(" healthy -> unhealthy  "));
    let up = block
        .iter()
        .position(|l| l.ends_with(" unknown -> healthy"));
    assert!(matches!((down, up), (Some(d), Some(u)) if d < u), "{out}");

    let (_, out, _) = status(&d.config(), &["--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v["history"]["api.svc.emfor"]["backends"][&addr].is_array());
    assert_eq!(v["flapping"], serde_json::json!([]));
}
