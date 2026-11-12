//! A lookup before fork, in the child, and again in the parent all succeed:
//! the child opens the store again instead of using the inherited handle,
//! and reads the environment again (here a different `EMFOR_STORE`).
mod support;

use emfor_client::lookup;

const CHILD_BACKEND: &str = "10.0.0.99:8080";

#[test]
fn lookup_after_fork() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::write_store(&path);
    support::set_env(&path, None);
    // A second store with one other backend, used only by the child.
    let child_path = dir.path().join("child.mdb");
    let mut store = support::store_json();
    store["services"][0]["backends"] = serde_json::json!([{"id": CHILD_BACKEND, "ip": "10.0.0.99",
        "port": 8080, "state": "healthy", "checked_at": 1, "reason": null}]);
    emfor_fixture::write_store(&store, 3000, emfor_fixture::now_ms(), &child_path).unwrap();
    assert!(support::API.contains(&lookup("api.svc.emfor", None).unwrap().as_str()));
    // SAFETY: the child only looks up and calls _exit.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        // SAFETY: the child has a single thread.
        unsafe { std::env::set_var("EMFOR_STORE", &child_path) };
        let ok = (0..3).all(|_| lookup("api.svc.emfor", None).is_ok_and(|a| a == CHILD_BACKEND));
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }
    let mut status = 0;
    // SAFETY: waiting for our own child.
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    assert!(
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
        "child lookup failed: status {status}"
    );
    assert!(support::API.contains(&lookup("api.svc.emfor", None).unwrap().as_str()));
}
