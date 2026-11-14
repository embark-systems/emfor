//! [`PluginHost`]: runs one plugin process, routes answers to requests by
//! id, logs its stderr, and restarts it with backoff when it exits or stops
//! answering.

use std::collections::HashMap;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::protocol::{PROTOCOL, Request, Response};
use crate::live::PluginStatus;

/// How long a plugin has to answer the handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a stopping plugin has to exit after its stdin is closed.
pub const STOP_GRACE: Duration = Duration::from_secs(2);
/// First restart delay; doubles per consecutive failure.
const BACKOFF_START: Duration = Duration::from_secs(1);
/// Longest restart delay.
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// Uptime after which the restart delay goes back to [`BACKOFF_START`].
const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(30);
/// Longest line read from a plugin's stdout or stderr, in bytes; no more
/// than this is buffered for one line.
pub const LINE_LIMIT: usize = 65536;
/// Longest stderr line logged, in characters.
const STDERR_LINE_CHARS: usize = 1000;
/// How long to keep routing answers the plugin wrote just before it exited.
const EXIT_DRAIN: Duration = Duration::from_millis(100);
/// Requests in a row whose callers gave up waiting, with no answer to any
/// request in between, after which a running plugin counts as stuck and is
/// restarted.
pub const STALL_UNANSWERED: u32 = 3;

/// Delay before the next start after `failures` consecutive failures
/// (1 → 1s, 2 → 2s, 3 → 4s, … capped at 30s).
pub fn restart_delay(failures: u32) -> Duration {
    let exp = failures.saturating_sub(1).min(16);
    (BACKOFF_START * 2u32.pow(exp)).min(BACKOFF_MAX)
}

/// Counts consecutive failures of one plugin.
#[derive(Debug, Default)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    /// Record an exit after `uptime` (zero for a failed start) and return
    /// the delay before the next start.
    pub fn on_exit(&mut self, uptime: Duration) -> Duration {
        if uptime >= BACKOFF_RESET_AFTER {
            self.failures = 0;
        }
        self.failures += 1;
        restart_delay(self.failures)
    }
}

/// Why a request got no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallError {
    /// The plugin is not running (crashed and not yet restarted, or stopped).
    Unavailable,
    /// The plugin exited while the request was pending.
    Exited,
}

/// Requests waiting for an answer. `closed` is set when the plugin closes
/// stdout, so no request is accepted that could never be answered.
/// `unanswered` counts requests in a row whose callers gave up waiting; any
/// answer resets it, and reaching [`STALL_UNANSWERED`] wakes `stalled`.
#[derive(Default)]
struct Waiters {
    closed: bool,
    map: HashMap<u64, oneshot::Sender<Response>>,
    unanswered: u32,
    stalled: Arc<Notify>,
}

impl Waiters {
    /// Close and drop every waiter; they see [`CallError::Exited`].
    fn close(&mut self) {
        self.closed = true;
        self.map.clear();
    }
}

type Pending = Arc<Mutex<Waiters>>;

fn lock(p: &Pending) -> std::sync::MutexGuard<'_, Waiters> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

/// The running process's request side.
#[derive(Clone)]
struct Conn {
    tx: mpsc::UnboundedSender<String>,
    pending: Pending,
    pid: u32,
}

struct Shared {
    name: String,
    command: Vec<String>,
    conn: Mutex<Option<Conn>>,
    next_id: AtomicU64,
    status: Arc<PluginStatus>,
}

impl Shared {
    fn set_conn(&self, conn: Option<Conn>) {
        self.status.up.store(conn.is_some(), Ordering::Relaxed);
        *self.conn.lock().unwrap_or_else(|e| e.into_inner()) = conn;
    }
}

/// One plugin process and its supervisor.
pub struct PluginHost {
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    supervisor: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for PluginHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginHost")
            .field("name", &self.shared.name)
            .field("pid", &self.pid())
            .finish()
    }
}

/// A started process before it is handed to the supervisor.
struct Process {
    child: Child,
    conn: Conn,
    /// Finishes when the plugin closes stdout.
    router: JoinHandle<()>,
}

