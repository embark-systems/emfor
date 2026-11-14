//! The `/metrics` endpoint: the live state and the check counters in the
//! Prometheus text exposition format 0.0.4.
//!
//! [`render`] turns a [`LiveView`] into the response body; the HTTP side is
//! a deliberately small HTTP/1.1 server that answers one request per
//! connection and then closes it, which is all a Prometheus scrape needs.

use std::fmt::Write as _;

/// `writeln!` into a `String`. `fmt::Write` for `String` never returns an
/// error, so there is nothing to handle.
macro_rules! put {
    ($out:expr, $($arg:tt)*) => {{
        let _ = writeln!($out, $($arg)*);
    }};
}
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use emfor_format::{Balance, ServiceRecord};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use crate::live::{BackendCounters, LiveState, LiveView};

/// Longest request head (request line and headers) accepted.
const MAX_HEAD: usize = 8192;
/// Time a client has to send its request head, and to take the response.
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Connections served at once; more are closed without a response.
const MAX_CONNECTIONS: usize = 16;
/// Time to read leftover input after the response before closing.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// The running server; it stops accepting connections when dropped.
pub struct MetricsServer {
    addrs: Vec<SocketAddr>,
    tasks: Vec<JoinHandle<()>>,
}

impl MetricsServer {
    /// The bound addresses, with the ports the system chose for port 0.
    pub fn local_addrs(&self) -> &[SocketAddr] {
        &self.addrs
    }
}

impl Drop for MetricsServer {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Bind every listen address and start serving `/metrics` from `live`.
/// Errors name the address.
pub async fn serve(listen: &[SocketAddr], live: LiveState) -> Result<MetricsServer, String> {
    let mut listeners = Vec::with_capacity(listen.len());
    for addr in listen {
        let l = TcpListener::bind(addr)
            .await
            .map_err(|e| format!("cannot listen on {addr} (tcp): {e}"))?;
        listeners.push(l);
    }
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut server = MetricsServer {
        addrs: Vec::with_capacity(listeners.len()),
        tasks: Vec::with_capacity(listeners.len()),
    };
    for l in listeners {
        server.addrs.push(
            l.local_addr()
                .map_err(|e| format!("metrics listener: {e}"))?,
        );
        let (permits, live) = (permits.clone(), live.clone());
        server.tasks.push(tokio::spawn(async move {
            loop {
                let stream = match l.accept().await {
                    Ok((s, _)) => s,
                    // Out of file descriptors and the like: back off.
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    continue; // dropping the stream closes it
                };
                let live = live.clone();
                tokio::spawn(async move {
                    connection(stream, &live).await;
                    drop(permit);
                });
            }
        }));
    }
    Ok(server)
}

/// Serve one request on `stream`, then close it.
async fn connection(mut stream: TcpStream, live: &LiveState) {
    let mut buf = vec![0u8; MAX_HEAD];
    let mut len = 0;
    let complete = tokio::time::timeout(IO_TIMEOUT, async {
        loop {
            if find_end(&buf[..len]).is_some() {
                return true;
            }
            if len == MAX_HEAD {
                return true; // too long; answered with 400
            }
            match stream.read(&mut buf[len..]).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => len += n,
            }
        }
    })
    .await;
    if complete != Ok(true) {
        return;
    }
    let head = find_end(&buf[..len]).map(|end| &buf[..end]);
    let response = respond(head, || {
        let view = live.read();
        view.ready.then(|| render(&view, env!("CARGO_PKG_VERSION")))
    });
    let _ = tokio::time::timeout(IO_TIMEOUT, async {
        stream.write_all(&response).await?;
        stream.shutdown().await
    })
    .await;
    // Read what the client still sends (a body, the rest of a long head)
    // before closing: closing with unread input resets the connection,
    // and the client may then lose the response.
    let _ = tokio::time::timeout(DRAIN_TIMEOUT, async {
        let mut sink = [0u8; 1024];
        while let Ok(n) = stream.read(&mut sink).await
            && n > 0
        {}
    })
    .await;
}

