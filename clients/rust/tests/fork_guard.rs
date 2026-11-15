//! The fork handlers (src/fork.rs) keep every thread out of the store while
//! a fork is prepared, and let them in again after it. Called directly, on
//! any platform: spec client-lookup "Clients are safe across threads and
//! fork".
#![cfg(feature = "testing")]
mod support;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use emfor_client::lookup;
use emfor_client::testing::{fork_prepare, fork_release};

/// The tests share the process's client and the fork state: one at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn open_store() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::write_store(&path);
    support::set_env(&path, Some("60s"));
    // The process's client exists and has opened the store.
    assert!(support::API.contains(&lookup("api.svc.emfor", None).unwrap().as_str()));
    dir
}

/// After `fork_prepare` a lookup on another thread does not return until
/// `fork_release`, then it returns an address.
#[test]
fn prepare_blocks_lookups_until_release() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let _dir = open_store();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel();
    let t = std::thread::spawn(move || {
        go_rx.recv().unwrap();
        done_tx.send(lookup("api.svc.emfor", None)).unwrap();
    });
    fork_prepare();
    go_tx.send(()).unwrap();
    assert!(
        done_rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "a lookup returned while a fork was prepared"
    );
    fork_release();
    let r = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the lookup returns once the fork is over");
    assert!(support::API.contains(&r.unwrap().as_str()));
    t.join().unwrap();
}

/// A second `fork_prepare` without a release in between takes no second
/// lock (it would wait for itself): one release is enough.
#[test]
fn prepare_twice_needs_one_release() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let _dir = open_store();
    fork_prepare();
    fork_prepare();
    fork_release();
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || done_tx.send(lookup("api.svc.emfor", None)).unwrap());
    let r = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("a lookup returns after one release");
    assert!(support::API.contains(&r.unwrap().as_str()));
}

/// A prepare waits for the lookups in progress and a release leaves no hold
/// behind: 1000 pairs with 4 threads looking up, after which every thread
/// still makes progress.
#[test]
fn release_leaves_no_hold() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let _dir = open_store();
    let stop = Arc::new(AtomicBool::new(false));
    let counts: Vec<Arc<AtomicU64>> = (0..4).map(|_| Arc::new(AtomicU64::new(0))).collect();
    let threads: Vec<_> = counts
        .iter()
        .map(|count| {
            let (stop, count) = (stop.clone(), count.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    lookup("api.svc.emfor", None).unwrap();
                    count.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();
    // Every thread is looking up before the forks start.
    wait_until("every thread looked up", || {
        counts.iter().all(|c| c.load(Ordering::Relaxed) > 0)
    });
    for _ in 0..1000 {
        fork_prepare();
        fork_release();
    }
    // And again after them: no hold is left.
    let after: Vec<u64> = counts.iter().map(|c| c.load(Ordering::Relaxed)).collect();
    wait_until("every thread went on looking up", || {
        counts
            .iter()
            .zip(&after)
            .all(|(c, before)| c.load(Ordering::Relaxed) > *before)
    });
    stop.store(true, Ordering::Relaxed);
    for t in threads {
        t.join().unwrap();
    }
    assert!(support::API.contains(&lookup("api.svc.emfor", None).unwrap().as_str()));
}

/// Wait up to 10 seconds for `cond`.
fn wait_until(what: &str, cond: impl Fn() -> bool) {
    let start = std::time::Instant::now();
    while !cond() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