impl PluginHost {
    /// Start the plugin and complete the handshake. The error names the
    /// plugin and says what went wrong.
    pub async fn start(name: &str, command: &[String]) -> Result<PluginHost, String> {
        let process = launch(name, command).await?;
        let shared = Arc::new(Shared {
            name: name.to_string(),
            command: command.to_vec(),
            conn: Mutex::new(Some(process.conn.clone())),
            next_id: AtomicU64::new(1),
            status: Arc::new(PluginStatus {
                up: true.into(),
                restarts: 0.into(),
            }),
        });
        let (stop, stop_rx) = watch::channel(false);
        let supervisor = tokio::spawn(supervise(shared.clone(), process, stop_rx));
        Ok(PluginHost {
            shared,
            stop,
            supervisor: Mutex::new(Some(supervisor)),
        })
    }

    pub fn name(&self) -> &str {
        &self.shared.name
    }

    pub fn command(&self) -> &[String] {
        &self.shared.command
    }

    /// Up and restart count, for `/metrics`.
    pub fn status(&self) -> Arc<PluginStatus> {
        self.shared.status.clone()
    }

    /// Process id of the running plugin, `None` while it is down.
    pub fn pid(&self) -> Option<u32> {
        self.shared
            .conn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|c| c.pid)
    }

    /// Send a request built from a fresh id and wait for its answer. The
    /// caller enforces any timeout; dropping the future forgets the request.
    pub async fn call(&self, build: impl FnOnce(u64) -> Request) -> Result<Response, CallError> {
        let (tx, rx) = oneshot::channel();
        let guard = {
            let conn = self.shared.conn.lock().unwrap_or_else(|e| e.into_inner());
            let Some(conn) = conn.as_ref() else {
                return Err(CallError::Unavailable);
            };
            let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
            let line = serde_json::to_string(&build(id)).expect(
                "a Request serializes: it holds only strings, numbers and string-keyed maps",
            ) + "\n";
            {
                let mut waiters = lock(&conn.pending);
                if waiters.closed {
                    return Err(CallError::Unavailable);
                }
                waiters.map.insert(id, tx);
            }
            let guard = PendingGuard {
                pending: Some(conn.pending.clone()),
                id,
            };
            if conn.tx.send(line).is_err() {
                guard.done();
                return Err(CallError::Unavailable);
            }
            guard
        };
        let answer = rx.await.map_err(|_| CallError::Exited);
        guard.done();
        answer
    }

    /// Close the plugin's stdin, wait up to [`STOP_GRACE`] for it to exit,
    /// then kill it. Stops restarts too. Idempotent.
    pub async fn stop(&self) {
        let _ = self.stop.send(true);
        let handle = self
            .supervisor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(h) = handle {
            let _ = h.await;
        }
    }
}

/// Removes a request from the pending map when its caller stops waiting,
/// and counts it as unanswered when it was still pending then.
struct PendingGuard {
    pending: Option<Pending>,
    id: u64,
}

impl PendingGuard {
    /// The call finished (answered, or the plugin exited): nothing to count.
    fn done(mut self) {
        if let Some(p) = self.pending.take() {
            lock(&p).map.remove(&self.id);
        }
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let Some(p) = self.pending.take() else {
            return;
        };
        let mut waiters = lock(&p);
        if waiters.map.remove(&self.id).is_some() && !waiters.closed {
            waiters.unanswered += 1;
            if waiters.unanswered == STALL_UNANSWERED {
                waiters.stalled.notify_one();
            }
        }
    }
}

fn cut(s: &str, chars: usize) -> String {
    s.chars().take(chars).collect()
}

