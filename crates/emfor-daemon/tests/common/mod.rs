//! Helpers shared by the daemon integration tests: free loopback ports and
//! starting processes without leaking the tests' sockets into them.
//!
//! Each test file uses only some of these.
#![allow(dead_code)]

use std::io;
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// macOS has no `SOCK_CLOEXEC`: Rust creates a socket and only then marks
/// it close-on-exec, so a process another test thread starts between the
/// two steps inherits the socket. An inherited port probe keeps the port
/// bound in that process, and the daemon meant to listen there exits with
/// "Address already in use"; an inherited backend listener keeps the backend
/// up after the test drops it. Sockets (and other descriptors) are therefore
/// created under the read lock and processes started under the write lock.
static SPAWN: RwLock<()> = RwLock::new(());

/// Run `f`, which creates sockets or other descriptors, while no process is
/// being started.
pub fn sockets<T>(f: impl FnOnce() -> T) -> T {
    let _guard = SPAWN.read().unwrap_or_else(|e| e.into_inner());
    f()
}

/// `spawn` and `output` that wait until no socket is being created.
pub trait Locked {
    fn spawn_locked(&mut self) -> io::Result<Child>;
    /// Like [`Command::output`]: stdin is null, stdout and stderr are
    /// captured.
    fn output_locked(&mut self) -> io::Result<Output>;
}

impl Locked for Command {
    fn spawn_locked(&mut self) -> io::Result<Child> {
        let _guard = SPAWN.write().unwrap_or_else(|e| e.into_inner());
        self.spawn()
    }

    fn output_locked(&mut self) -> io::Result<Output> {
        self.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn_locked()?
            .wait_with_output()
    }
}

/// A loopback port for a daemon to listen on, from 20000 to 31999: below
/// the ephemeral ranges of macOS (49152 and up) and Linux (32768 and up), so
/// that no outgoing connection is given it between this probe and the
/// daemon's bind. Test processes running at the same time claim each port
/// with a lock on `<temp dir>/emfor-test-ports/<port>`, held until the
/// process exits, so two of them never hand out the same port.
fn free_port_where(bindable: impl Fn(u16) -> bool) -> u16 {
    const FIRST: u32 = 20_000;
    const COUNT: u32 = 12_000;
    static NEXT: AtomicU32 = AtomicU32::new(u32::MAX);
    static HELD: Mutex<Vec<std::fs::File>> = Mutex::new(Vec::new());
    if NEXT.load(Ordering::Relaxed) == u32::MAX {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let start = (std::process::id().wrapping_mul(7919) ^ nanos) % COUNT;
        let _ = NEXT.compare_exchange(u32::MAX, start, Ordering::Relaxed, Ordering::Relaxed);
    }
    let locks = std::env::temp_dir().join("emfor-test-ports");
    std::fs::create_dir_all(&locks).expect("the port lock directory can be created");
    for _ in 0..COUNT {
        let n = NEXT.fetch_add(1, Ordering::Relaxed) % COUNT;
        let port = u16::try_from(FIRST + n).expect("FIRST + COUNT is below 65536");
        let file = sockets(|| {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(locks.join(port.to_string()))
        })
        .expect("a port lock file can be opened");
        if file.try_lock().is_err() {
            continue;
        }
        if sockets(|| bindable(port)) {
            HELD.lock().unwrap_or_else(|e| e.into_inner()).push(file);
            return port;
        }
    }
    panic!(
        "no loopback port from {FIRST} to {} is free",
        FIRST + COUNT - 1
    );
}

/// A loopback address free for TCP.
pub fn free_tcp() -> SocketAddr {
    let port = free_port_where(|p| TcpListener::bind(("127.0.0.1", p)).is_ok());
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// A loopback address free for UDP.
pub fn free_udp() -> SocketAddr {
    let port = free_port_where(|p| UdpSocket::bind(("127.0.0.1", p)).is_ok());
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// A loopback port free for both UDP and TCP.
pub fn free_port() -> u16 {
    free_port_where(|p| {
        UdpSocket::bind(("127.0.0.1", p)).is_ok() && TcpListener::bind(("127.0.0.1", p)).is_ok()
    })
}
