//! The registration of the fork handlers (src/fork.rs `register_with`):
//! once per process, a thread that finds another one registering waits for
//! it, and a child that inherits a registration "in progress" from a parent
//! that forked in the middle of it registers itself instead of waiting for a
//! thread it does not have. `fork_register` runs the same state machine with
//! a counter in place of `pthread_atfork`, on any platform.
#![cfg(feature = "testing")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use emfor_client::testing::{fork_register, fork_registration_set};

/// The registration state is process-wide: one test at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Another process's pid, as the parent of a child.
fn parent_pid() -> u32 {
    std::process::id() + 1
}

#[test]
fn registration_inherited_in_progress_is_redone() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let calls = Arc::new(AtomicU64::new(0));
    let (done_tx, done_rx) = mpsc::channel();
    let c = calls.clone();
    std::thread::spawn(move || {
        // The parent forked while one of its threads was registering.
        fork_registration_set(parent_pid(), 1);
        fork_register(move || {
            c.fetch_add(1, Ordering::SeqCst);
            true
        });
        done_tx.send(()).unwrap();
    });
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("a child registers itself instead of waiting for its parent's thread");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn registration_inherited_done_is_redone_once() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let calls = AtomicU64::new(0);
    let register = || {
        calls.fetch_add(1, Ordering::SeqCst);
        true
    };
    fork_registration_set(parent_pid(), 2);
    fork_register(register);
    fork_register(register);
    fork_register(register);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn concurrent_callers_register_once_and_wait_for_it() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    // No registration in this process yet.
    fork_registration_set(parent_pid(), 0);
    let calls = Arc::new(AtomicU64::new(0));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let calls = calls.clone();
            std::thread::spawn(move || {
                let c = calls.clone();
                fork_register(move || {
                    std::thread::sleep(Duration::from_millis(100));
                    c.fetch_add(1, Ordering::SeqCst);
                    true
                });
                // Returns only once the registration is over.
                calls.load(Ordering::SeqCst)
            })
        })
        .collect();
    for t in threads {
        assert_eq!(
            t.join().unwrap(),
            1,
            "a caller returned before the registration ended"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_registration_is_tried_again() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    fork_registration_set(parent_pid(), 0);
    let calls = AtomicU64::new(0);
    // The first attempt fails (pthread_atfork out of memory): not recorded.
    fork_register(|| {
        calls.fetch_add(1, Ordering::SeqCst);
        false
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // The next lookup tries again, and this time it succeeds.
    fork_register(|| {
        calls.fetch_add(1, Ordering::SeqCst);
        true
    });
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // Registered now: no more attempts.
    fork_register(|| {
        calls.fetch_add(1, Ordering::SeqCst);
        true
    });
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