/// Spawn the process, wire its pipes, and complete the handshake.
async fn launch(name: &str, command: &[String]) -> Result<Process, String> {
    let program = command
        .first()
        .ok_or_else(|| format!("plugin {name}: empty command"))?;
    let mut cmd = Command::new(program);
    cmd.args(&command[1..])
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::child::inherit_stdio_only(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("plugin {name}: cannot run {program}: {e}"))?;
    let pid = child.id().unwrap_or(0);
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(format!(
            "plugin {name}: cannot connect to its stdin, stdout and stderr"
        ));
    };

    let log_name = name.to_string();
    tokio::spawn(async move {
        let mut stderr = BufReader::new(stderr);
        loop {
            let line = match read_line_limited(&mut stderr, LINE_LIMIT).await {
                Ok(LineRead::Line(line)) => line,
                Ok(LineRead::TooLong(start)) => {
                    if skip_line(&mut stderr).await.is_err() {
                        break;
                    }
                    start
                }
                Ok(LineRead::Eof) | Err(_) => break,
            };
            tracing::info!(plugin = %log_name, "plugin {}: {}", log_name, cut(&line, STDERR_LINE_CHARS));
        }
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
        // Dropping stdin here is the plugin's end of file.
    });

    let hello = serde_json::to_string(&Request::Hello { protocol: PROTOCOL })
        .expect("a Request serializes: it holds only strings, numbers and string-keyed maps")
        + "\n";
    let _ = tx.send(hello);
    let mut stdout = BufReader::new(stdout);
    let first = match tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        read_line_limited(&mut stdout, LINE_LIMIT),
    )
    .await
    {
        Err(_) => {
            return Err(format!(
                "plugin {name} did not answer the handshake within {}s",
                HANDSHAKE_TIMEOUT.as_secs()
            ));
        }
        Ok(Ok(LineRead::Line(line))) => line,
        Ok(Ok(LineRead::TooLong(_))) => {
            return Err(format!(
                "plugin {name} answered the handshake with a line longer than {LINE_LIMIT} bytes"
            ));
        }
        Ok(_) => {
            let status = match tokio::time::timeout(Duration::from_secs(1), child.wait()).await {
                Ok(Ok(s)) => format!(" ({s})"),
                _ => String::new(),
            };
            return Err(format!("plugin {name} exited before the handshake{status}"));
        }
    };
    let (plugin_name, version) = match serde_json::from_str::<Response>(&first) {
        Ok(Response::Hello {
            protocol,
            name: n,
            version: v,
        }) => {
            if protocol != PROTOCOL {
                return Err(format!(
                    "plugin {name} speaks protocol {protocol}, emfor speaks protocol {PROTOCOL}"
                ));
            }
            (n, v)
        }
        _ => {
            return Err(format!(
                "plugin {name} answered the handshake with {:?}",
                cut(&first, 200)
            ));
        }
    };
    tracing::info!(
        plugin = %name,
        "plugin {name} started (pid {pid}, {plugin_name} {version})"
    );

    let pending: Pending = Arc::default();
    let router_pending = pending.clone();
    let router_name = name.to_string();
    let router = tokio::spawn(async move {
        loop {
            match read_line_limited(&mut stdout, LINE_LIMIT).await {
                Ok(LineRead::Line(line)) => route(&router_name, &router_pending, &line),
                Ok(LineRead::TooLong(_)) => {
                    // Stop reading: the supervisor sees stdout end, stops the
                    // plugin and restarts it.
                    tracing::warn!(
                        plugin = %router_name,
                        "plugin {router_name} wrote a line longer than {LINE_LIMIT} bytes"
                    );
                    break;
                }
                Ok(LineRead::Eof) | Err(_) => break,
            }
        }
        lock(&router_pending).close();
    });
    Ok(Process {
        child,
        conn: Conn { tx, pending, pid },
        router,
    })
}

/// One line read by [`read_line_limited`].
#[derive(Debug, PartialEq, Eq)]
enum LineRead {
    /// A line without its line feed (and carriage return), or the last
    /// bytes before end of file.
    Line(String),
    /// The line went on past the limit: its first `limit` bytes. The rest of
    /// the line is still unread.
    TooLong(String),
    Eof,
}