/// Length of the head including its blank line, if `buf` holds all of it.
fn find_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// Fuzzing entry point (`fuzz/fuzz_targets/metrics.rs`): answer `data` as
/// the bytes a client sent.
#[cfg(fuzzing)]
pub fn fuzz_request(data: &[u8]) {
    let head = find_end(data).map(|end| &data[..end]);
    let _ = respond(head, || Some(String::new()));
}

/// The full response for a request `head` (`None`: longer than
/// [`MAX_HEAD`]); `body` is called only for a `GET /metrics` and returns
/// `None` until the daemon has applied its first layout.
fn respond(head: Option<&[u8]>, body: impl FnOnce() -> Option<String>) -> Vec<u8> {
    let (status, extra, body) = match head.and_then(parse_head) {
        None => ("400 Bad Request", "", "bad request\n".to_owned()),
        Some((_, path)) if path != "/metrics" => ("404 Not Found", "", "not found\n".to_owned()),
        Some((method, _)) if method != "GET" => (
            "405 Method Not Allowed",
            "Allow: GET\r\n",
            "method not allowed\n".to_owned(),
        ),
        Some(_) => match body() {
            Some(b) => ("200 OK", "", b),
            None => ("503 Service Unavailable", "", "not ready\n".to_owned()),
        },
    };
    let ct = if status.starts_with("200") {
        CONTENT_TYPE
    } else {
        "text/plain; charset=utf-8"
    };
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// Method and path (without the query) of a valid HTTP/1.x head ending in
/// a blank line.
fn parse_head(head: &[u8]) -> Option<(&str, &str)> {
    let head = std::str::from_utf8(head).ok()?;
    let mut lines = head.strip_suffix("\r\n\r\n")?.split("\r\n");
    let mut parts = lines.next()?.split(' ');
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    let valid_version = version.len() == 8
        && version.starts_with("HTTP/1.")
        && version.as_bytes()[7].is_ascii_digit();
    if parts.next().is_some()
        || method.is_empty()
        || !method.bytes().all(|b| b.is_ascii_alphabetic())
        || !target.starts_with('/')
        || !valid_version
    {
        return None;
    }
    for line in lines {
        let (name, _) = line.split_once(':')?;
        if name.is_empty() || name.contains(' ') {
            return None;
        }
    }
    Some((method, target.split('?').next().unwrap_or(target)))
}

/// Seconds with millisecond precision: `1762905600.123`.
fn seconds(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

/// A label value with `\`, `"` and newline escaped.
fn label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

fn family(out: &mut String, name: &str, kind: &str, help: &str) {
    put!(out, "# HELP {name} {help}");
    put!(out, "# TYPE {name} {kind}");
}

/// The response body for `view`.
pub fn render(view: &LiveView, version: &str) -> String {
    let mut out = String::new();
    family(
        &mut out,
        "emfor_build_info",
        "gauge",
        "emfor version, always 1.",
    );
    put!(out, "emfor_build_info{{version=\"{}\"}} 1", label(version));
    family(
        &mut out,
        "emfor_start_time_seconds",
        "gauge",
        "Daemon start time in Unix seconds.",
    );
    put!(
        out,
        "emfor_start_time_seconds {}",
        seconds(view.started_at_ms)
    );
    family(
        &mut out,
        "emfor_store_commit_failures_total",
        "counter",
        "Failed state store commits since the daemon started.",
    );
    put!(
        out,
        "emfor_store_commit_failures_total {}",
        view.commit_failures
    );
    if view.last_commit_ms > 0 {
        family(
            &mut out,
            "emfor_store_last_commit_timestamp_seconds",
            "gauge",
            "Unix time of the last successful state store commit.",
        );
        put!(
            out,
            "emfor_store_last_commit_timestamp_seconds {}",
            seconds(view.last_commit_ms)
        );
    }
    if let Some((used, map)) = view.store_usage {
        family(
            &mut out,
            "emfor_store_used_bytes",
            "gauge",
            "Bytes of the state store in use; commits fail when this reaches emfor_store_map_size_bytes.",
        );
        put!(out, "emfor_store_used_bytes {used}");
        family(
            &mut out,
            "emfor_store_map_size_bytes",
            "gauge",
            "Size of the state store map ([store] map_size).",
        );
        put!(out, "emfor_store_map_size_bytes {map}");
    }
    if !view.plugins.is_empty() {
        family(
            &mut out,
            "emfor_plugin_up",
            "gauge",
            "1 while the plugin process runs and answered the handshake.",
        );
        for (name, s) in &view.plugins {
            let up = u8::from(s.up.load(std::sync::atomic::Ordering::Relaxed));
            put!(out, "emfor_plugin_up{{plugin=\"{}\"}} {up}", label(name));
        }
        family(
            &mut out,
            "emfor_plugin_restarts_total",
            "counter",
            "Times the plugin was started again after it exited.",
        );
        for (name, s) in &view.plugins {
            let n = s.restarts.load(std::sync::atomic::Ordering::Relaxed);
            put!(
                out,
                "emfor_plugin_restarts_total{{plugin=\"{}\"}} {n}",
                label(name)
            );
        }
    }

    if let Some(peers) = &view.peers {
        family(
            &mut out,
            "emfor_peer_up",
            "gauge",
            "1 while a message from this peer group member was accepted in the last 3s.",
        );
        for (member, live) in peers.up() {
            put!(out, "emfor_peer_up{{peer=\"{member}\"}} {}", u8::from(live));
        }
        family(
            &mut out,
            "emfor_peer_messages_rejected_total",
            "counter",
            "Peer messages dropped, by the first check they failed.",
        );
        for r in crate::peers::Reject::ALL {
            put!(
                out,
                "emfor_peer_messages_rejected_total{{reason=\"{}\"}} {}",
                r.as_str(),
                peers.rejected(r)
            );
        }
        family(
            &mut out,
            "emfor_peer_notifications_total",
            "counter",
            "Backend failure messages sent (one per member) and accepted.",
        );
        put!(
            out,
            "emfor_peer_notifications_total{{direction=\"sent\"}} {}",
            peers.sent()
        );
        put!(
            out,
            "emfor_peer_notifications_total{{direction=\"received\"}} {}",
            peers.received()
        );
        family(
            &mut out,
            "emfor_peer_results_total",
            "counter",
            "Check result messages sent (one per member) and accepted.",
        );
        put!(
            out,
            "emfor_peer_results_total{{direction=\"sent\"}} {}",
            peers.results_sent()
        );
        put!(
            out,
            "emfor_peer_results_total{{direction=\"received\"}} {}",
            peers.results_received()
        );
        family(
            &mut out,
            "emfor_peer_disagreements",
            "gauge",
            "Backends that a majority of their assigned checkers see down while this daemon's own check passes.",
        );
        put!(out, "emfor_peer_disagreements {}", peers.disagreements());
        family(
            &mut out,
            "emfor_peer_checks_skipped_total",
            "counter",
            "Scheduled checks not made because the votes of the backend's assigned checkers decided the result.",
        );
        put!(
            out,
            "emfor_peer_checks_skipped_total {}",
            peers.checks_skipped()
        );
    }

    let mut names: Vec<&String> = view.services.keys().collect();
    names.sort();
    let records: Vec<_> = names.iter().map(|n| &view.services[*n].record).collect();

    family(
        &mut out,
        "emfor_service_all_down",
        "gauge",
        "1 when no backend of the service is healthy.",
    );
    for r in &records {
        put!(
            out,
            "emfor_service_all_down{{service=\"{}\"}} {}",
            label(&r.name),
            u8::from(r.all_down)
        );
    }

    // Every backend of every service, in that order, with its labels.
    let backends: Vec<_> = records
        .iter()
        .flat_map(|r| {
            r.backends.iter().map(move |b| {
                let labels = format!(
                    "service=\"{}\",backend=\"{}\"",
                    label(&r.name),
                    label(&b.id)
                );
                (*r, b, labels)
            })
        })
        .collect();

    family(
        &mut out,
        "emfor_backend_state",
        "gauge",
        "1 for the backend's current health state, 0 for the others.",
    );
    for (_, b, l) in &backends {
        for s in BackendCounters::STATES {
            put!(
                out,
                "emfor_backend_state{{{l},state=\"{s}\"}} {}",
                u8::from(b.state == s)
            );
        }
    }
    family(
        &mut out,
        "emfor_backend_check_latency_seconds",
        "gauge",
        "Smoothed check latency of a healthy backend.",
    );
    for (_, b, l) in &backends {
        if let Some(us) = b.latency_us {
            put!(
                out,
                "emfor_backend_check_latency_seconds{{{l}}} {}",
                us as f64 / 1e6
            );
        }
    }
    family(
        &mut out,
        "emfor_backend_last_check_timestamp_seconds",
        "gauge",
        "Unix time when the last check of the backend finished.",
    );
    for (_, b, l) in &backends {
        if let Some(ms) = b.checked_at {
            put!(
                out,
                "emfor_backend_last_check_timestamp_seconds{{{l}}} {}",
                seconds(ms)
            );
        }
    }
    family(
        &mut out,
        "emfor_backend_in_pool",
        "gauge",
        "1 for members of a fastest service's pool.",
    );
    for (r, b, l) in &backends {
        if r.balance == Balance::Fastest {
            let in_pool = r.pool.as_ref().is_some_and(|p| p.contains(&b.id));
            put!(out, "emfor_backend_in_pool{{{l}}} {}", u8::from(in_pool));
        }
    }

    let counters = |r: &ServiceRecord, id: &str| {
        view.counters
            .get(&(r.name.clone(), id.to_owned()))
            .copied()
            .unwrap_or_default()
    };
    family(
        &mut out,
        "emfor_backend_checks_total",
        "counter",
        "Checks of the backend by result since it appeared in the running daemon.",
    );
    for (r, b, l) in &backends {
        let c = counters(r, &b.id);
        for (o, name) in BackendCounters::OUTCOMES {
            put!(
                out,
                "emfor_backend_checks_total{{{l},result=\"{name}\"}} {}",
                c.checks[BackendCounters::outcome_index(o)]
            );
        }
    }
    family(
        &mut out,
        "emfor_backend_transitions_total",
        "counter",
        "Health state transitions of the backend by new state since it appeared in the running daemon.",
    );
    for (r, b, l) in &backends {
        let c = counters(r, &b.id);
        for s in BackendCounters::STATES {
            put!(
                out,
                "emfor_backend_transitions_total{{{l},to=\"{s}\"}} {}",
                c.transitions[BackendCounters::state_index(s)]
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::LiveService;
    use emfor_format::{BackendRecord, HealthState, OnAllDown};

    fn backend(id: &str, state: HealthState) -> BackendRecord {
        let (ip, port) = id.rsplit_once(':').unwrap();
        BackendRecord {
            id: id.into(),
            ip: ip.trim_matches(['[', ']']).into(),
            port: port.parse().unwrap(),
            weight: 1,
            site: None,
            state,
            checked_at: None,
            reason: None,
            latency_us: None,
            recovered_at: None,
        }
    }

    fn service(name: &str, balance: Balance, backends: Vec<BackendRecord>) -> ServiceRecord {
        let mut r = ServiceRecord {
            name: name.into(),
            on_all_down: OnAllDown::Open,
            balance,
            all_down: true,
            slow_start_ms: 0,
            backends,
            pool: None,
        };
        r.refresh_all_down();
        r
    }

    fn view(services: Vec<ServiceRecord>) -> LiveView {
        let mut v = LiveView {
            started_at_ms: 1762905000456,
            ..LiveView::default()
        };
        for r in services {
            for b in &r.backends {
                v.counters
                    .insert((r.name.clone(), b.id.clone()), BackendCounters::default());
            }
            v.services.insert(r.name.clone(), LiveService::new(r, None));
        }
        v
    }

    #[test]
    fn store_and_plugin_series() {
        use crate::live::PluginStatus;
        let mut v = view(vec![]);
        let body = render(&v, "0.1.0");
        for name in [
            "emfor_store_last_commit_timestamp_seconds",
            "emfor_store_used_bytes",
            "emfor_plugin_up",
        ] {
            assert!(!body.contains(name), "{name} before it is known: {body}");
        }
        v.last_commit_ms = 1762905600500;
        v.store_usage = Some((65536, 4194304));
        let down = PluginStatus::default();
        down.restarts.store(3, std::sync::atomic::Ordering::Relaxed);
        v.plugins = vec![
            ("redis".into(), std::sync::Arc::new(down)),
            (
                "my\"sql".into(),
                std::sync::Arc::new(PluginStatus {
                    up: true.into(),
                    restarts: 0.into(),
                }),
            ),
        ];
        let body = render(&v, "0.1.0");
        for line in [
            "emfor_store_last_commit_timestamp_seconds 1762905600.500",
            "emfor_store_used_bytes 65536",
            "emfor_store_map_size_bytes 4194304",
            "emfor_plugin_up{plugin=\"redis\"} 0",
            "emfor_plugin_up{plugin=\"my\\\"sql\"} 1",
            "emfor_plugin_restarts_total{plugin=\"redis\"} 3",
        ] {
            assert!(body.lines().any(|l| l == line), "{line}: {body}");
        }
    }

    /// Spec metrics-endpoint "Peer metrics", and no peer series without
    /// `[peers]`.
    #[test]
    fn peer_metrics() {
        use crate::peers::{Group, Keys, Kind, Message, PeerStats, encode};
        use std::sync::Arc;
        let me: std::net::SocketAddr = "127.0.0.1:5302".parse().unwrap();
        let other: std::net::SocketAddr = "127.0.0.1:5303".parse().unwrap();
        let stats = Arc::new(PeerStats::default());
        let mut g = Group::new(
            me,
            "dc1".into(),
            vec![me, other],
            Keys::new(vec![1u8; 32]),
            7,
            stats.clone(),
        );
        let now = tokio::time::Instant::now();
        let t = 1_763_000_000_000;
        let hb = |seq, key: &[u8]| {
            encode(
                &Message {
                    from: other,
                    site: "dc1".into(),
                    boot: 1,
                    seq,
                    ts: t,
                    kind: Kind::Heartbeat(Vec::new()),
                },
                key,
            )
        };
        assert!(g.accept(&hb(1, &[1u8; 32]), other, t, now).is_ok());
        assert!(g.accept(&hb(2, &[2u8; 32]), other, t, now).is_err());
        let mut v = view(vec![]);
        assert!(!render(&v, "0.1.0").contains("emfor_peer_"));
        v.peers = Some(stats.clone());
        let body = render(&v, "0.1.0");
        for line in [
            "emfor_peer_results_total{direction=\"sent\"} 0",
            "emfor_peer_disagreements 0",
            "emfor_peer_checks_skipped_total 0",
            "emfor_peer_up{peer=\"127.0.0.1:5303\"} 1",
            "emfor_peer_messages_rejected_total{reason=\"auth\"} 1",
            "emfor_peer_messages_rejected_total{reason=\"replay\"} 0",
            "emfor_peer_notifications_total{direction=\"sent\"} 0",
            "emfor_peer_notifications_total{direction=\"received\"} 0",
        ] {
            assert!(body.lines().any(|l| l == line), "{line}: {body}");
        }
        // Spec metrics-endpoint "Peer result metrics": an assigned checker
        // in a group of three made 4 up checks and has one disagreement.
        stats.set_results(8, 3, 1);
        let body = render(&v, "0.1.0");
        for line in [
            "emfor_peer_results_total{direction=\"sent\"} 8",
            "emfor_peer_results_total{direction=\"received\"} 3",
            "emfor_peer_disagreements 1",
        ] {
            assert!(body.lines().any(|l| l == line), "{line}: {body}");
        }
        // Spec metrics-endpoint "Skipped checks counted".
        stats.set_checks_skipped(8);
        let body = render(&v, "0.1.0");
        assert!(
            body.lines()
                .any(|l| l == "emfor_peer_checks_skipped_total 8"),
            "{body}"
        );
        // A member removed on reload loses its series.
        g.set_members(vec![me, "127.0.0.1:5304".parse().unwrap()]);
        let body = render(&v, "0.1.0");
        assert!(!body.contains("127.0.0.1:5303"), "{body}");
        assert!(
            body.contains("emfor_peer_up{peer=\"127.0.0.1:5304\"} 0"),
            "{body}"
        );
    }

    fn api() -> ServiceRecord {
        let mut good = backend("10.0.0.11:8080", HealthState::Healthy);
        good.latency_us = Some(1200);
        good.checked_at = Some(1762905600123);
        service(
            "api.svc.emfor",
            Balance::RoundRobin,
            vec![good, backend("10.0.0.12:8080", HealthState::Unhealthy)],
        )
    }

    /// Spec "Backend and service metrics", scenario "One healthy and one
    /// failing backend".
    #[test]
    fn one_healthy_and_one_failing_backend() {
        let body = render(&view(vec![api()]), "0.1.0");
        let lines: Vec<&str> = body.lines().collect();
        for want in [
            r#"emfor_backend_state{service="api.svc.emfor",backend="10.0.0.11:8080",state="healthy"} 1"#,
            r#"emfor_backend_state{service="api.svc.emfor",backend="10.0.0.12:8080",state="healthy"} 0"#,
            r#"emfor_backend_state{service="api.svc.emfor",backend="10.0.0.12:8080",state="unhealthy"} 1"#,
            r#"emfor_backend_check_latency_seconds{service="api.svc.emfor",backend="10.0.0.11:8080"} 0.0012"#,
            r#"emfor_backend_last_check_timestamp_seconds{service="api.svc.emfor",backend="10.0.0.11:8080"} 1762905600.123"#,
            r#"emfor_service_all_down{service="api.svc.emfor"} 0"#,
            r#"emfor_backend_checks_total{service="api.svc.emfor",backend="10.0.0.12:8080",result="error"} 0"#,
            r#"emfor_backend_transitions_total{service="api.svc.emfor",backend="10.0.0.11:8080",to="unknown"} 0"#,
        ] {
            assert!(lines.contains(&want), "missing {want}\n{body}");
        }
        assert!(
            !body.contains(r#"emfor_backend_check_latency_seconds{service="api.svc.emfor",backend="10.0.0.12:8080"}"#),
            "{body}"
        );
        assert!(
            !body.contains(r#"emfor_backend_last_check_timestamp_seconds{service="api.svc.emfor",backend="10.0.0.12:8080"}"#)
        );
        // Not a fastest service: no pool series.
        assert!(!body.contains("emfor_backend_in_pool{"), "{body}");
    }

    /// Spec "Daemon metrics" and the HELP / TYPE lines.
    #[test]
    fn daemon_metrics_and_families() {
        let mut v = view(vec![api()]);
        v.commit_failures = 2;
        let body = render(&v, "0.1.0");
        assert!(body.starts_with(
            "# HELP emfor_build_info emfor version, always 1.\n# TYPE emfor_build_info gauge\nemfor_build_info{version=\"0.1.0\"} 1\n"
        ));
        assert!(body.contains("\nemfor_start_time_seconds 1762905000.456\n"));
        assert!(body.contains("\nemfor_store_commit_failures_total 2\n"));
        for (name, kind) in [
            ("emfor_backend_state", "gauge"),
            ("emfor_backend_checks_total", "counter"),
            ("emfor_backend_transitions_total", "counter"),
        ] {
            assert_eq!(body.matches(&format!("# TYPE {name} {kind}\n")).count(), 1);
            assert_eq!(body.matches(&format!("# HELP {name} ")).count(), 1);
        }
    }

    #[test]
    fn counters_and_order() {
        let db = service(
            "db.svc.emfor",
            Balance::RoundRobin,
            vec![backend("10.0.3.1:5432", HealthState::Unknown)],
        );
        let mut v = view(vec![db, api()]);
        let c = v
            .counters
            .get_mut(&("api.svc.emfor".into(), "10.0.0.11:8080".into()))
            .unwrap();
        c.checks = [2, 3, 0];
        c.transitions = [0, 1, 1];
        let body = render(&v, "0.1.0");
        for want in [
            r#"emfor_backend_checks_total{service="api.svc.emfor",backend="10.0.0.11:8080",result="up"} 2"#,
            r#"emfor_backend_checks_total{service="api.svc.emfor",backend="10.0.0.11:8080",result="down"} 3"#,
            r#"emfor_backend_transitions_total{service="api.svc.emfor",backend="10.0.0.11:8080",to="healthy"} 1"#,
            r#"emfor_backend_transitions_total{service="api.svc.emfor",backend="10.0.0.11:8080",to="unhealthy"} 1"#,
        ] {
            assert!(body.lines().any(|l| l == want), "missing {want}\n{body}");
        }
        // Services in name order, backends in record order.
        let order: Vec<&str> = body
            .lines()
            .filter(|l| l.starts_with("emfor_backend_state{") && l.contains("state=\"unknown\""))
            .collect();
        assert_eq!(order.len(), 3, "{order:?}");
        assert!(order[0].contains("10.0.0.11:8080"));
        assert!(order[1].contains("10.0.0.12:8080"));
        assert!(order[2].contains("db.svc.emfor"));
    }

    #[test]
    fn fastest_pool() {
        let mut f = api();
        f.balance = Balance::Fastest;
        f.pool = Some(vec!["10.0.0.11:8080".into()]);
        let body = render(&view(vec![f]), "0.1.0");
        assert!(body.contains(
            "\nemfor_backend_in_pool{service=\"api.svc.emfor\",backend=\"10.0.0.11:8080\"} 1\n"
        ));
        assert!(body.contains(
            "\nemfor_backend_in_pool{service=\"api.svc.emfor\",backend=\"10.0.0.12:8080\"} 0\n"
        ));
    }

    #[test]
    fn label_escaping() {
        assert_eq!(label("a\\b\"c\nd"), "a\\\\b\\\"c\\nd");
        assert_eq!(label("[2001:db8::5]:5432"), "[2001:db8::5]:5432");
        assert!(render(&view(vec![]), "1\"x").contains("emfor_build_info{version=\"1\\\"x\"} 1"));
    }

    /// A server on a free port whose state has its first (empty) layout.
    async fn server() -> MetricsServer {
        let live = LiveState::default();
        live.replace(Default::default(), None);
        serve(&["127.0.0.1:0".parse().unwrap()], live)
            .await
            .unwrap()
    }

    /// Send `request` and read until the server closes the connection.
    async fn exchange(addr: SocketAddr, request: &[u8]) -> String {
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(request).await.unwrap();
        // A connection closed without a response may be reset.
        let mut out = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(n) = s.read(&mut chunk).await
            && n > 0
        {
            out.extend_from_slice(&chunk[..n]);
        }
        String::from_utf8(out).unwrap()
    }

    /// Spec "Metrics HTTP behavior", example "request outcomes".
    #[tokio::test]
    async fn request_outcomes() {
        let srv = server().await;
        let addr = srv.local_addrs()[0];
        let long = format!(
            "GET /metrics HTTP/1.1\r\nX-Pad: {}\r\n\r\n",
            "a".repeat(9000)
        );
        for (request, status) in [
            ("GET /metrics HTTP/1.1\r\nHost: x\r\n\r\n", "200 OK"),
            ("GET /metrics?x=1 HTTP/1.0\r\n\r\n", "200 OK"),
            ("GET / HTTP/1.1\r\n\r\n", "404 Not Found"),
            (
                "POST /metrics HTTP/1.1\r\nContent-Length: 2\r\n\r\nhi",
                "405 Method Not Allowed",
            ),
            ("HELLO\r\n\r\n", "400 Bad Request"),
            (long.as_str(), "400 Bad Request"),
        ] {
            let resp = exchange(addr, request.as_bytes()).await;
            let (head, body) = resp.split_once("\r\n\r\n").unwrap();
            assert!(
                head.starts_with(&format!("HTTP/1.1 {status}\r\n")),
                "{request:.40}: {head}"
            );
            assert!(head.contains("\r\nConnection: close"), "{head}");
            assert!(
                head.contains(&format!("\r\nContent-Length: {}", body.len())),
                "{head}"
            );
            assert_eq!(
                head.contains("\r\nAllow: GET"),
                status.starts_with("405"),
                "{head}"
            );
            if status == "200 OK" {
                assert!(
                    head.contains("\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8")
                );
                assert!(body.contains("emfor_build_info{version="), "{body}");
            }
        }
    }

    #[test]
    fn heads() {
        assert_eq!(
            parse_head(b"GET /metrics?a=b HTTP/1.1\r\nHost: x\r\n\r\n"),
            Some(("GET", "/metrics"))
        );
        for bad in [
            &b"GET /metrics HTTP/2.0\r\n\r\n"[..],
            b"GET /metrics\r\n\r\n",
            b"GET  /metrics HTTP/1.1\r\n\r\n",
            b"GET metrics HTTP/1.1\r\n\r\n",
            b"GET /metrics HTTP/1.1\r\nno colon\r\n\r\n",
            b"\xff /metrics HTTP/1.1\r\n\r\n",
        ] {
            assert_eq!(parse_head(bad), None, "{}", String::from_utf8_lossy(bad));
        }
        assert!(respond(None, || unreachable!()).starts_with(b"HTTP/1.1 400 "));
    }

    /// More than 16 connections at once: the extra one is closed without a
    /// response, and requests are answered again once the others go.
    #[tokio::test]
    async fn connection_limit() {
        let srv = server().await;
        let addr = srv.local_addrs()[0];
        let mut idle = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            idle.push(TcpStream::connect(addr).await.unwrap());
        }
        // Let the server accept them all before the extra one.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(exchange(addr, b"GET /metrics HTTP/1.1\r\n\r\n").await, "");
        drop(idle);
        for _ in 0..50 {
            let resp = exchange(addr, b"GET /metrics HTTP/1.1\r\n\r\n").await;
            if resp.starts_with("HTTP/1.1 200 OK") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("no 200 after the idle connections closed");
    }

    #[tokio::test]
    async fn bind_failure_names_the_address() {
        let srv = server().await;
        let addr = srv.local_addrs()[0];
        let err = serve(&[addr], LiveState::default()).await.err().unwrap();
        assert!(err.contains(&addr.to_string()), "{err}");
    }

    /// Spec "Metrics HTTP behavior", scenario "Not ready yet".
    #[tokio::test]
    async fn not_ready_yet() {
        let live = LiveState::default();
        let srv = serve(&["127.0.0.1:0".parse().unwrap()], live.clone())
            .await
            .unwrap();
        let addr = srv.local_addrs()[0];
        let resp = exchange(addr, b"GET /metrics HTTP/1.1\r\n\r\n").await;
        assert!(
            resp.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
            "{resp}"
        );
        assert!(!resp.contains("emfor_"), "{resp}");
        // Other requests are answered as usual.
        let resp = exchange(addr, b"GET / HTTP/1.1\r\n\r\n").await;
        assert!(resp.starts_with("HTTP/1.1 404 "), "{resp}");
        live.replace(Default::default(), None);
        let resp = exchange(addr, b"GET /metrics HTTP/1.1\r\n\r\n").await;
        assert!(resp.starts_with("HTTP/1.1 200 OK\r\n"), "{resp}");
        assert!(resp.contains("\nemfor_build_info{"), "{resp}");
    }
}
