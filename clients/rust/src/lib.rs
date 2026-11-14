//! emfor client: pick a backend for a service from the local emfor state
//! store, which the emfor daemon on the same host keeps up to date.
//!
//! ```no_run
//! match emfor_client::lookup("api.svc.emfor", Some("user-42")) {
//!     Ok(addr) => println!("connect to {addr}"), // "10.0.0.11:8080" or "[2001:db8::11]:8080"
//!     Err(e) => eprintln!("{e}"),                // e.g. "ALL_DOWN: ..."
//! }
//! ```
//!
//! Configuration comes only from the environment, read on the first lookup
//! of a process (and again after `fork`):
//! - `EMFOR_STORE`: store path, default `/var/lib/emfor/state.mdb`;
//! - `EMFOR_MAX_STALENESS`: a positive whole number with `ms` or `s`,
//!   default `3s`. An invalid value makes every lookup fail with
//!   [`ErrorCode::Unavailable`].
//!
//! Lookups are safe from many threads and after `fork`. The store is read
//! with the daemon's own LMDB build, so this crate (and `libemfor`, its C
//! library) must come from the same emfor release as the daemon.

mod client;
mod config;
mod select;
mod store;

#[cfg(feature = "capi")]
pub mod capi;

use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use client::Client;

/// Why a lookup failed. The names ([`ErrorCode::as_str`]) are shared by every
/// emfor client language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The store cannot be used: it cannot be opened, its LMDB format differs
    /// from this client's, it is not initialized, or the client's
    /// environment setting is invalid.
    Unavailable,
    /// The store's format version is neither 1 nor 2, or a value cannot be parsed.
    Incompatible,
    /// No service has that name.
    NotFound,
    /// No healthy backend, and the service's `on_all_down` is `closed`.
    AllDown,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Unavailable => "UNAVAILABLE",
            ErrorCode::Incompatible => "INCOMPATIBLE",
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::AllDown => "ALL_DOWN",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A failed lookup: its code and a message naming the cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    pub(crate) fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Error {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

/// A process's client and the pid that created it.
struct Global {
    pid: u32,
    client: Client,
}

/// The process's client: null before the first lookup, and the parent's
/// after a fork until the child's first lookup. Not a lock: a child forked
/// while another thread of the parent held it could never take it. A value
/// stored here is never freed, so a reference to it stays valid: a client
/// inherited across fork must not be closed (LMDB forbids it), and one
/// replaced by `testing::use_store` may still be in use.
static GLOBAL: AtomicPtr<Global> = AtomicPtr::new(ptr::null_mut());

/// The client for this process, created on first use and again after a fork.
fn global() -> &'static Client {
    let pid = std::process::id();
    loop {
        let current = GLOBAL.load(Ordering::Acquire);
        // SAFETY: a non-null value comes from `Box::into_raw` below (or in
        // `testing::use_store`) and is never freed.
        if let Some(g) = unsafe { current.as_ref() }
            && g.pid == pid
        {
            return &g.client;
        }
        let new = Box::into_raw(Box::new(Global {
            pid,
            client: Client::from_env(),
        }));
        match GLOBAL.compare_exchange(current, new, Ordering::AcqRel, Ordering::Acquire) {
            // The value replaced, if any, is the parent's: left alone.
            // SAFETY: just stored, never freed.
            Ok(_) => return unsafe { &(*new).client },
            Err(_) => {
                // Another thread stored its client first: use that one.
                // SAFETY: `new` was never shared.
                drop(unsafe { Box::from_raw(new) });
            }
        }
    }
}

/// Look up one backend of service `name` and return its address, `ip:port`
/// (`[ip]:port` for IPv6). `affinity_key` matters only for `sticky`
/// services: the same key gets the same backend while it is healthy.
pub fn lookup(name: &str, affinity_key: Option<&str>) -> Result<String, Error> {
    global().lookup(name, affinity_key)
}

/// Test entry point: a client with an explicit store path and staleness,
/// independent of the environment and of the process's client. Not part of
/// the public API.
#[cfg(feature = "testing")]
pub mod testing {
    use std::path::PathBuf;

    use crate::Error;

    pub struct Client(crate::client::Client);

    impl Client {
        /// `max_staleness` is parsed like `EMFOR_MAX_STALENESS`.
        pub fn new(path: impl Into<PathBuf>, max_staleness: &str) -> Self {
            Client(crate::client::Client::new(
                path.into(),
                crate::config::parse_max_staleness(max_staleness),
            ))
        }

        pub fn with_max_staleness_ms(path: impl Into<PathBuf>, ms: u64) -> Self {
            Client(crate::client::Client::new(path.into(), Ok(ms)))
        }

        pub fn lookup(&self, name: &str, affinity_key: Option<&str>) -> Result<String, Error> {
            self.0.lookup(name, affinity_key)
        }
    }

    /// Parse an `EMFOR_MAX_STALENESS` value into milliseconds.
    pub fn parse_max_staleness(s: &str) -> Result<u64, String> {
        crate::config::parse_max_staleness(s)
    }

    /// Make the process's client (used by [`crate::lookup`] and the C API)
    /// read `path` with `max_staleness` (parsed like `EMFOR_MAX_STALENESS`),
    /// replacing the one from the environment. The replaced client is never
    /// freed (a lookup may still use it).
    pub fn use_store(path: impl Into<PathBuf>, max_staleness: &str) {
        let client = crate::client::Client::new(
            path.into(),
            crate::config::parse_max_staleness(max_staleness),
        );
        let g = Box::into_raw(Box::new(crate::Global {
            pid: std::process::id(),
            client,
        }));
        crate::GLOBAL.store(g, std::sync::atomic::Ordering::Release);
    }

    /// Rendezvous hash version 1 of `key` and backend `id`.
    pub fn rendezvous_hash(key: &str, id: &str) -> u32 {
        crate::select::rendezvous_hash(key, id)
    }

    /// Rendezvous pick among `ids` (all healthy) for `key`, every weight 1.
    pub fn rendezvous_pick<'a>(key: &str, ids: &[&'a str]) -> &'a str {
        let i = crate::select::rendezvous_index(key, ids.iter().map(|id| (*id, 1)));
        ids[i]
    }

    /// Weighted rendezvous pick among `(id, weight)` (all healthy) for
    /// `key`, as `balance = "sticky"` chooses with an affinity key.
    ///
    /// # Panics
    ///
    /// If `ids` is empty.
    pub fn rendezvous_pick_weighted<'a>(key: &str, ids: &[(&'a str, u32)]) -> &'a str {
        let i = crate::select::rendezvous_index(key, ids.iter().copied());
        ids[i].0
    }
}
