//! End-to-end tests of the `/metrics` endpoint: run the `emfor` binary and
//! scrape it over HTTP. Every test uses a loopback port it found free, never
//! the default 5301.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

mod common;
use common::{Locked, free_tcp, sockets};

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
    /// Everything the daemon wrote to stderr so far, read by `reader`.
    log: Arc<Mutex<String>>,
    reader: Option<JoinHandle<()>>,
}

impl Daemon {
    fn write_config(&self, body: &str) {
        std::fs::write(
            self.dir.path().join("emfor.toml"),
            format!("{}{body}", header(self.dir.path())),
        )
        .unwrap();
    }
    fn signal(&self, s: Signal) {
        kill(Pid::from_raw(self.child.id() as i32), s).unwrap();
    }
    fn log(&self) -> String {
        self.log.lock().unwrap().clone()
    }
    /// Wait until the daemon has logged `text`.
    fn wait_log(&self, text: &str) {
        wait_for(|| self.log().contains(text));
    }
    /// Wait for the daemon to exit; returns (exit code, all of stderr).
    fn wait(&mut self) -> (i32, String) {
        let status = self.child.wait().unwrap();
        if let Some(r) = self.reader.take() {
            r.join().unwrap();
        }
        (status.code().unwrap_or(-1), self.log())
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

fn metrics_table(addr: SocketAddr) -> String {
    format!("[metrics]\nlisten = [\"{addr}\"]\n\n")
}

fn service(name: &str, backends: &[String]) -> String {
    let list: Vec<String> = backends.iter().map(|b| format!("\"{b}\"")).collect();
    format!(
        "[services.\"{name}\"]\ncheck = \"tcp\"\nbackends = [{}]\ninterval = \"200ms\"\ntimeout = \"100ms\"\nrise = 1\nfall = 1\njitter = \"0%\"\n\n",
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
        .spawn_locked()
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

/// `GET /metrics`; the whole response, head and body.
fn scrape(addr: SocketAddr) -> String {
    let mut s = sockets(|| TcpStream::connect(addr)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"GET /metrics HTTP/1.1\r\nHost: test\r\n\r\n")
        .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

/// Scrape until the body has a line equal to `line`.
fn wait_line(addr: SocketAddr, line: &str) -> String {
    let mut last = String::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        last = scrape(addr);
        if last.lines().any(|l| l == line) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("no line {line} within 10s; last response:\n{last}");
}

/// Spec "Metrics endpoint enabled by the metrics table", scenario "No
/// metrics table".
#[test]
fn no_metrics_table_no_server() {
    let backend = sockets(|| TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = backend.local_addr().unwrap();
    let d = spawn(&service("api.svc.emfor", &[addr.to_string()]));
    d.wait_log("emfor started");
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
    assert!(!log.contains("metrics on"), "{log}");
}

/// A scrape answers 200 with the metrics, and the server starts before
/// `emfor started`.
#[test]
fn serves_metrics() {
    let backend = sockets(|| TcpListener::bind("127.0.0.1:0")).unwrap();
    let up = backend.local_addr().unwrap().to_string();
    let addr = free_tcp();
    let d = spawn(&format!(
        "{}{}",
        metrics_table(addr),
        service("api.svc.emfor", std::slice::from_ref(&up))
    ));
    d.wait_log("emfor started");
    let log = d.log();
    // stderr is a pipe here, as under systemd: no color escape sequences.
    assert!(!log.contains('\x1b'), "{log:?}");
    let on = log.find(&format!("metrics on {addr}")).expect(&log);
    assert!(on < log.find("emfor started").unwrap(), "{log}");
    let resp = wait_line(
        addr,
        &format!(
            "emfor_backend_state{{service=\"api.svc.emfor\",backend=\"{up}\",state=\"healthy\"}} 1"
        ),
    );
    assert!(resp.starts_with("HTTP/1.1 200 OK\r\n"), "{resp}");
    assert!(
        resp.contains("\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\n"),
        "{resp}"
    );
    assert!(
        resp.contains(&format!(
            "\nemfor_build_info{{version=\"{}\"}} 1\n",
            env!("CARGO_PKG_VERSION")
        )),
        "{resp}"
    );
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
}

/// Spec metrics-endpoint "Daemon metrics": store usage, last commit, and
/// a plugin's up state and restarts.
#[test]
fn store_and_plugin_metrics() {
    let addr = free_tcp();
    let plugin = env!("CARGO_BIN_EXE_emfor-test-plugin");
    let d = spawn(&format!(
        "{}[plugins.\"p\"]\ncommand = [\"{plugin}\"]\n\n[services.\"x.svc.emfor\"]\ncheck = \"p\"\nbackends = [\"10.0.2.1:6379\"]\ninterval = \"5s\"\ntimeout = \"1s\"\njitter = \"0%\"\n[services.\"x.svc.emfor\".check_options]\nmode = \"crash\"\n\n",
        metrics_table(addr)
    ));
    d.wait_log("emfor started");
    // The first check makes the plugin exit; it is started again after 1s.
    let body = wait_line(addr, "emfor_plugin_restarts_total{plugin=\"p\"} 1");
    assert!(body.contains("\n# TYPE emfor_plugin_up gauge\n"), "{body}");
    assert!(body.contains("\nemfor_plugin_up{plugin=\"p\"} "), "{body}");
    assert!(
        body.contains("\n# TYPE emfor_plugin_restarts_total counter\n"),
        "{body}"
    );
    assert!(
        body.contains("\nemfor_store_map_size_bytes 4194304\n"),
        "{body}"
    );
    let used: u64 = body
        .lines()
        .find_map(|l| l.strip_prefix("emfor_store_used_bytes "))
        .expect(&body)
        .parse()
        .unwrap();
    assert!(used > 0 && used <= 4194304, "{used}");
    let last: f64 = body
        .lines()
        .find_map(|l| l.strip_prefix("emfor_store_last_commit_timestamp_seconds "))
        .expect(&body)
        .parse()
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    assert!((now - last).abs() < 5.0, "{last} vs {now}");
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
}

/// Spec scenario "Address in use".
#[test]
fn address_in_use_exits_1() {
    let held = sockets(|| TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = held.local_addr().unwrap();
    let mut d = spawn(&metrics_table(addr));
    let (code, log) = d.wait();
    assert_eq!(code, 1, "{log}");
    assert!(log.contains(&addr.to_string()), "{log}");
    assert!(!log.contains("emfor started"), "{log}");
}

/// Service-config "Reload on SIGHUP": a changed `[metrics] listen` rejects
/// the reload and the old address keeps answering.
#[test]
fn reload_rejects_listen_change() {
    let addr = free_tcp();
    let d = spawn(&metrics_table(addr));
    d.wait_log("emfor started");
    let other = free_tcp();
    d.write_config(&metrics_table(other));
    d.signal(Signal::SIGHUP);
    d.wait_log("reload rejected");
    let log = d.log();
    assert!(
        log.contains("metrics.listen") && log.contains("metrics listen settings require restart"),
        "{log}"
    );
    assert!(scrape(addr).starts_with("HTTP/1.1 200 OK\r\n"));
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
}

/// Spec scenario "Slow client": a connection that sends nothing is closed
/// after 5 seconds without a response, and other requests are answered
/// meanwhile.
#[test]
fn slow_client_closed() {
    let addr = free_tcp();
    let d = spawn(&metrics_table(addr));
    d.wait_log("emfor started");
    let mut idle = sockets(|| TcpStream::connect(addr)).unwrap();
    idle.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let start = Instant::now();
    assert!(scrape(addr).starts_with("HTTP/1.1 200 OK\r\n"));
    let mut out = Vec::new();
    // A reset counts as closed too.
    let _ = idle.read_to_end(&mut out);
    let waited = start.elapsed();
    assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));
    assert!(
        waited >= Duration::from_millis(4500) && waited < Duration::from_secs(8),
        "{waited:?}"
    );
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
}

/// A refused backend reads `state="unhealthy"` 1, and a service removed by
/// a reload is gone from the next response (spec scenario "Removed service
/// disappears").
#[test]
fn unhealthy_backend_and_removed_service() {
    // Bound and dropped: connections are refused.
    let refused = free_tcp().to_string();
    let backend = sockets(|| TcpListener::bind("127.0.0.1:0")).unwrap();
    let up = backend.local_addr().unwrap().to_string();
    let addr = free_tcp();
    let keep = service("api.svc.emfor", std::slice::from_ref(&up));
    let d = spawn(&format!(
        "{}{keep}{}",
        metrics_table(addr),
        service("old.svc.emfor", std::slice::from_ref(&refused))
    ));
    d.wait_log("emfor started");
    let resp = wait_line(
        addr,
        &format!(
            "emfor_backend_state{{service=\"old.svc.emfor\",backend=\"{refused}\",state=\"unhealthy\"}} 1"
        ),
    );
    assert!(
        resp.lines()
            .any(|l| l == "emfor_service_all_down{service=\"old.svc.emfor\"} 1"),
        "{resp}"
    );
    d.write_config(&format!("{}{keep}", metrics_table(addr)));
    d.signal(Signal::SIGHUP);
    d.wait_log("config reloaded");
    // The writer applies the new layout shortly after the reload is logged.
    wait_for(|| !scrape(addr).contains("old.svc.emfor"));
    let resp = scrape(addr);
    assert!(resp.contains("service=\"api.svc.emfor\""), "{resp}");
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
}

/// One service of 10 refused backends gives 107 series once every backend
/// is checked: 10 per backend (state 3, last check 1, checks 3,
/// transitions 3) plus 6 for the daemon (3 of them for the store, once it
/// has committed) and 1 for the service. The count
/// in docs/deploy/daemon.md, section "Metrics served".
#[test]
fn series_for_ten_backends() {
    let backends: Vec<String> = (0..10).map(|_| free_tcp().to_string()).collect();
    let addr = free_tcp();
    let d = spawn(&format!(
        "{}{}",
        metrics_table(addr),
        service("api.svc.emfor", &backends)
    ));
    d.wait_log("emfor started");
    let mut body = String::new();
    wait_for(|| {
        body = scrape(addr);
        body.matches("state=\"unhealthy\"} 1\n").count() == 10
            && body
                .matches("emfor_backend_last_check_timestamp_seconds{")
                .count()
                == 10
    });
    let (_, body) = body.split_once("\r\n\r\n").unwrap();
    let series = body.lines().filter(|l| !l.starts_with('#')).count();
    assert_eq!(series, 107, "{body}");
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
}
