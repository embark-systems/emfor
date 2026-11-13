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
    spawn_env(config_for, &[])
}

/// [`spawn`] with extra environment variables for the daemon process.
fn spawn_env(
    config_for: impl FnOnce(&Path, SocketAddr) -> String,
    env: &[(&str, &Path)],
) -> Daemon {
    let dir = tempfile::tempdir().expect("a temporary directory can be created");
    let metrics = free_tcp();
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(&cfg, config_for(dir.path(), metrics)).expect("the config can be written");
    let mut child = Command::new(cargo_bin("emfor"))
        .arg("server")
        .arg("--config")
        .arg(&cfg)
        .env_remove("RUST_LOG")
        .envs(env.iter().copied())
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
    // Two members: no majority, B's reason is its own.
    let line = log
        .lines()
        .find(|l| l.contains(UNHEALTHY))
        .expect("B logged the change");
    assert!(
        !line.contains("(peers ") && !line.contains("(local: "),
        "{line}"
    );
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

/// One member of a group of three with an `exec` service whose result the
/// test controls per daemon: the command is the same everywhere (so is the
/// digest), and it fails while the file named by the daemon's own
/// `EMFOR_TEST_FLAG` exists.
fn exec_config(
    dir: &Path,
    metrics: SocketAddr,
    listen: SocketAddr,
    members: &[SocketAddr],
) -> String {
    let members: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
    format!(
        "site = \"dc1\"\n\n[store]\npath = \"{}\"\nmap_size = \"4MiB\"\n\n[metrics]\nlisten = [\"{metrics}\"]\n\n[peers]\nlisten = \"{listen}\"\nmembers = [{}]\nsecret_file = \"{}\"\n\n[services.\"api.svc.emfor\"]\ncheck = \"exec\"\nbackends = [\"127.0.0.1:9\"]\ninterval = \"1s\"\ntimeout = \"900ms\"\nrise = 1\nfall = 3\njitter = \"0%\"\n[services.\"api.svc.emfor\".check_options]\ncommand = [\"/bin/sh\", \"-c\", 'if [ -n \"$EMFOR_TEST_COUNT\" ]; then echo x >> \"$EMFOR_TEST_COUNT\"; fi; if [ -e \"$EMFOR_TEST_FLAG\" ]; then echo refused; exit 1; fi']\n",
        dir.join("state.mdb").display(),
        members.join(", "),
        secret(dir).display(),
    )
}

/// `emfor status` for daemon `d`: (stdout of the text output, `--json`).
fn status(d: &Daemon) -> (String, serde_json::Value) {
    let cfg = d.dir.path().join("emfor.toml");
    let run = |json: bool| {
        let mut c = Command::new(cargo_bin("emfor"));
        c.arg("status").arg("--config").arg(&cfg);
        if json {
            c.arg("--json");
        }
        let out = c.output().expect("emfor status runs");
        String::from_utf8(out.stdout).expect("status prints UTF-8")
    };
    let json = serde_json::from_str(&run(true)).expect("status --json prints JSON");
    (run(false), json)
}

/// Sum of a counter over all its series whose labels contain `label`.
fn counter(body: &str, name: &str, label: &str) -> u64 {
    body.lines()
        .filter(|l| l.starts_with(&format!("{name}{{")) && l.contains(label))
        .filter_map(|l| l.rsplit(' ').next()?.parse::<u64>().ok())
        .sum()
}

/// Spec peer-group "Two of three assigned checkers see the backend down"
/// and "Only one daemon sees the backend down".
#[test]
fn majority_of_three() {
    let flags = tempfile::tempdir().expect("a temporary directory can be created");
    let flag = |i: usize| flags.path().join(format!("down-{i}"));
    let members = [free_udp(), free_udp(), free_udp()];
    let daemons: Vec<Daemon> = (0..3)
        .map(|i| {
            let f = flag(i);
            spawn_env(
                |dir, m| exec_config(dir, m, members[i], &members),
                &[("EMFOR_TEST_FLAG", &f)],
            )
        })
        .collect();
    for d in &daemons {
        d.wait_log(HEALTHY);
    }
    wait_for(
        "every member live everywhere",
        Duration::from_secs(10),
        || {
            daemons.iter().all(|d| {
                let body = d.scrape();
                counter(&body, "emfor_peer_up", "") == 2
            })
        },
    );
    let [a, b, c] = [&daemons[0], &daemons[1], &daemons[2]];

    // A and B see the backend down, C's own checks pass.
    for i in [0, 1] {
        std::fs::write(flag(i), "").expect("the flag can be written");
    }
    // C's own checks keep passing: only the vote can make it unhealthy
    // (about `fall` = 3 checks of 1s; wait_log allows 10s for slow hosts).
    c.wait_log("changed from healthy to unhealthy (peers 2/3: refused)");
    c.wait_log("is down at 2/3 peers but up here");
    wait_for("C counts the disagreement", Duration::from_secs(10), || {
        c.has_line("emfor_peer_disagreements 1")
    });
    wait_for("meta/peers lists it", Duration::from_secs(10), || {
        status(c).1["peers"]["disagreements"]
            == serde_json::json!([{"service":"api.svc.emfor","backend":"127.0.0.1:9","down":2,"fresh":3}])
    });
    let (text, json) = status(c);
    assert!(text.contains("peers: 2/2 live"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.contains("127.0.0.1:9") && l.contains("local-up")),
        "{text}"
    );
    assert_eq!(
        json["peers"]["listen"],
        serde_json::json!(members[2].to_string())
    );
    // Each own up or down check of an assigned checker goes to both others.
    // Compared from now on: a member that briefly stopped being live on a
    // loaded host left checks without assigned checkers, and so without
    // results, earlier.
    let sent_and_checks = || {
        let body = c.scrape();
        let checks = counter(&body, "emfor_backend_checks_total", "result=\"up\"")
            + counter(&body, "emfor_backend_checks_total", "result=\"down\"");
        let sent = counter(&body, "emfor_peer_results_total", "direction=\"sent\"");
        (sent, checks)
    };
    let (sent0, checks0) = sent_and_checks();
    wait_for(
        "one round of results per check",
        Duration::from_secs(10),
        || {
            let (sent, checks) = sent_and_checks();
            checks >= checks0 + 2 && sent - sent0 == 2 * (checks - checks0)
        },
    );

    // Everyone sees it up again.
    for i in [0, 1] {
        std::fs::remove_file(flag(i)).expect("the flag can be removed");
    }
    wait_for("all healthy again", Duration::from_secs(10), || {
        daemons
            .iter()
            .all(|d| d.log().contains("changed from unhealthy to healthy"))
    });
    wait_for("C clears the disagreement", Duration::from_secs(10), || {
        c.has_line("emfor_peer_disagreements 0")
    });

    // Only A sees it down: A alone marks it, with a local reason.
    let before: Vec<usize> = daemons
        .iter()
        .map(|d| d.log().matches(UNHEALTHY).count())
        .collect();
    std::fs::write(flag(0), "").expect("the flag can be written");
    a.wait_log("changed from healthy to unhealthy (local: refused)");
    std::thread::sleep(Duration::from_secs(4));
    for (d, n) in [(b, before[1]), (c, before[2])] {
        assert_eq!(d.log().matches(UNHEALTHY).count(), n, "{}", d.log());
    }
}

/// Own checks a daemon of [`split_checks`] has made so far.
fn own_checks(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |s| s.lines().count())
}

