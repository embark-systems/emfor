//! The fork handlers return when they run on a thread whose thread-local
//! storage is being destroyed: a `fork` from a thread-local destructor, or a
//! signal that reaches a thread that is exiting. They are `extern "C"`
//! functions called by the threading library, so a panic in them aborts the
//! process (src/fork.rs `HELD`, the store lock the prepare handler keeps in a
//! thread-local). That fork is not guarded, but it returns. Its own binary:
//! the process's client can be created only once.
#![cfg(feature = "testing")]
mod support;

use std::sync::atomic::{AtomicU32, Ordering};

use emfor_client::lookup;
use emfor_client::testing::{fork_prepare, fork_release};

/// How the destructor ended: 1 the handlers returned, 2 they panicked.
static OUTCOME: AtomicU32 = AtomicU32::new(0);

/// A thread-local whose destructor runs the fork handlers. Thread-local
/// destructors run in the reverse order of the first use of their values, so
/// the thread touches this one first: the store lock's `HELD` is registered
/// after it and is already destroyed when this destructor runs.
struct ForkOnDrop;

impl Drop for ForkOnDrop {
    fn drop(&mut self) {
        let outcome = std::panic::catch_unwind(|| {
            fork_prepare();
            fork_release();
        });
        OUTCOME.store(if outcome.is_ok() { 1 } else { 2 }, Ordering::SeqCst);
    }
}

thread_local! {
    static FORK_ON_DROP: ForkOnDrop = const { ForkOnDrop };
}

#[test]
fn fork_handlers_return_in_a_thread_local_destructor() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::write_store(&path);
    support::set_env(&path, Some("60s"));
    // The process's client: without it the prepare handler has no lock to
    // take, and does not touch `HELD`.
    assert!(support::API.contains(&lookup("api.svc.emfor", None).unwrap().as_str()));

    std::thread::spawn(|| {
        FORK_ON_DROP.with(|_| {});
        // The first use of `HELD` in this thread.
        fork_prepare();
        fork_release();
    })
    .join()
    .unwrap();

    assert_eq!(
        OUTCOME.load(Ordering::SeqCst),
        1,
        "the fork handlers panicked in a thread-local destructor (0: the destructor did not run)"
    );
    // The process still looks up: the handlers left no lock or flag behind.
    assert!(support::API.contains(&lookup("api.svc.emfor", None).unwrap().as_str()));
}