/// Read one line, buffering at most `limit` bytes of it.
async fn read_line_limited<R: AsyncBufRead + Unpin>(
    r: &mut R,
    limit: usize,
) -> std::io::Result<LineRead> {
    let text = |mut b: Vec<u8>| {
        if b.last() == Some(&b'\r') {
            b.pop();
        }
        String::from_utf8_lossy(&b).into_owned()
    };
    let mut buf = Vec::new();
    loop {
        let avail = r.fill_buf().await?;
        if avail.is_empty() {
            return Ok(if buf.is_empty() {
                LineRead::Eof
            } else {
                LineRead::Line(text(buf))
            });
        }
        let end = avail.iter().position(|&b| b == b'\n');
        let chunk = &avail[..end.unwrap_or(avail.len())];
        let room = limit - buf.len();
        if chunk.len() > room {
            buf.extend_from_slice(&chunk[..room]);
            r.consume(room);
            return Ok(LineRead::TooLong(
                String::from_utf8_lossy(&buf).into_owned(),
            ));
        }
        buf.extend_from_slice(chunk);
        match end {
            Some(i) => {
                r.consume(i + 1);
                return Ok(LineRead::Line(text(buf)));
            }
            None => {
                let n = chunk.len();
                r.consume(n);
            }
        }
    }
}

/// Discard input up to and including the next line feed.
async fn skip_line<R: AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<()> {
    loop {
        let avail = r.fill_buf().await?;
        if avail.is_empty() {
            return Ok(());
        }
        match avail.iter().position(|&b| b == b'\n') {
            Some(i) => {
                r.consume(i + 1);
                return Ok(());
            }
            None => {
                let n = avail.len();
                r.consume(n);
            }
        }
    }
}

/// Fuzzing entry point (`fuzz/fuzz_targets/plugin_line.rs`): handle `line`
/// as a line the plugin wrote to stdout, with a request waiting for id 1.
#[cfg(fuzzing)]
pub fn fuzz_line(line: &str) {
    let pending: Pending = Arc::default();
    let (tx, _rx) = oneshot::channel();
    lock(&pending).map.insert(1, tx);
    route("fuzz", &pending, line);
}

/// Deliver one stdout line to the request waiting for it.
fn route(name: &str, pending: &Pending, line: &str) {
    let answer = match serde_json::from_str::<Response>(line) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(plugin = %name, "plugin {name}: ignoring invalid line {:?}: {e}", cut(line, 200));
            return;
        }
    };
    let Some(id) = answer.id() else {
        tracing::debug!(plugin = %name, "plugin {name}: ignoring unexpected hello");
        return;
    };
    let waiter = {
        let mut waiters = lock(pending);
        // Any answer, even to a request nobody waits for any more, shows the
        // plugin is still working.
        waiters.unanswered = 0;
        waiters.map.remove(&id)
    };
    match waiter {
        Some(w) => {
            let _ = w.send(answer);
        }
        None => {
            tracing::debug!(plugin = %name, "plugin {name}: ignoring answer to id {id}, nobody is waiting")
        }
    }
}

enum Event {
    Exited(std::io::Result<ExitStatus>),
    StdoutClosed,
    Stalled,
    Stop,
}

