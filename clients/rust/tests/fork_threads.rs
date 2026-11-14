//! A child forked while other threads of the parent look up finishes its own
//! lookup: no lock the parent's threads held at the fork may block it.
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use emfor_client::lookup;

/// Forks to try; each has a chance that a thread is inside a lookup.
const FORKS: usize = 300;
/// A child's lookup takes milliseconds; one that has not exited by then hangs.
const CHILD_TIMEOUT: Duration = Duration::from_secs(5);

#[test]
fn child_of_a_busy_parent_looks_up() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::write_store(&path);
    support::set_env(&path, Some("60s"));
    assert!(support::API.contains(&lookup("api.svc.emfor", None).unwrap().as_str()));

    let stop = Arc::new(AtomicBool::new(false));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    lookup("api.svc.emfor", None).unwrap();
                }
            })
        })
        .collect();

    let mut hung = None;
    for n in 0..FORKS {
        // SAFETY: the child only looks up and calls _exit; the allocator and
        // the client are fork-safe, which is what this test checks.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            let ok = lookup("api.svc.emfor", None).is_ok();
            unsafe { libc::_exit(if ok { 0 } else { 1 }) };
        }
        let start = Instant::now();
        let mut status = 0;
        loop {
            // SAFETY: waiting for our own child.
            let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            assert!(r >= 0, "waitpid failed");
            if r == pid {
                break;
            }
            if start.elapsed() > CHILD_TIMEOUT {
                // SAFETY: our own child.
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                    libc::waitpid(pid, &mut status, 0);
                }
                hung = Some(n);
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        if hung.is_some() {
            break;
        }
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "child {n} lookup failed: status {status}"
        );
    }
    stop.store(true, Ordering::Relaxed);
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(
        hung, None,
        "a child hung for {CHILD_TIMEOUT:?} in its lookup"
    );
}
