//! A running client keeps reading after the daemon grows the store past the
//! client's map size (a restart with a larger `map_size`, then more data):
//! LMDB reports `MDB_MAP_RESIZED`, and the client maps the new size.
#![cfg(feature = "testing")]

mod support;

use heed::types::{Bytes, Str};
use heed::{Database, EnvFlags, EnvOpenOptions};

/// What the fixture's store maps (emfor_fixture::write_store).
const FIXTURE_MAP: usize = 4 << 20;

/// Open the store as a daemon with a 16 MiB map would and add 8 MiB, so the
/// file grows past [`FIXTURE_MAP`].
fn grow(path: &std::path::Path) {
    // SAFETY: the store is only read by the parent meanwhile.
    let env = unsafe {
        EnvOpenOptions::new()
            .map_size(4 * FIXTURE_MAP)
            .flags(EnvFlags::NO_SUB_DIR)
            .open(path)
    }
    .unwrap();
    let mut txn = env.write_txn().unwrap();
    let db: Database<Str, Bytes> = env.open_database(&txn, None).unwrap().unwrap();
    let filler = vec![0x5a_u8; 64 << 10];
    for i in 0..128 {
        db.put(&mut txn, &format!("filler/{i}"), &filler).unwrap();
    }
    txn.commit().unwrap();
    env.prepare_for_closing().wait();
}

#[test]
fn store_grown_past_the_map_size_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::write_store(&path);
    let c = emfor_client::testing::Client::new(&path, "60s");
    let got = c.lookup("api.svc.emfor", None).unwrap();
    assert!(support::API.contains(&got.as_str()), "{got}");

    // In a child process: LMDB must not open one file twice in a process.
    // SAFETY: this test's process has no other thread that could hold a lock
    // the child needs; the child only writes the store and calls _exit.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        let ok = std::panic::catch_unwind(|| grow(&path)).is_ok();
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }
    let mut status = 0;
    // SAFETY: waiting for our own child.
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    assert!(
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
        "growing the store failed: status {status}"
    );
    assert!(
        std::fs::metadata(&path).unwrap().len() > FIXTURE_MAP as u64,
        "store did not grow past the old map size"
    );

    for _ in 0..3 {
        let got = c.lookup("api.svc.emfor", None).unwrap();
        assert!(support::API.contains(&got.as_str()), "{got}");
    }
}
