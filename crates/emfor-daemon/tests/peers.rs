//! End-to-end tests of the peer group (spec peer-group): run several `emfor`
//! binaries on loopback ports that were free a moment ago and watch their
//! logs and `/metrics`.

use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use assert_cmd::cargo::cargo_bin;
use emfor_daemon::peers::{Kind, Message, encode};
use nix::sys::signal::Signal;

mod common;
use common::{Daemon, Locked, free_tcp, free_udp, scrape, sockets, wait_within};

/// A member: a daemon with a `[metrics]` listener on `metrics`.
struct Member {
    daemon: Daemon,
    metrics: SocketAddr,
}

impl std::ops::Deref for Member {
    type Target = Daemon;
    fn deref(&self) -> &Daemon {
        &self.daemon
    }
}

impl std::ops::DerefMut for Member {
    fn deref_mut(&mut self) -> &mut Daemon {
        &mut self.daemon
    }
}

impl Member {
    fn scrape(&self) -> String {
        scrape(self.metrics)
    }
    fn has_line(&self, line: &str) -> bool {
        self.scrape().lines().any(|l| l == line)
    }
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

fn spawn(config_for: impl FnOnce(&Path, SocketAddr) -> String) -> Member {
    spawn_env(config_for, &[])
}

/// [`spawn`] with extra environment variables for the daemon process.
fn spawn_env(
    config_for: impl FnOnce(&Path, SocketAddr) -> String,
    env: &[(&str, &Path)],
) -> Member {
    let metrics = free_tcp();
    Member {
        daemon: Daemon::spawn_with(|dir| config_for(dir, metrics), env),
        metrics,
    }
}

const UNHEALTHY: &str = "changed from healthy to unhealthy";
const HEALTHY: &str = "changed from unknown to healthy";

/// Two members checking one TCP backend: A every second, B every 30
/// seconds (the digest leaves `interval` out, so they still share). `b_extra`
/// is added to B's service.
fn pair(backend: SocketAddr, b_extra: &str) -> (Member, Member, SocketAddr, SocketAddr) {
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
    wait_within("both members live", Duration::from_secs(5), || {
        da.has_line(&format!("emfor_peer_up{{peer=\"{b}\"}} 1"))
            && db.has_line(&format!("emfor_peer_up{{peer=\"{a}\"}} 1"))
    });
    (da, db, a, b)
}

/// Spec peer-group "Peer sees the failure first": B, which would need a
/// minute on its own, follows A within 2 seconds.
#[test]
fn peer_sees_the_failure_first() {
    let held = sockets(|| TcpListener::bind("127.0.0.1:0")).expect("the backend can listen");
    let backend = held.local_addr().expect("the backend has an address");
    let (da, db, a, _b) = pair(backend, "");
    drop(held);
    wait_within("A marks it unhealthy", Duration::from_secs(10), || {
        da.log().contains(UNHEALTHY)
    });
    let first = Instant::now();
    wait_within("B follows within 2s", Duration::from_secs(2), || {
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
    let held = sockets(|| TcpListener::bind("127.0.0.1:0")).expect("the backend can listen");
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
    wait_within("A live at B", Duration::from_secs(5), || {
        db.has_line(&format!("emfor_peer_up{{peer=\"{a}\"}} 1"))
    });
    drop(held);
    wait_within("A marks it unhealthy", Duration::from_secs(10), || {
        da.log().contains(UNHEALTHY)
    });
    wait_within("B accepts the report", Duration::from_secs(2), || {
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
    let held = sockets(|| TcpListener::bind("127.0.0.1:0")).expect("the backend can listen");
    let backend = held.local_addr().expect("the backend has an address");
    let (da, mut db, a, b) = pair(backend, "");
    db.signal(Signal::SIGTERM);
    db.wait();
    let stopped = Instant::now();
    wait_within("B not live at A", Duration::from_secs(5), || {
        da.has_line(&format!("emfor_peer_up{{peer=\"{b}\"}} 0"))
    });
    assert!(stopped.elapsed() <= Duration::from_secs(4) + Duration::from_millis(500));
    da.wait_log(&format!("peer {b} is not live"));

    let c = sockets(|| UdpSocket::bind("127.0.0.1:0")).expect("C can bind");
    c.set_read_timeout(Some(Duration::from_secs(3)))
        .expect("a read timeout can be set");
    let c_addr = c.local_addr().expect("C has an address");
    da.write_whole_config(&config(
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
    d.write_whole_config(&config(
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
    let sb = sockets(|| UdpSocket::bind(b)).expect("B's address is free");
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
            kind: Kind::Heartbeat(Vec::new()),
        },
        SECRET.trim_end().as_bytes(),
    );
    sb.send_to(&heartbeat, a).expect("B can send to A");
    wait_within("B live at A", Duration::from_secs(3), || {
        d.has_line(&format!("emfor_peer_up{{peer=\"{b}\"}} 1"))
    });
}

/// Binding `[peers] listen` at start fails: exit 1 before starting.
#[test]
fn occupied_peer_port_exits_1() {
    let held = sockets(|| UdpSocket::bind("127.0.0.1:0")).expect("a UDP port can be held");
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
fn status(d: &Member) -> (String, serde_json::Value) {
    let cfg = d.dir.path().join("emfor.toml");
    let run = |json: bool| {
        let mut c = Command::new(cargo_bin("emfor"));
        c.arg("status").arg("--config").arg(&cfg);
        if json {
            c.arg("--json");
        }
        let out = c.output_locked().expect("emfor status runs");
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
    let daemons: Vec<Member> = (0..3)
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
    wait_within(
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
    wait_within("C counts the disagreement", Duration::from_secs(10), || {
        c.has_line("emfor_peer_disagreements 1")
    });
    wait_within("meta/peers lists it", Duration::from_secs(10), || {
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
    wait_within(
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
    wait_within("all healthy again", Duration::from_secs(10), || {
        daemons
            .iter()
            .all(|d| d.log().contains("changed from unhealthy to healthy"))
    });
    wait_within("C clears the disagreement", Duration::from_secs(10), || {
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
    let mut daemons: Vec<Member> = (0..4)
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
    wait_within(
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
    wait_within(text, Duration::from_secs(15), || {
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
    wait_within(
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
    wait_within(
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

/// Spec peer-group "A service left out on one member" and state-store
/// "Services left out are listed": C has `peers = false` for the service,
/// so it neither sends nor counts votes, while A and B still decide by
/// theirs.
#[test]
fn service_left_out_on_one_member() {
    let flags = tempfile::tempdir().expect("a temporary directory can be created");
    let flag = |i: usize| flags.path().join(format!("down-{i}"));
    let members = [free_udp(), free_udp(), free_udp()];
    let daemons: Vec<Member> = (0..3)
        .map(|i| {
            let f = flag(i);
            spawn_env(
                |dir, m| {
                    let c = exec_config(dir, m, members[i], &members);
                    if i == 2 {
                        c.replace("jitter = \"0%\"\n", "jitter = \"0%\"\npeers = false\n")
                    } else {
                        c
                    }
                },
                &[("EMFOR_TEST_FLAG", &f)],
            )
        })
        .collect();
    for d in &daemons {
        d.wait_log(HEALTHY);
    }
    wait_within(
        "every member live everywhere",
        Duration::from_secs(10),
        || {
            daemons
                .iter()
                .all(|d| counter(&d.scrape(), "emfor_peer_up", "") == 2)
        },
    );
    let [a, b, c] = [&daemons[0], &daemons[1], &daemons[2]];
    assert_eq!(
        status(c).1["peers"]["services_off"],
        serde_json::json!(["api.svc.emfor"])
    );
    assert_eq!(status(a).1["peers"]["services_off"], serde_json::json!([]));
    // C announces the service, so A and B do not count it as a candidate:
    // with two candidates left the backend has no assigned checkers there.
    // `meta/peers` follows within 2 seconds.
    let c_off = |d: &Member| {
        status(d).1["peers"]["members"]
            .as_array()
            .expect("members is an array")
            .iter()
            .find(|m| m["addr"] == members[2].to_string())
            .map(|m| m["services_off"].clone())
            .expect("C is a member")
    };
    for d in [a, b] {
        wait_within(
            "C's services_off at A and B",
            Duration::from_secs(2),
            || c_off(d) == serde_json::json!(["api.svc.emfor"]),
        );
        let json = status(d).1;
        assert_eq!(
            json["assigned_checkers"][0]["checkers"],
            serde_json::json!([]),
            "{json}"
        );
    }

    // A and B see the backend down, C's own checks pass.
    for i in [0, 1] {
        std::fs::write(flag(i), "").expect("the flag can be written");
    }
    a.wait_log(UNHEALTHY);
    b.wait_log(UNHEALTHY);
    // Well past the 3 checks of 1s that would make C follow the votes.
    std::thread::sleep(Duration::from_secs(5));
    assert!(!c.log().contains(UNHEALTHY), "{}", c.log());
    let body = c.scrape();
    assert_eq!(
        counter(&body, "emfor_peer_results_total", "direction=\"sent\""),
        0
    );
    assert_eq!(
        counter(
            &body,
            "emfor_peer_notifications_total",
            "direction=\"sent\""
        ),
        0
    );
    assert!(c.has_line("emfor_peer_disagreements 0"));
    assert!(c.has_line("emfor_peer_checks_skipped_total 0"));
    let (text, json) = status(c);
    assert_eq!(json["peers"]["disagreements"], serde_json::json!([]));
    assert!(text.lines().all(|l| !l.contains("voted")), "{text}");
    assert!(
        json["assigned_checkers"]
            .as_array()
            .expect("assigned_checkers is an array")
            .iter()
            .all(|e| e["checkers"] == serde_json::json!([])),
        "{json}"
    );
}

/// Spec state-store "Services another member leaves out": a member's
/// heartbeats announce `api.svc.emfor` and `web.svc.emfor`; `meta/peers`
/// names only the one this daemon has, and nothing once the member is not
/// live.
#[test]
fn services_another_member_leaves_out() {
    let held = sockets(|| TcpListener::bind("127.0.0.1:0")).expect("the backend can listen");
    let backend = held.local_addr().expect("the backend has an address");
    let other_socket =
        sockets(|| UdpSocket::bind("127.0.0.1:0")).expect("the other member's address is free");
    let other = other_socket
        .local_addr()
        .expect("the other member has an address");
    let a = free_udp();
    let d = spawn(|dir, m| {
        config(
            dir,
            m,
            a,
            &[a, other],
            backend,
            &format!(
                "\n[services.\"db.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"{backend}\"]\n"
            ),
        )
    });
    d.wait_log(HEALTHY);
    let tags = vec![
        emfor_daemon::quorum::service_tag("api.svc.emfor"),
        emfor_daemon::quorum::service_tag("web.svc.emfor"),
    ];
    let mut seq = 0;
    let mut heartbeat = || {
        seq += 1;
        let data = encode(
            &Message {
                from: other,
                site: "dc1".into(),
                boot: 0x0123_4567_89ab_cdef,
                seq,
                ts: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .expect("the clock is after 1970"),
                kind: Kind::Heartbeat(tags.clone()),
            },
            SECRET.trim_end().as_bytes(),
        );
        other_socket
            .send_to(&data, a)
            .expect("the other member can send");
    };
    let member = |d: &Member| status(d).1["peers"]["members"][0].clone();
    heartbeat();
    let first = Instant::now();
    let mut seen = false;
    while first.elapsed() < Duration::from_secs(2) {
        if member(&d)
            == serde_json::json!({"addr": other.to_string(), "live": true, "services_off": ["api.svc.emfor"]})
        {
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        heartbeat();
    }
    assert!(seen, "{}", member(&d));
    // Stop sending: not live after 3 seconds, and then nothing announced.
    wait_within("the other member not live", Duration::from_secs(5), || {
        d.has_line(&format!("emfor_peer_up{{peer=\"{other}\"}} 0"))
    });
    wait_within("its services forgotten", Duration::from_secs(2), || {
        member(&d)
            == serde_json::json!({"addr": other.to_string(), "live": false, "services_off": []})
    });
}

/// Spec state-store "Services left out are listed": `services_off` at
/// start, and after a reload turns one service back on.
#[test]
fn services_off_follow_reloads() {
    let held = sockets(|| TcpListener::bind("127.0.0.1:0")).expect("the backend can listen");
    let backend = held.local_addr().expect("the backend has an address");
    let (a, other) = (free_udp(), free_udp());
    let cfg = move |dir: &Path, metrics: SocketAddr, web: bool| {
        config(
            dir,
            metrics,
            a,
            &[a, other],
            backend,
            &format!(
                "peers = false\n\n[services.\"web.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"{backend}\"]\npeers = {web}\n\n[services.\"db.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"{backend}\"]\npeers = false\n"
            ),
        )
    };
    let d = spawn(|dir, m| cfg(dir, m, false));
    d.wait_log(HEALTHY);
    let off = |d: &Member| status(d).1["peers"]["services_off"].clone();
    assert_eq!(
        off(&d),
        serde_json::json!(["api.svc.emfor", "db.svc.emfor", "web.svc.emfor"])
    );
    d.write_whole_config(&cfg(d.dir.path(), d.metrics, true));
    d.signal(Signal::SIGHUP);
    d.wait_log("config reloaded");
    let reloaded = Instant::now();
    wait_within(
        "web.svc.emfor back in the group",
        Duration::from_secs(2),
        || off(&d) == serde_json::json!(["api.svc.emfor", "db.svc.emfor"]),
    );
    assert!(reloaded.elapsed() <= Duration::from_secs(2));
}

/// A config for one member at `listen` with the services `api`, `db` and
/// `web` (`.svc.emfor`) on `backend`, whose `peers` settings are `peers`
/// in that order.
fn three_services(
    dir: &Path,
    metrics: SocketAddr,
    listen: SocketAddr,
    members: &[SocketAddr],
    backend: SocketAddr,
    peers: [bool; 3],
) -> String {
    let [api, db, web] = peers;
    config(
        dir,
        metrics,
        listen,
        members,
        backend,
        &format!(
            "peers = {api}\n\n[services.\"db.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"{backend}\"]\npeers = {db}\n\n[services.\"web.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"{backend}\"]\npeers = {web}\n"
        ),
    )
}

/// The `off` field of the next heartbeat `socket` receives, as JSON
/// (`null` when absent).
fn next_heartbeat_off(socket: &UdpSocket) -> serde_json::Value {
    let mut buf = [0u8; 1500];
    loop {
        let n = socket
            .recv(&mut buf)
            .expect("a heartbeat arrives within the timeout");
        // EMP1 and the 32-byte tag, then the JSON payload.
        let payload: serde_json::Value =
            serde_json::from_slice(&buf[36..n]).expect("the payload is JSON");
        if payload["kind"] == "heartbeat" {
            return payload["off"].clone();
        }
    }
}

/// Spec peer-group "Heartbeat announces services left out": heartbeats
/// carry the tags of the services left out, and none after a reload puts
/// them all back in the group.
#[test]
fn heartbeat_announces_services_left_out() {
    let held = sockets(|| TcpListener::bind("127.0.0.1:0")).expect("the backend can listen");
    let backend = held.local_addr().expect("the backend has an address");
    let other =
        sockets(|| UdpSocket::bind("127.0.0.1:0")).expect("the other member's address is free");
    other
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("a read timeout can be set");
    let peer = other.local_addr().expect("the other member has an address");
    let a = free_udp();
    let d = spawn(|dir, m| three_services(dir, m, a, &[a, peer], backend, [false, false, true]));
    d.wait_log(HEALTHY);
    assert_eq!(
        next_heartbeat_off(&other),
        serde_json::json!(["a513a047", "fa6f63b4"])
    );
    d.write_whole_config(&three_services(
        d.dir.path(),
        d.metrics,
        a,
        &[a, peer],
        backend,
        [true; 3],
    ));
    d.signal(Signal::SIGHUP);
    d.wait_log("config reloaded");
    // The heartbeat after the next tick has no `off`.
    wait_within("heartbeats without off", Duration::from_secs(3), || {
        next_heartbeat_off(&other).is_null()
    });
}

/// Spec peer-group "More services left out than fit": 81 services left
/// out give one warning that 1 tag is not sent, also after a reload that
/// changes nothing.
#[test]
fn more_services_left_out_than_fit() {
    let held = sockets(|| TcpListener::bind("127.0.0.1:0")).expect("the backend can listen");
    let backend = held.local_addr().expect("the backend has an address");
    let (a, other) = (free_udp(), free_udp());
    let extra: String = (1..=80)
        .map(|i| {
            format!(
                "\n[services.\"s{i}.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"{backend}\"]\npeers = false\n"
            )
        })
        .collect();
    let cfg = move |dir: &Path, m: SocketAddr| {
        config(
            dir,
            m,
            a,
            &[a, other],
            backend,
            &format!("peers = false\n{extra}"),
        )
    };
    let d = spawn(|dir, m| cfg(dir, m));
    d.wait_log("1 tag is not sent");
    let warnings = |d: &Member| d.log().matches("1 tag is not sent").count();
    assert_eq!(warnings(&d), 1, "{}", d.log());
    d.write_whole_config(&cfg(d.dir.path(), d.metrics));
    d.signal(Signal::SIGHUP);
    d.wait_log("config reloaded");
    // Give a second warning time to show up.
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(warnings(&d), 1, "{}", d.log());
}
