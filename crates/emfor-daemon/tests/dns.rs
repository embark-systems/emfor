//! End-to-end tests of the DNS server: run the `emfor` binary and send real
//! DNS messages over UDP and TCP.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use hickory_proto::op::{Message, OpCode, Query, ResponseCode};
use hickory_proto::rr::{Name, RData, RecordType};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
    /// Everything the daemon wrote to stderr so far, read by `reader`.
    log: Arc<Mutex<String>>,
    reader: Option<JoinHandle<()>>,
}

impl Daemon {
    fn config(&self) -> std::path::PathBuf {
        self.dir.path().join("emfor.toml")
    }
    fn write_config(&self, body: &str) {
        std::fs::write(self.config(), format!("{}{body}", header(self.dir.path()))).unwrap();
    }
    fn signal(&self, s: Signal) {
        kill(Pid::from_raw(self.child.id() as i32), s).unwrap();
    }
    /// Wait until the daemon has logged `text`.
    fn wait_log(&self, text: &str) {
        wait_for(|| self.log.lock().unwrap().contains(text));
    }
    /// Wait for the daemon to exit; returns (exit code, all of stderr).
    fn wait(&mut self) -> (i32, String) {
        let status = self.child.wait().unwrap();
        if let Some(r) = self.reader.take() {
            r.join().unwrap();
        }
        (
            status.code().unwrap_or(-1),
            self.log.lock().unwrap().clone(),
        )
    }
    fn stop(mut self) -> (i32, String) {
        self.signal(Signal::SIGTERM);
        self.wait()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn header(dir: &std::path::Path) -> String {
    format!(
        "[store]\npath = \"{}\"\nmap_size = \"4MiB\"\n\n",
        dir.join("state.mdb").display()
    )
}

/// A loopback port free for both UDP and TCP.
fn free_port() -> u16 {
    loop {
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = udp.local_addr().unwrap().port();
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}

fn dns_table(port: u16) -> String {
    format!("[dns]\nlisten = [\"127.0.0.1:{port}\"]\n\n")
}

fn service(name: &str, backends: &[&str], extra: &str) -> String {
    let list: Vec<String> = backends.iter().map(|b| format!("\"{b}\"")).collect();
    format!(
        "[services.\"{name}\"]\ncheck = \"tcp\"\nbackends = [{}]\ninterval = \"200ms\"\ntimeout = \"100ms\"\nrise = 1\nfall = 1\njitter = \"0%\"\n{extra}\n",
        list.join(", ")
    )
}

fn spawn(body: &str) -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(&cfg, format!("{}{body}", header(dir.path()))).unwrap();
    let mut child = Command::new(cargo_bin("emfor"))
        .arg("server")
        .arg("--config")
        .arg(&cfg)
        .env_remove("RUST_LOG")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stderr.take().unwrap();
    let log = Arc::new(Mutex::new(String::new()));
    let sink = log.clone();
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = pipe.read(&mut buf) {
            sink.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    Daemon {
        child,
        dir,
        log,
        reader: Some(reader),
    }
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

fn request(name: &str, t: RecordType) -> Vec<u8> {
    let mut m = Message::query();
    m.metadata.id = rand_id();
    m.metadata.recursion_desired = true;
    m.add_query(Query::query(Name::from_ascii(name).unwrap(), t));
    m.to_vec().unwrap()
}

fn rand_id() -> u16 {
    (Instant::now().elapsed().subsec_nanos() & 0xffff) as u16 ^ std::process::id() as u16
}

fn udp(port: u16, packet: &[u8]) -> Option<Message> {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    s.send_to(packet, ("127.0.0.1", port)).ok()?;
    let mut buf = [0u8; 4096];
    let (n, _) = s.recv_from(&mut buf).ok()?;
    Some(Message::from_vec(&buf[..n]).unwrap())
}

fn tcp(port: u16, packet: &[u8]) -> Option<Message> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let mut framed = (packet.len() as u16).to_be_bytes().to_vec();
    framed.extend_from_slice(packet);
    s.write_all(&framed).ok()?;
    let mut len = [0u8; 2];
    s.read_exact(&mut len).ok()?;
    let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
    s.read_exact(&mut buf).ok()?;
    Some(Message::from_vec(&buf).unwrap())
}

/// UDP query, retried so a busy test machine does not drop an answer.
fn ask(port: u16, name: &str, t: RecordType) -> Option<Message> {
    (0..3).find_map(|_| udp(port, &request(name, t)))
}

/// First answer's address and TTL.
fn a_record(m: &Message) -> Option<(String, u32)> {
    m.answers.first().map(|r| (r.data.to_string(), r.ttl))
}

fn backend_listener() -> (TcpListener, String) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap().to_string();
    (l, a)
}

#[test]
fn healthy_backend_over_udp_and_tcp() {
    let (_l, backend) = backend_listener();
    let port = free_port();
    let d = spawn(&format!(
        "{}{}",
        dns_table(port),
        service("api.svc.emfor", &[&backend], "")
    ));
    // Healthy once the answer is the backend with AA set.
    wait_for(|| {
        ask(port, "api.svc.emfor", RecordType::A)
            .is_some_and(|m| m.metadata.authoritative && a_record(&m).is_some())
    });
    let m = ask(port, "api.svc.emfor", RecordType::A).unwrap();
    assert_eq!(m.metadata.response_code, ResponseCode::NoError);
    assert_eq!(a_record(&m), Some(("127.0.0.1".into(), 1)));
    let m = tcp(port, &request("api.svc.emfor", RecordType::A)).expect("TCP answer");
    assert_eq!(a_record(&m), Some(("127.0.0.1".into(), 1)));
    assert!(m.metadata.authoritative);
    // Names that are not services.
    let m = ask(port, "nope.svc.emfor", RecordType::A).unwrap();
    assert_eq!(m.metadata.response_code, ResponseCode::NXDomain);
    assert!(matches!(m.authorities[0].data, RData::SOA(_)));
    let m = ask(port, "example.com", RecordType::A).unwrap();
    assert_eq!(m.metadata.response_code, ResponseCode::Refused);
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    // Queries are not logged one by one at the default level.
    assert!(!err.contains("hickory_server"), "{err}");
}

#[test]
fn update_opcode_is_notimp() {
    let port = free_port();
    let _d = spawn(&format!(
        "{}{}",
        dns_table(port),
        service("api.svc.emfor", &["192.0.2.1:9"], "")
    ));
    let mut m = Message::query();
    m.metadata.op_code = OpCode::Update;
    m.add_query(Query::query(
        Name::from_ascii("svc.emfor").unwrap(),
        RecordType::SOA,
    ));
    let packet = m.to_vec().unwrap();
    wait_for(|| udp(port, &packet).is_some());
    assert_eq!(
        udp(port, &packet).unwrap().metadata.response_code,
        ResponseCode::NotImp
    );
}

#[test]
fn no_dns_table_opens_no_dns_socket() {
    let d = spawn(&service("api.svc.emfor", &["192.0.2.1:9"], ""));
    let store = d.dir.path().join("state.mdb");
    wait_for(|| std::fs::metadata(&store).is_ok_and(|m| m.len() > 0));
    // A DNS server would be reported before this line.
    d.wait_log("emfor started");
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    assert!(!err.contains("DNS server"), "{err}");
}

#[test]
fn address_in_use_exits_one() {
    let taken = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let mut d = spawn(&format!(
        "{}{}",
        dns_table(port),
        service("api.svc.emfor", &["192.0.2.1:9"], "")
    ));
    let (code, err) = d.wait();
    assert_eq!(code, 1);
    assert!(err.contains(&format!("127.0.0.1:{port}")), "{err}");
}

#[test]
fn service_outside_zone_is_warned_and_refused() {
    let port = free_port();
    let d = spawn(&format!(
        "{}{}",
        dns_table(port),
        service("db.internal", &["192.0.2.1:9"], "")
    ));
    wait_for(|| ask(port, "db.internal", RecordType::A).is_some());
    assert_eq!(
        ask(port, "db.internal", RecordType::A)
            .unwrap()
            .metadata
            .response_code,
        ResponseCode::Refused
    );
    let (_, err) = d.stop();
    assert!(
        err.lines()
            .any(|l| l.contains("WARN") && l.contains("db.internal") && l.contains("svc.emfor")),
        "{err}"
    );
}

#[test]
fn health_change_visible_in_next_answer() {
    // Failover: 192.0.2.1 never answers; until the loopback backend is
    // healthy both are candidates (all down, open) and the first is chosen.
    let (_l, backend) = backend_listener();
    let port = free_port();
    let _d = spawn(&format!(
        "{}{}",
        dns_table(port),
        service(
            "api.svc.emfor",
            &["192.0.2.1:9", &backend],
            "balance = \"failover\"\n"
        )
    ));
    wait_for(|| {
        ask(port, "api.svc.emfor", RecordType::A)
            .and_then(|m| a_record(&m))
            .is_some_and(|(ip, _)| ip == "127.0.0.1")
    });
}

#[test]
fn reload_rejects_listen_change_and_applies_balance_and_ttl() {
    // Both backends never answer: with on_all_down open, both are candidates.
    let port = free_port();
    let backends = ["192.0.2.1:9", "192.0.2.2:9"];
    let d = spawn(&format!(
        "{}{}",
        dns_table(port),
        service("api.svc.emfor", &backends, "")
    ));
    wait_for(|| ask(port, "api.svc.emfor", RecordType::A).is_some());
    let picks: Vec<String> = (0..4)
        .map(|_| {
            a_record(&ask(port, "api.svc.emfor", RecordType::A).unwrap())
                .unwrap()
                .0
        })
        .collect();
    assert_ne!(picks[0], picks[1], "round-robin alternates: {picks:?}");
    assert_eq!(picks[0], picks[2]);

    // Listen change: rejected, still answering on the old port. Hold the
    // new port for the rest of the test: tests run in parallel and also
    // serve api.svc.emfor, so another daemon could otherwise bind it and
    // answer there. Holding it, our own silent socket gets the question.
    let held = UdpSocket::bind("127.0.0.1:0").unwrap();
    let other = held.local_addr().unwrap().port();
    d.write_config(&format!(
        "{}{}",
        dns_table(other),
        service("api.svc.emfor", &backends, "balance = \"failover\"\n")
    ));
    d.signal(Signal::SIGHUP);
    d.wait_log("reload rejected");
    assert!(ask(other, "api.svc.emfor", RecordType::A).is_none());
    let picks: Vec<String> = (0..2)
        .map(|_| {
            a_record(&ask(port, "api.svc.emfor", RecordType::A).unwrap())
                .unwrap()
                .0
        })
        .collect();
    assert_ne!(picks[0], picks[1], "previous config (round-robin) kept");

    // Balance and ttl change: applied.
    d.write_config(&format!(
        "[dns]\nlisten = [\"127.0.0.1:{port}\"]\nttl = \"7s\"\n\n{}",
        service("api.svc.emfor", &backends, "balance = \"failover\"\n")
    ));
    d.signal(Signal::SIGHUP);
    wait_for(|| {
        a_record(&ask(port, "api.svc.emfor", RecordType::A).unwrap())
            .is_some_and(|(_, ttl)| ttl == 7)
    });
    for _ in 0..3 {
        assert_eq!(
            a_record(&ask(port, "api.svc.emfor", RecordType::A).unwrap()),
            Some(("192.0.2.1".into(), 7))
        );
    }
    let (_, err) = d.stop();
    assert!(err.contains("dns listen settings require restart"), "{err}");
}

#[test]
fn reload_removing_dns_table_is_reported_with_other_errors() {
    let port = free_port();
    let d = spawn(&format!(
        "{}{}",
        dns_table(port),
        service("api.svc.emfor", &["192.0.2.1:9"], "")
    ));
    wait_for(|| ask(port, "api.svc.emfor", RecordType::A).is_some());
    // No [dns] table, and a timeout above the interval.
    d.write_config(
        &service("api.svc.emfor", &["192.0.2.1:9"], "")
            .replace("timeout = \"100ms\"", "timeout = \"5s\""),
    );
    d.signal(Signal::SIGHUP);
    d.wait_log("reload rejected");
    assert!(
        ask(port, "api.svc.emfor", RecordType::A).is_some(),
        "DNS keeps answering"
    );
    let (_, err) = d.stop();
    assert!(err.contains("dns listen settings require restart"), "{err}");
    assert!(err.contains("timeout must not exceed interval"), "{err}");
}

#[test]
fn reload_adding_dns_table_requires_restart() {
    let d = spawn(&service("api.svc.emfor", &["192.0.2.1:9"], ""));
    let store = d.dir.path().join("state.mdb");
    wait_for(|| std::fs::metadata(&store).is_ok_and(|m| m.len() > 0));
    // Held for the whole test, as in
    // reload_rejects_listen_change_and_applies_balance_and_ttl: another
    // test's daemon must not answer on it.
    let held = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    d.write_config(&format!(
        "{}{}",
        dns_table(port),
        service("api.svc.emfor", &["192.0.2.1:9"], "")
    ));
    d.signal(Signal::SIGHUP);
    d.wait_log("reload rejected");
    assert!(
        ask(port, "api.svc.emfor", RecordType::A).is_none(),
        "no DNS socket opened"
    );
    let (_, err) = d.stop();
    assert!(err.contains("dns listen settings require restart"), "{err}");
}

/// HTTP backend answering every request with 200 after `delay`.
fn http_backend(delay: Duration) -> u16 {
    let (l, _) = backend_listener();
    let port = l.local_addr().unwrap().port();
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
    port
}

/// (port, priority) of every SRV answer.
fn srv_ports(m: &Message) -> Vec<(u16, u16)> {
    m.answers
        .iter()
        .filter_map(|r| match &r.data {
            RData::SRV(s) => Some((s.port, s.priority)),
            _ => None,
        })
        .collect()
}

#[test]
fn fastest_srv_gives_slow_backend_priority_1() {
    let fast = http_backend(Duration::ZERO);
    let slow = http_backend(Duration::from_millis(30));
    let port = free_port();
    let svc = service(
        "api.svc.emfor",
        &[&format!("127.0.0.1:{fast}"), &format!("127.0.0.1:{slow}")],
        "balance = \"fastest\"\n",
    )
    .replace("check = \"tcp\"", "check = \"http\"")
    .replace("timeout = \"100ms\"", "timeout = \"150ms\"");
    let d = spawn(&format!("{}{svc}", dns_table(port)));
    let want = vec![(fast, 0), (slow, 1)];
    wait_for(|| ask(port, "api.svc.emfor", RecordType::SRV).is_some_and(|m| srv_ports(&m) == want));
    // Every later answer keeps the order, whatever the rotation.
    for _ in 0..5 {
        let m = ask(port, "api.svc.emfor", RecordType::SRV).unwrap();
        assert_eq!(srv_ports(&m), want);
        std::thread::sleep(Duration::from_millis(100));
    }
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}
