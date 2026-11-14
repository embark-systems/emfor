//! End-to-end tests of the DNS server: run the `emfor` binary and send real
//! DNS messages over UDP and TCP.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use hickory_proto::op::{Message, OpCode, Query, ResponseCode};
use hickory_proto::rr::{Name, RData, RecordType};
use nix::sys::signal::Signal;

mod common;
use common::{Daemon, free_port, http_backend, listener, sockets, wait_for};

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
    let s = sockets(|| UdpSocket::bind("127.0.0.1:0")).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    s.send_to(packet, ("127.0.0.1", port)).ok()?;
    let mut buf = [0u8; 4096];
    let (n, _) = s.recv_from(&mut buf).ok()?;
    Some(Message::from_vec(&buf[..n]).unwrap())
}

fn tcp(port: u16, packet: &[u8]) -> Option<Message> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = sockets(|| TcpStream::connect_timeout(&addr, Duration::from_millis(500))).ok()?;
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

#[test]
fn healthy_backend_over_udp_and_tcp() {
    let (_l, backend) = listener();
    let port = free_port();
    let d = Daemon::spawn(&format!(
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
    let _d = Daemon::spawn(&format!(
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
    let d = Daemon::spawn(&service("api.svc.emfor", &["192.0.2.1:9"], ""));
    let store = d.store();
    wait_for(|| std::fs::metadata(&store).is_ok_and(|m| m.len() > 0));
    // A DNS server would be reported before this line.
    d.wait_log("emfor started");
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    assert!(!err.contains("DNS server"), "{err}");
}

#[test]
fn address_in_use_exits_one() {
    let taken = sockets(|| UdpSocket::bind("127.0.0.1:0")).unwrap();
    let port = taken.local_addr().unwrap().port();
    let mut d = Daemon::spawn(&format!(
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
    let d = Daemon::spawn(&format!(
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
    let (_l, backend) = listener();
    let port = free_port();
    let _d = Daemon::spawn(&format!(
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
    let d = Daemon::spawn(&format!(
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
    let held = sockets(|| UdpSocket::bind("127.0.0.1:0")).unwrap();
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
    let d = Daemon::spawn(&format!(
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
    let d = Daemon::spawn(&service("api.svc.emfor", &["192.0.2.1:9"], ""));
    let store = d.store();
    wait_for(|| std::fs::metadata(&store).is_ok_and(|m| m.len() > 0));
    // Held for the whole test, as in
    // reload_rejects_listen_change_and_applies_balance_and_ttl: another
    // test's daemon must not answer on it.
    let held = sockets(|| UdpSocket::bind("127.0.0.1:0")).unwrap();
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
    let svc = service("api.svc.emfor", &[&fast, &slow], "balance = \"fastest\"\n")
        .replace("check = \"tcp\"", "check = \"http\"")
        .replace("timeout = \"100ms\"", "timeout = \"150ms\"");
    let d = Daemon::spawn(&format!("{}{svc}", dns_table(port)));
    let port_of = |a: &str| a.parse::<SocketAddr>().expect("a backend address").port();
    let want = vec![(port_of(&fast), 0), (port_of(&slow), 1)];
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