/// Spec peer-group "Split checks": in a group of four, the one daemon that
/// is not an assigned checker of the backend checks it about every fifth
/// time, still notices a failure only it sees, and checks every time once
/// a member is gone and it becomes an assigned checker.
#[test]
fn split_checks() {
    let files = tempfile::tempdir().expect("a temporary directory can be created");
    let flag = |i: usize| files.path().join(format!("down-{i}"));
    let count = |i: usize| files.path().join(format!("checks-{i}"));
    let members = [free_udp(), free_udp(), free_udp(), free_udp()];
    let mut daemons: Vec<Daemon> = (0..4)
        .map(|i| {
            let (f, c) = (flag(i), count(i));
            spawn_env(
                |dir, m| exec_config(dir, m, members[i], &members),
                &[("EMFOR_TEST_FLAG", &f), ("EMFOR_TEST_COUNT", &c)],
            )
        })
        .collect();
    let live: std::collections::BTreeSet<SocketAddr> = members[1..].iter().copied().collect();
    let ranked = emfor_daemon::quorum::assigned("api.svc.emfor", "127.0.0.1:9", members[0], &live);
    let spare = (0..4)
        .find(|i| !ranked.contains(&members[*i]))
        .expect("one member is not an assigned checker");
    let checkers: Vec<usize> = (0..4).filter(|i| *i != spare).collect();
    for d in &daemons {
        d.wait_log(HEALTHY);
    }
    wait_for(
        "every member live everywhere",
        Duration::from_secs(10),
        || {
            daemons
                .iter()
                .all(|d| counter(&d.scrape(), "emfor_peer_up", "") == 3)
        },
    );

    // About 8 check times: the assigned checkers check every time, the
    // spare one about every fifth.
    std::thread::sleep(Duration::from_secs(2));
    let before: Vec<usize> = (0..4).map(|i| own_checks(&count(i))).collect();
    std::thread::sleep(Duration::from_secs(8));
    let made: Vec<usize> = (0..4).map(|i| own_checks(&count(i)) - before[i]).collect();
    eprintln!("own checks in 8s: {made:?} (spare {spare})");
    for i in &checkers {
        assert!(made[*i] >= 6, "assigned checker {i}: {made:?}");
        assert!(2 * made[spare] < made[*i], "spare {spare}: {made:?}");
    }
    let skipped = daemons[spare]
        .scrape()
        .lines()
        .find_map(|l| {
            l.strip_prefix("emfor_peer_checks_skipped_total ")?
                .parse::<u64>()
                .ok()
        })
        .expect("the spare one has the skipped-check counter");
    assert!(skipped > 0, "the spare one counts its skipped checks");
    assert!(daemons[spare].log().contains(HEALTHY));

    // A failure only the spare one sees: found within 5 check times plus
    // `fall` = 3, with a local reason; the others stay healthy.
    let unhealthy: Vec<usize> = daemons
        .iter()
        .map(|d| d.log().matches(UNHEALTHY).count())
        .collect();
    std::fs::write(flag(spare), "").expect("the flag can be written");
    let text = "changed from healthy to unhealthy (local: refused)";
    wait_for(text, Duration::from_secs(15), || {
        daemons[spare].log().contains(text)
    });
    for i in &checkers {
        assert_eq!(
            daemons[*i].log().matches(UNHEALTHY).count(),
            unhealthy[*i],
            "{}",
            daemons[*i].log()
        );
    }
    std::fs::remove_file(flag(spare)).expect("the flag can be removed");
    wait_for(
        "the spare one is healthy again",
        Duration::from_secs(10),
        || {
            daemons[spare]
                .log()
                .contains("changed from unhealthy to healthy")
        },
    );

    // One assigned checker stops: the remaining three are all assigned, so
    // the spare one checks every time.
    drop(daemons.remove(checkers[0]));
    let spare_d = &daemons[if checkers[0] < spare {
        spare - 1
    } else {
        spare
    }];
    wait_for(
        "the stopped member is not live",
        Duration::from_secs(10),
        || counter(&spare_d.scrape(), "emfor_peer_up", "") == 2,
    );
    std::thread::sleep(Duration::from_secs(1));
    let before = own_checks(&count(spare));
    std::thread::sleep(Duration::from_secs(5));
    let made = own_checks(&count(spare)) - before;
    assert!(made >= 4, "the spare one checks every time now: {made}");
}
