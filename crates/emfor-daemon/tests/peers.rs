//! End-to-end tests of the peer group (spec peer-group): run several `emfor`
//! binaries on loopback ports that were free a moment ago and watch their
//! logs and `/metrics`.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use assert_cmd::cargo::cargo_bin;
use emfor_daemon::peers::{Kind, Message, encode};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
    metrics: SocketAddr,
    /// Everything the daemon wrote to stderr so far.
    log: Arc<Mutex<String>>,
    reader: Option<JoinHandle<()>>,
}

impl Daemon {
    fn write_config(&self, body: &str) {
        std::fs::write(self.dir.path().join("emfor.toml"), body)
            .expect("the test config can be written");
    }
    fn signal(&self, s: Signal) {
        kill(Pid::from_raw(self.child.id() as i32), s).expect("the daemon can be signalled");
    }
    fn log(&self) -> String {
        self.log.lock().expect("log lock").clone()
    }
    fn wait_log(&self, text: &str) {
        wait_for(text, Duration::from_secs(10), || self.log().contains(text));
    }
    fn wait(&mut self) -> (i32, String) {
        let status = self.child.wait().expect("the daemon can be waited for");
        if let Some(r) = self.reader.take() {
            r.join().expect("the log reader ends");
        }
        (status.code().unwrap_or(-1), self.log())
    }
    fn scrape(&self) -> String {
        let mut s = TcpStream::connect(self.metrics).expect("the metrics endpoint answers");
        s.set_read_timeout(Some(Duration::from_secs(10)))
            .expect("a read timeout can be set");
        s.write_all(b"GET /metrics HTTP/1.1\r\nHost: test\r\n\r\n")
            .expect("the request can be sent");
        let mut out = String::new();
        s.read_to_string(&mut out)
            .expect("the response can be read");
        out
    }
    fn has_line(&self, line: &str) -> bool {
        self.scrape().lines().any(|l| l == line)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn wait_for(what: &str, limit: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{what}: not within {limit:?}");
}

fn free_tcp() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a loopback TCP port is free")
}

fn free_udp() -> SocketAddr {
    UdpSocket::bind("127.0.0.1:0")
        .and_then(|s| s.local_addr())
        .expect("a loopback UDP port is free")
}

/// The secret every test member uses, as written to its file.
const SECRET: &str = "0123456789abcdef0123456789abcdef\n";

/// A secret file with mode 0600 in `dir`.
fn secret(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("peers.key");
    std::fs::write(&path, SECRET).expect("the secret can be written");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .expect("the secret's mode can be set");
    path
}

/// One member's config: `site`, its own store and metrics, `[peers]`, and
/// one TCP service `api.svc.emfor` on `backend`.
fn config(
    dir: &Path,
    metrics: SocketAddr,
    listen: SocketAddr,
    members: &[SocketAddr],
    backend: SocketAddr,
    service: &str,
) -> String {
    let members: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
    format!(
        "site = \"dc1\"\n\n[store]\npath = \"{}\"\nmap_size = \"4MiB\"\n\n[metrics]\nlisten = [\"{metrics}\"]\n\n[peers]\nlisten = \"{listen}\"\nmembers = [{}]\nsecret_file = \"{}\"\n\n[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"{backend}\"]\n{service}",
        dir.join("state.mdb").display(),
        members.join(", "),
        secret(dir).display(),
    )
}

fn spawn(config_for: impl FnOnce(&Path, SocketAddr) -> String) -> Daemon {
    let dir = tempfile::tempdir().expect("a temporary directory can be created");
    let metrics = free_tcp();
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(&cfg, config_for(dir.path(), metrics)).expect("the config can be written");
    let mut child = Command::new(cargo_bin("emfor"))
        .arg("server")
        .arg("--config")
        .arg(&cfg)
        .env_remove("RUST_LOG")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the emfor binary starts");
    let mut pipe = child.stderr.take().expect("stderr is piped");
    let log = Arc::new(Mutex::new(String::new()));
    let sink = log.clone();
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = pipe.read(&mut buf) {
            sink.lock()
                .expect("log lock")
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    Daemon {
        child,
        dir,
        metrics,
        log,
        reader: Some(reader),
    }
}

const UNHEALTHY: &str = "changed from healthy to unhealthy";
const HEALTHY: &str = "changed from unknown to healthy";

/// Two members checking one TCP backend: A every second, B every 30
/// seconds (the digest leaves `interval` out, so they still share). `b_extra`
/// is added to B's service.
fn pair(backend: SocketAddr, b_extra: &str) -> (Daemon, Daemon, SocketAddr, SocketAddr) {
    let (a, b) = (free_udp(), free_udp());
    let da = spawn(|dir, m| {
        config(
            dir,
            m,
            a,
            &[a, b],
            backend,
            "interval = \"1s\"\ntimeout = \"500ms\"\nfall = 3\n",
        )
    });
    let db = spawn(|dir, m| {
        config(
            dir,
            m,
            b,
            &[a, b],
            backend,
            &format!("interval = \"30s\"\ntimeout = \"500ms\"\nfall = 3\n{b_extra}"),
        )
    });
    for d in [&da, &db] {
        d.wait_log(HEALTHY);
    }
    wait_for("both members live", Duration::from_secs(5), || {
        da.has_line(&format!("emfor_peer_up{{peer=\"{b}\"}} 1"))
            && db.has_line(&format!("emfor_peer_up{{peer=\"{a}\"}} 1"))
    });
    (da, db, a, b)
}

/// Spec peer-group "Peer sees the failure first": B, which would need a
/// minute on its own, follows A within 2 seconds.
#[test]
fn peer_sees_the_failure_first() {
    let held = TcpListener::bind("127.0.0.1:0").expect("the backend can listen");
    let backend = held.local_addr().expect("the backend has an address");
    let (da, db, a, _b) = pair(backend, "");
    drop(held);
    wait_for("A marks it unhealthy", Duration::from_secs(10), || {
        da.log().contains(UNHEALTHY)
    });
    let first = Instant::now();
    wait_for("B follows within 2s", Duration::from_secs(2), || {
        db.log().contains(UNHEALTHY)
    });
    assert!(first.elapsed() <= Duration::from_secs(2));
    let log = db.log();
    assert!(
        log.contains(&format!(
            "peer {a} reports backend {backend} of api.svc.emfor down"
        )) && log.contains("checking now"),
        "{log}"
    );
    assert!(da.has_line("emfor_peer_notifications_total{direction=\"sent\"} 1"));
    assert!(db.has_line("emfor_peer_notifications_total{direction=\"received\"} 1"));
}

/// Spec peer-group "Digest differs": B's service has another timeout, so
/// it ignores A's report and keeps its own schedule.
#[test]
fn digest_differs() {
    let held = TcpListener::bind("127.0.0.1:0").expect("the backend can listen");
    let backend = held.local_addr().expect("the backend has an address");
    let (a, b) = (free_udp(), free_udp());
    let da = spawn(|dir, m| {
        config(
            dir,
            m,
            a,
            &[a, b],
            backend,
            "interval = \"1s\"\ntimeout = \"500ms\"\n",
        )
    });
    let db = spawn(|dir, m| {
        config(
            dir,
            m,
            b,
            &[a, b],
            backend,
            "interval = \"30s\"\ntimeout = \"400ms\"\n",
        )
    });
    for d in [&da, &db] {
        d.wait_log(HEALTHY);
    }
    wait_for("A live at B", Duration::from_secs(5), || {
        db.has_line(&format!("emfor_peer_up{{peer=\"{a}\"}} 1"))
    });
    drop(held);
    wait_for("A marks it unhealthy", Duration::from_secs(10), || {
        da.log().contains(UNHEALTHY)
    });
    wait_for("B accepts the report", Duration::from_secs(2), || {
        db.has_line("emfor_peer_notifications_total{direction=\"received\"} 1")
    });
    std::thread::sleep(Duration::from_secs(2));
    let log = db.log();
    assert!(
        !log.contains(UNHEALTHY) && !log.contains("checking now"),
        "{log}"
    );
}

/// Spec peer-group "Member stops" and "Member added on reload".
#[test]
fn member_stops_and_member_added_on_reload() {
    let held = TcpListener::bind("127.0.0.1:0").expect("the backend can listen");
    let backend = held.local_addr().expect("the backend has an address");
    let (da, mut db, a, b) = pair(backend, "");
    db.signal(Signal::SIGTERM);
    db.wait();
    let stopped = Instant::now();
    wait_for("B not live at A", Duration::from_secs(5), || {
        da.has_line(&format!("emfor_peer_up{{peer=\"{b}\"}} 0"))
    });
    assert!(stopped.elapsed() <= Duration::from_secs(4) + Duration::from_millis(500));
    da.wait_log(&format!("peer {b} is not live"));

    let c = UdpSocket::bind("127.0.0.1:0").expect("C can bind");
    c.set_read_timeout(Some(Duration::from_secs(3)))
        .expect("a read timeout can be set");
    let c_addr = c.local_addr().expect("C has an address");
    da.write_config(&config(
        da.dir.path(),
        da.metrics,
        a,
        &[a, b, c_addr],
        backend,
        "interval = \"1s\"\ntimeout = \"500ms\"\n",
    ));
    da.signal(Signal::SIGHUP);
    da.wait_log("config reloaded");
    assert!(da.has_line(&format!("emfor_peer_up{{peer=\"{c_addr}\"}} 0")));
    let mut buf = [0u8; 1500];
    let (n, from) = c
        .recv_from(&mut buf)
        .expect("C receives a heartbeat within 3s");
    assert_eq!(from, a);
    assert_eq!(&buf[..4], b"EMP1");
    assert!(String::from_utf8_lossy(&buf[36..n]).contains("\"kind\":\"heartbeat\""));
}

/// Service-config "Peers listen change requires restart".
#[test]
fn reload_rejects_peers_listen_change() {
    let backend = free_tcp();
    let (a, b) = (free_udp(), free_udp());
    let d = spawn(|dir, m| config(dir, m, a, &[a, b], backend, ""));
    d.wait_log("emfor started");
    let other = free_udp();
    d.write_config(&config(
        d.dir.path(),
        d.metrics,
        other,
        &[other, b],
        backend,
        "",
    ));
    d.signal(Signal::SIGHUP);
    d.wait_log("reload rejected");
    let log = d.log();
    assert!(
        log.contains("peers.listen") && log.contains("peers listen settings require restart"),
        "{log}"
    );
    // Still receiving on the old address: a signed heartbeat from B makes B
    // live there.
    let sb = UdpSocket::bind(b).expect("B's address is free");
    let heartbeat = encode(
        &Message {
            from: b,
            site: "dc1".into(),
            boot: 0x0123_4567_89ab_cdef,
            seq: 1,
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .expect("the clock is after 1970"),
            kind: Kind::Heartbeat,
        },
        SECRET.trim_end().as_bytes(),
    );
    sb.send_to(&heartbeat, a).expect("B can send to A");
    wait_for("B live at A", Duration::from_secs(3), || {
        d.has_line(&format!("emfor_peer_up{{peer=\"{b}\"}} 1"))
    });
}

/// Binding `[peers] listen` at start fails: exit 1 before starting.
#[test]
fn occupied_peer_port_exits_1() {
    let held = UdpSocket::bind("127.0.0.1:0").expect("a UDP port can be held");
    let a = held.local_addr().expect("the held port has an address");
    let b = free_udp();
    let mut d = spawn(|dir, m| config(dir, m, a, &[a, b], free_tcp(), ""));
    let (code, log) = d.wait();
    assert_eq!(code, 1, "{log}");
    assert!(
        log.contains(&format!("cannot bind peer address {a}")),
        "{log}"
    );
    assert!(!log.contains("emfor started"), "{log}");
}
