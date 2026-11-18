//! `fork` while other threads look up (spec client-lookup "Clients are safe
//! across threads and fork").
//!
//! LMDB keeps its reader table in the lock file, behind process-shared
//! mutexes. FreeBSD's threading library looks such a mutex up in a table
//! behind a lock of its own, and its `fork` only takes that lock for
//! reading: a thread that is inside an LMDB mutex call when another thread
//! forks leaves its read hold in the child, where nothing releases it, and
//! the child's first use of a mutex of a newly mapped lock file waits for
//! it for good. Next to it, a second problem of the same library: its
//! `fork` takes the allocator's locks before that lock, while a thread that
//! maps a mutex for the first time calls `malloc` holding it for writing, so
//! a parent that forks at that moment can deadlock itself. The lock is
//! taken with `URWLOCK_PREFER_READER`, so waiting on it from the outside
//! does not work either: the threads have to be kept out of LMDB before
//! the library's own `fork` code runs, which the prepare handler (it runs
//! first) does.
//!
//! So a fork must not happen while a thread of the process is inside the
//! store. Every call into LMDB runs under the write or read lock of
//! `Client::env`; the prepare handler takes the write lock of the process's
//! client, which waits for the lookups in progress and keeps new ones out
//! until the parent's and the child's handlers drop it again. All three
//! handlers run on the thread that calls `fork`, so the guard waits in a
//! thread-local between them.
//!
//! A process whose first lookup has not stored its client yet, or has not
//! finished opening the store, has no lock to take: `FORKING` covers it. It
//! counts the forks being prepared (two threads can fork at once); the
//! prepare handler raises it before it reads the client, and a thread waits
//! for it to be zero ([`wait_while_forking`]) before it takes the write lock
//! to open the store, so that it never waits for it while holding the lock
//! the prepare handler waits for. The parent handler lowers it by one; the
//! child handler sets it to zero, since the other threads' forks do not
//! exist in the child and would never lower it.
//!
//! A `fork` from a signal handler that interrupted a lookup of the same
//! thread would ask for the write lock that thread holds for reading:
//! [`InStore`] marks the threads inside the store, and the prepare handler
//! leaves the lock alone then (that fork is not guarded, but it returns).
//!
//! The handlers are registered only on FreeBSD: Linux does not hang, and a
//! handler there would be risk without benefit. The registration itself
//! ([`register_with`]) is per process, so that a `fork` during it leaves
//! the child registering instead of waiting for a thread it does not have.

// Only the FreeBSD registration and the `testing` hooks call the handlers.
#![cfg_attr(not(any(target_os = "freebsd", feature = "testing")), allow(dead_code))]

use std::cell::{Cell, RefCell};
use std::sync::RwLockWriteGuard;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::store::Env;

/// Forks in this process whose prepare handler has run and whose parent
/// handler has not.
static FORKING: AtomicUsize = AtomicUsize::new(0);

type StoreGuard = RwLockWriteGuard<'static, Option<Env>>;

thread_local! {
    /// The store lock taken by this thread's prepare handler.
    static HELD: RefCell<Option<StoreGuard>> = const { RefCell::new(None) };
    /// This thread's prepare handler has run and its release has not.
    static PREPARED: Cell<bool> = const { Cell::new(false) };
    /// How many store locks this thread is taking, holding or has just
    /// dropped (see [`InStore`]).
    static IN_STORE: Cell<u32> = const { Cell::new(0) };
}

/// Marks the calling thread as inside the store, from before it takes a
/// lock of `Client::env` until after it drops it. A `fork` from a signal
/// handler that interrupted this thread would otherwise ask for the write
/// lock this thread holds for reading, and wait for itself: the prepare
/// handler sees the mark and leaves the lock alone (that fork is not
/// protected, as before the handlers existed, but it returns). Per thread,
/// so the lookup path gains no shared variable.
pub(crate) struct InStore;

impl InStore {
    pub(crate) fn enter() -> InStore {
        IN_STORE.with(|c| c.set(c.get() + 1));
        InStore
    }
}

impl Drop for InStore {
    fn drop(&mut self) {
        IN_STORE.with(|c| c.set(c.get() - 1));
    }
}

/// Wait while a fork is being prepared. Called before taking the write lock
/// that opens the store, never while holding it. Only a thread on the slow
/// path (the store is opened or replaced) waits, and only for the time a
/// fork takes, so spinning with `yield_now` is cheaper than a wake-up
/// mechanism on the lookup path.
pub(crate) fn wait_while_forking() {
    while FORKING.load(Ordering::SeqCst) != 0 {
        std::thread::yield_now();
    }
}

