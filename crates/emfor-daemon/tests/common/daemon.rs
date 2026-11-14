//! Running the `emfor` binary: a daemon in its own temporary directory,
//! its stderr collected as it comes, and one-shot commands.

use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

use super::{Locked, sockets};

/// How long [`wait_for`] and [`Daemon::wait_log`] wait.
pub const WAIT: Duration = Duration::from_secs(15);

/// Wait until `cond` holds, checking every 20 ms; panic after [`WAIT`].
#[track_caller]
pub fn wait_for(cond: impl FnMut() -> bool) {
    wait_within("condition", WAIT, cond);
}

/// Wait at most `limit` until `cond` holds; the panic names `what`.
#[track_caller]
pub fn wait_within(what: &str, limit: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{what}: not within {limit:?}");
}

/// The `[store]` table for a store in `dir`, with a blank line after it.
pub fn store_header(dir: &Path) -> String {
    format!(
        "[store]\npath = \"{}\"\nmap_size = \"4MiB\"\n\n",
        dir.join("state.mdb").display()
    )
}

fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary directory can be created")
}

/// A config function giving [`store_header`] and `body`.
fn with_store(body: &str) -> impl FnOnce(&Path) -> String + '_ {
    move |dir| format!("{}{body}", store_header(dir))
}

/// A running `emfor server`. It is killed when dropped.
pub struct Daemon {
    pub child: Child,
    /// Holds `emfor.toml` and the store, `state.mdb`.
    pub dir: tempfile::TempDir,
    /// Everything the daemon wrote to stderr so far, read as it comes so a
    /// long log never fills the pipe and blocks the daemon.
    log: Arc<Mutex<String>>,
    reader: Option<JoinHandle<()>>,
}

impl Daemon {
    /// Start a daemon whose config is [`store_header`] and `body`, without
    /// waiting for it.
    pub fn spawn(body: &str) -> Daemon {
        Daemon::spawn_in(temp_dir(), with_store(body), &[])
    }

    /// [`Daemon::spawn`], then wait until the store exists.
    pub fn start(body: &str) -> Daemon {
        Daemon::start_in(temp_dir(), body, &[])
    }

    /// Start a daemon in `dir` (which may already hold a store) with
    /// [`store_header`] and `body` and the extra environment `env`, then
    /// wait until the store exists.
    pub fn start_in(dir: tempfile::TempDir, body: &str, env: &[(&str, &Path)]) -> Daemon {
        let d = Daemon::spawn_in(dir, with_store(body), env);
        wait_for(|| std::fs::metadata(d.store()).is_ok_and(|m| m.len() > 0));
        d
    }

    /// Start a daemon in a new directory without waiting for it: `config`
    /// gets the directory and returns the whole config.
    pub fn spawn_with(config: impl FnOnce(&Path) -> String, env: &[(&str, &Path)]) -> Daemon {
        Daemon::spawn_in(temp_dir(), config, env)
    }

    /// Start a daemon in `dir` without waiting for it: `config` gets the
    /// directory and returns the whole config; `env` is added to the
    /// daemon's environment (`RUST_LOG` is removed).
    pub fn spawn_in(
        dir: tempfile::TempDir,
        config: impl FnOnce(&Path) -> String,
        env: &[(&str, &Path)],
    ) -> Daemon {
        let cfg = dir.path().join("emfor.toml");
        std::fs::write(&cfg, config(dir.path())).expect("the config can be written");
        let mut child = Command::new(cargo_bin("emfor"))
            .arg("server")
            .arg("--config")
            .arg(&cfg)
            .env_remove("RUST_LOG")
            .envs(env.iter().copied())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn_locked()
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
            log,
            reader: Some(reader),
        }
    }

    pub fn config(&self) -> PathBuf {
        self.dir.path().join("emfor.toml")
    }

    pub fn store(&self) -> PathBuf {
        self.dir.path().join("state.mdb")
    }

    /// Replace the config with [`store_header`] and `body` (the daemon
    /// reads it on SIGHUP).
    pub fn write_config(&self, body: &str) {
        self.write_whole_config(&format!("{}{body}", store_header(self.dir.path())));
    }

    /// Replace the config with `config` as it is.
    pub fn write_whole_config(&self, config: &str) {
        std::fs::write(self.config(), config).expect("the config can be written");
    }

    pub fn signal(&self, s: Signal) {
        let pid = i32::try_from(self.child.id()).expect("a pid fits an i32");
        kill(Pid::from_raw(pid), s).expect("the daemon can be signalled");
    }

    /// Everything the daemon wrote to stderr so far.
    pub fn log(&self) -> String {
        self.log.lock().expect("log lock").clone()
    }

    /// Wait until the daemon has logged `text`; the panic shows the log.
    #[track_caller]
    pub fn wait_log(&self, text: &str) {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if self.log().contains(text) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("{text}: not logged within {WAIT:?}; log:\n{}", self.log());
    }

    pub fn running(&mut self) -> bool {
        self.child
            .try_wait()
            .expect("the daemon can be polled")
            .is_none()
    }

    /// Wait for the daemon to exit and its stderr to be read; returns the
    /// exit code (-1 for a signal) and all of stderr.
    pub fn wait(&mut self) -> (i32, String) {
        let status = self.child.wait().expect("the daemon can be waited for");
        if let Some(r) = self.reader.take() {
            r.join().expect("the log reader ends");
        }
        (status.code().unwrap_or(-1), self.log())
    }

    /// SIGTERM, then [`Daemon::wait`].
    pub fn stop(mut self) -> (i32, String) {
        self.signal(Signal::SIGTERM);
        self.wait()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Run `emfor` with `args`; returns the exit code, stdout and stderr. It is
/// killed, and the test fails, if it runs longer than 10 seconds.
#[track_caller]
pub fn emfor(args: &[&str]) -> (i32, String, String) {
    let mut child = Command::new(cargo_bin("emfor"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_locked()
        .expect("the emfor binary starts");
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().expect("emfor can be polled").is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("emfor {args:?} still running after 10s");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child
        .wait_with_output()
        .expect("emfor's output can be read");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A TCP backend that accepts connections and does nothing else, and its
/// address as written in a config.
pub fn listener() -> (TcpListener, String) {
    let l = sockets(|| TcpListener::bind("127.0.0.1:0")).expect("a backend can listen");
    let a = l
        .local_addr()
        .expect("a listener has an address")
        .to_string();
    (l, a)
}

/// An HTTP backend answering every request with 200 after `delay`; its
/// address as written in a config.
pub fn http_backend(delay: Duration) -> String {
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

/// `GET /metrics` from `addr`; the whole response, head and body.
pub fn scrape(addr: SocketAddr) -> String {
    use std::io::Write;
    let mut s = sockets(|| TcpStream::connect(addr)).expect("the metrics endpoint answers");
    s.set_read_timeout(Some(Duration::from_secs(10)))
        .expect("a read timeout can be set");
    s.write_all(b"GET /metrics HTTP/1.1\r\nHost: test\r\n\r\n")
        .expect("the request can be sent");
    let mut out = String::new();
    s.read_to_string(&mut out)
        .expect("the response can be read");
    out
}