/// Watch the process; restart it with backoff when it exits, closes stdout
/// or stops answering ([`STALL_UNANSWERED`]); stop it when asked.
async fn supervise(shared: Arc<Shared>, mut process: Process, mut stop: watch::Receiver<bool>) {
    let name = shared.name.clone();
    let mut backoff = Backoff::default();
    loop {
        let started = Instant::now();
        let stalled = lock(&process.conn.pending).stalled.clone();
        let event = tokio::select! {
            s = process.child.wait() => Event::Exited(s),
            _ = &mut process.router => Event::StdoutClosed,
            _ = stalled.notified() => Event::Stalled,
            _ = stop.changed() => Event::Stop,
        };
        shared.set_conn(None);
        match event {
            Event::Stop => {
                drop(process.conn); // closes stdin once the writer drains
                if tokio::time::timeout(STOP_GRACE, process.child.wait())
                    .await
                    .is_err()
                {
                    let _ = process.child.kill().await;
                }
                return;
            }
            Event::Stalled => {
                tracing::warn!(
                    plugin = %name,
                    "plugin {name} stopped answering: {STALL_UNANSWERED} requests in a row got no answer"
                );
                let _ = process.child.start_kill();
                let status = process.child.wait().await;
                log_exit(&name, status);
            }
            Event::StdoutClosed => {
                let _ = process.child.start_kill();
                let status = process.child.wait().await;
                log_exit(&name, status);
            }
            Event::Exited(status) => {
                // Answers written right before exiting may still be in the
                // pipe; let the router deliver them before failing the rest.
                let _ = tokio::time::timeout(EXIT_DRAIN, &mut process.router).await;
                log_exit(&name, status);
            }
        }
        lock(&process.conn.pending).close();
        let mut delay = backoff.on_exit(started.elapsed());
        process = loop {
            tracing::warn!(plugin = %name, "plugin {name} restarting in {}s", delay.as_secs());
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = stop.changed() => return,
            }
            match launch(&name, &shared.command).await {
                Ok(p) => {
                    shared.status.restarts.fetch_add(1, Ordering::Relaxed);
                    break p;
                }
                Err(e) => {
                    tracing::warn!(plugin = %name, "{e}");
                    delay = backoff.on_exit(Duration::ZERO);
                }
            }
        };
        if *stop.borrow() {
            // Asked to stop while starting: stop the new process too.
            drop(process.conn);
            let _ = tokio::time::timeout(STOP_GRACE, process.child.wait()).await;
            return;
        }
        shared.set_conn(Some(process.conn.clone()));
    }
}

fn log_exit(name: &str, status: std::io::Result<ExitStatus>) {
    match status {
        Ok(s) => tracing::warn!(plugin = %name, "plugin {name} exited ({s})"),
        Err(e) => tracing::warn!(plugin = %name, "plugin {name} exited (cannot read status: {e})"),
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn lines_are_read_up_to_the_limit() {
        use super::{LineRead, read_line_limited, skip_line};
        let input = [
            b"short\r\n".to_vec(),
            [vec![b'a'; 8], b"\n".to_vec()].concat(),
            [vec![b'b'; 9], b"\nnext\n".to_vec()].concat(),
            b"tail".to_vec(),
        ]
        .concat();
        // A tiny buffer, so lines arrive in several pieces.
        let mut r = tokio::io::BufReader::with_capacity(3, &input[..]);
        let mut next = async || read_line_limited(&mut r, 8).await.unwrap();
        assert_eq!(next().await, LineRead::Line("short".into()));
        assert_eq!(
            next().await,
            LineRead::Line("a".repeat(8)),
            "exactly the limit"
        );
        assert_eq!(
            next().await,
            LineRead::TooLong("b".repeat(8)),
            "one byte over"
        );
        skip_line(&mut r).await.unwrap();
        let mut next = async || read_line_limited(&mut r, 8).await.unwrap();
        assert_eq!(next().await, LineRead::Line("next".into()));
        assert_eq!(
            next().await,
            LineRead::Line("tail".into()),
            "no line feed at the end"
        );
        assert_eq!(next().await, LineRead::Eof);
    }

    use super::*;

    #[test]
    fn restart_delays() {
        assert_eq!(restart_delay(1), Duration::from_secs(1));
        assert_eq!(restart_delay(2), Duration::from_secs(2));
        assert_eq!(restart_delay(3), Duration::from_secs(4));
        assert_eq!(restart_delay(6), Duration::from_secs(30));
        assert_eq!(restart_delay(1000), Duration::from_secs(30));
    }

    #[test]
    fn backoff_resets_after_long_uptime() {
        let mut b = Backoff::default();
        assert_eq!(b.on_exit(Duration::ZERO), Duration::from_secs(1));
        assert_eq!(b.on_exit(Duration::from_secs(1)), Duration::from_secs(2));
        assert_eq!(b.on_exit(Duration::ZERO), Duration::from_secs(4));
        assert_eq!(b.on_exit(Duration::from_secs(29)), Duration::from_secs(8));
        assert_eq!(b.on_exit(Duration::from_secs(30)), Duration::from_secs(1));
    }
}