/// `pthread_atfork` prepare handler: no thread of this process is inside
/// the store once it returns.
pub(crate) extern "C" fn prepare() {
    if PREPARED.replace(true) {
        return; // a release is missing: never take the lock twice
    }
    FORKING.fetch_add(1, Ordering::SeqCst);
    if IN_STORE.with(|c| c.get()) > 0 {
        return; // forked from a signal handler inside a lookup: no lock to take
    }
    // A `fork` from a thread-local destructor, or a signal handler on a thread
    // that is exiting, can find `HELD` destroyed: there is nowhere to keep the
    // lock, and `with` would panic, which in this `extern "C"` function aborts
    // the process. That fork is not guarded (as before the handlers existed),
    // but it returns.
    if HELD.try_with(|_| ()).is_err() {
        return;
    }
    let global = crate::GLOBAL.load(Ordering::SeqCst);
    // SAFETY: a non-null value comes from `Box::into_raw` and is never freed,
    // so the reference is valid for the rest of the process.
    if let Some(g) = unsafe { global.as_ref() } {
        let guard = g.client.lock_store();
        // `HELD` was alive a moment ago and this thread cannot destroy it in
        // between; were it not, the guard is dropped with the closure.
        let _ = HELD.try_with(|h| *h.borrow_mut() = Some(guard));
    }
}

/// Drop the store lock of this thread's prepare handler and lower
/// `FORKING`: by one in the parent, to zero in the child.
fn finish(child: bool) {
    if !PREPARED.replace(false) {
        return;
    }
    // `None` when `HELD` is destroyed: `prepare` kept no lock then.
    let guard = HELD.try_with(|h| h.borrow_mut().take()).ok().flatten();
    drop(guard);
    if child {
        FORKING.store(0, Ordering::SeqCst);
    } else {
        FORKING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// `pthread_atfork` parent handler: let lookups into the store again.
pub(crate) extern "C" fn release() {
    finish(false);
}

/// `pthread_atfork` child handler: the only thread of the child is the one
/// that forked.
pub(crate) extern "C" fn release_child() {
    // Handlers that run in this process are registered in it, whatever the
    // parent's registration was doing.
    REG.store(reg_state(std::process::id(), REGISTERED), Ordering::Release);
    finish(true);
}

/// Where the registration of the handlers stands, as
/// `pid << 2 | phase`: the pid of the process that set it, so that a child
/// that inherits the state of a parent that was in the middle of registering
/// does not wait for a thread it does not have.
static REG: AtomicU64 = AtomicU64::new(0);
const REGISTERING: u64 = 1;
const REGISTERED: u64 = 2;

fn reg_state(pid: u32, phase: u64) -> u64 {
    (u64::from(pid) << 2) | phase
}

/// Ends a registration attempt when dropped: "registered" when it
/// succeeded, "none" when it failed or panicked, so that a later lookup
/// tries again, and the threads waiting for it never wait for ever.
struct Attempt {
    pid: u32,
    done: bool,
}

impl Drop for Attempt {
    fn drop(&mut self) {
        let phase = if self.done { REGISTERED } else { 0 };
        REG.store(reg_state(self.pid, phase), Ordering::Release);
    }
}

/// Call `register` once in this process; it returns whether it registered
/// the handlers. A failed attempt is not recorded: the next lookup tries
/// again (a lookup does not fail for it: without the handlers it is as
/// before them). A thread that finds another thread of this process
/// registering waits for it. `fork` can run while a thread registers
/// (`pthread_atfork` waits for the fork in progress): the child then sees a
/// registration of another pid, which no thread of it is doing, and
/// registers itself.
pub(crate) fn register_with(register: impl FnOnce() -> bool) {
    let pid = std::process::id();
    let mut register = Some(register);
    loop {
        let current = REG.load(Ordering::Acquire);
        if current >> 2 == u64::from(pid) {
            match current & 3 {
                REGISTERED => return,
                REGISTERING => {
                    std::thread::yield_now(); // another thread of this process is registering
                    continue;
                }
                _ => {} // an earlier attempt failed: try again
            }
        }
        if REG
            .compare_exchange(
                current,
                reg_state(pid, REGISTERING),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            let mut attempt = Attempt { pid, done: false };
            if let Some(register) = register.take() {
                attempt.done = register();
            }
            return;
        }
    }
}

/// For the tests: the registration state a child inherits from a process
/// with pid `pid` (`phase` 1 registering, 2 registered, 0 none).
#[cfg(feature = "testing")]
pub(crate) fn set_registration(pid: u32, phase: u64) {
    REG.store(reg_state(pid, phase), Ordering::SeqCst);
}

/// Register the handlers, once, where the threading library needs them.
#[cfg(target_os = "freebsd")]
pub(crate) fn register() {
    register_with(|| {
        // SAFETY: the handlers are `extern "C"` functions of this crate and
        // `pthread_atfork` only stores their addresses. It fails only when
        // out of memory (ENOMEM).
        unsafe { libc::pthread_atfork(Some(prepare), Some(release), Some(release_child)) == 0 }
    });
}

#[cfg(not(target_os = "freebsd"))]
pub(crate) fn register() {}
