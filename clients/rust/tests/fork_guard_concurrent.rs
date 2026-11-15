//! Two threads fork at once before the process's first lookup (no client
//! yet, so no lock to take): one fork's release must not let the first
//! lookup in while the other fork is still being prepared (src/fork.rs
//! `FORKING` counts the forks). Its own binary: the process's client can be
//! created only once.
#![cfg(feature = "testing")]
mod support;

use std::sync::mpsc;
use std::time::Duration;

use emfor_client::lookup;
use emfor_client::testing::{fork_prepare, fork_release};

#[test]
fn one_release_does_not_end_another_fork() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::write_store(&path);
    support::set_env(&path, Some("60s"));

    // Thread B prepares a fork and keeps it prepared until told to release.
    let (prepared_tx, prepared_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let b = std::thread::spawn(move || {
        fork_prepare();
        prepared_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        fork_release();
    });
    prepared_rx.recv().unwrap();

    // This thread prepares and releases its own fork meanwhile.
    fork_prepare();
    fork_release();

    // The first lookup of the process must still wait: B's fork is on.
    let (done_tx, done_rx) = mpsc::channel();
    let c = std::thread::spawn(move || done_tx.send(lookup("api.svc.emfor", None)).unwrap());
    assert!(
        done_rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "the first lookup opened the store while another fork was prepared"
    );
    release_tx.send(()).unwrap();
    let r = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the first lookup returns once both forks are over");
    assert!(support::API.contains(&r.unwrap().as_str()));
    b.join().unwrap();
    c.join().unwrap();
}
