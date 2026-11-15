//! A fork from a signal handler that interrupted a lookup of the same thread
//! returns: the handlers must not ask for the store lock that thread holds
//! (src/fork.rs `InStore`). The handler calls the prepare and release
//! handlers directly, so it covers every platform.
#![cfg(feature = "testing")]
mod support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use emfor_client::lookup;
use emfor_client::testing::{fork_prepare, fork_release};

static FORKS: AtomicU64 = AtomicU64::new(0);

/// What the registered handlers of a fork run, without the fork: a real
/// `fork` here can deadlock on the allocator's own fork locks when the
/// signal interrupted `malloc`, whatever the client does. A test of what
/// happens when an application forks from a signal handler anyway.
extern "C" fn on_alarm(_: libc::c_int) {
    fork_prepare();
    fork_release();
    FORKS.fetch_add(1, Ordering::Relaxed);
}

#[test]
fn fork_handlers_from_a_signal_handler_return() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::write_store(&path);
    support::set_env(&path, Some("60s"));
    assert!(support::API.contains(&lookup("api.svc.emfor", None).unwrap().as_str()));

    // A hang aborts the test run instead of hanging it.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(30));
        eprintln!("the fork handlers of a signal handler did not return within 30 seconds");
        std::process::abort();
    });

    // SAFETY: installing a handler for this process; `pthread_self` is this
    // (the main) thread, which the ticker thread signals.
    let main_thread = unsafe { libc::pthread_self() } as usize;
    unsafe { libc::signal(libc::SIGALRM, on_alarm as *const () as usize) };
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(2));
            // SAFETY: the main thread outlives the test.
            unsafe { libc::pthread_kill(main_thread as libc::pthread_t, libc::SIGALRM) };
        }
    });

    let end = Instant::now() + Duration::from_secs(3);
    let mut lookups = 0u64;
    while Instant::now() < end {
        assert!(support::API.contains(&lookup("api.svc.emfor", None).unwrap().as_str()));
        lookups += 1;
    }
    let forks = FORKS.load(Ordering::Relaxed);
    assert!(forks > 0, "no signal handler ran");
    eprintln!("{forks} prepare and release pairs from the signal handler, {lookups} lookups");
}
