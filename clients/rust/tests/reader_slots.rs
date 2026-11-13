//! A client that opens the store while no other process has it open sizes
//! the lock file, so it asks for the daemon's reader slot count rather than
//! LMDB's default of 126.
#![cfg(feature = "testing")]

mod support;

use emfor_client::testing::Client;

#[test]
fn first_reader_sizes_the_lock_file_for_every_slot() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::write_store(&path); // written and closed again
    Client::with_max_staleness_ms(&path, 3000)
        .lookup("api.svc.emfor", None)
        .unwrap();

    // One 64-byte (cache line) slot per reader, after a small header.
    let lock = std::fs::metadata(format!("{}-lock", path.display())).unwrap();
    let slots = emfor_format::MAX_READERS as u64;
    assert!(
        lock.len() >= slots * 64,
        "lock file is {} bytes",
        lock.len()
    );
    assert!(
        lock.len() < (slots + 8) * 64,
        "lock file is {} bytes",
        lock.len()
    );
}
