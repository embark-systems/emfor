//! A fork prepared before the process's first lookup keeps that lookup from
//! opening the store until the fork is over (the process has no client yet,
//! so there is no lock to take: src/fork.rs `FORKING`). A real fork in the
//! middle: the child and the parent both look up afterwards. Its own binary:
//! the process's client can be created only once.
#![cfg(feature = "testing")]
mod support;

use std::sync::mpsc;
use std::time::Duration;

use emfor_client::lookup;
use emfor_client::testing::{fork_prepare, fork_release, fork_release_child};

// spectra-scenario: client-lookup / Fork during the first lookup
#[test]
fn forking_flag_blocks_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::write_store(&path);
    support::set_env(&path, Some("60s"));
    fork_prepare();
    let (done_tx, done_rx) = mpsc::channel();
    let t = std::thread::spawn(move || done_tx.send(lookup("api.svc.emfor", None)).unwrap());
    // The fork itself, with the first lookup waiting; the child runs what
    // the child handler does, then looks up.
    // SAFETY: the child only releases, looks up and calls _exit, as the
    // fork_threads test's children do.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        fork_release_child();
        let ok = lookup("api.svc.emfor", None).is_ok_and(|a| support::API.contains(&a.as_str()));
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }
    assert!(
        done_rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "the first lookup opened the store while a fork was prepared"
    );
    fork_release();
    let r = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the first lookup returns once the fork is over");
    assert!(support::API.contains(&r.unwrap().as_str()));
    t.join().unwrap();
    let start = std::time::Instant::now();
    let mut status = 0;
    loop {
        // SAFETY: waiting for our own child.
        let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        assert!(r >= 0, "waitpid failed");
        if r == pid {
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            // SAFETY: our own child.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
                libc::waitpid(pid, &mut status, 0);
            }
            panic!("the child's first lookup did not return within 5 seconds");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
    assert!(support::API.contains(&lookup("api.svc.emfor", None).unwrap().as_str()));
}
