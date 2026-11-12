//! Spec "Store written by a different LMDB format", lock file side: while
//! another process has the store open, a lock file whose format word differs
//! from this build's (as the system liblmdb's did in spike 1) gives
//! `UNAVAILABLE` with the store path and the release rule.
#![cfg(feature = "testing")]

mod support;

use std::io::{Read, Seek, SeekFrom, Write};

use emfor_client::ErrorCode;
use emfor_client::testing::Client;

#[test]
fn lock_format_mismatch_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::write_store(&path);

    let mut ready = [0; 2];
    let mut done = [0; 2];
    // SAFETY: plain pipe(2) calls on local arrays.
    assert_eq!(unsafe { libc::pipe(ready.as_mut_ptr()) }, 0);
    assert_eq!(unsafe { libc::pipe(done.as_mut_ptr()) }, 0);
    // SAFETY: the child only opens the store, writes and reads a pipe, and
    // calls _exit.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        // Keep the store open, holding LMDB's shared lock, until told to stop.
        let c = Client::with_max_staleness_ms(&path, 3000);
        let ok = c.lookup("api.svc.emfor", None).is_ok();
        let mut b = [u8::from(ok)];
        unsafe {
            libc::write(ready[1], b.as_ptr().cast(), 1);
            libc::read(done[0], b.as_mut_ptr().cast(), 1);
            libc::_exit(0);
        }
    }
    let mut b = [0u8];
    // SAFETY: reading one byte from our pipe.
    assert_eq!(unsafe { libc::read(ready[0], b.as_mut_ptr().cast(), 1) }, 1);
    assert_eq!(b[0], 1, "child could not open the store");

    // The lock file starts with a u32 magic and the u32 format word.
    let lock = format!("{}-lock", path.display());
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock)
        .unwrap();
    let mut word = [0u8; 4];
    f.seek(SeekFrom::Start(4)).unwrap();
    f.read_exact(&mut word).unwrap();
    let changed = (u32::from_ne_bytes(word) ^ 0x8000_0000).to_ne_bytes();
    f.seek(SeekFrom::Start(4)).unwrap();
    f.write_all(&changed).unwrap();
    drop(f);

    let e = Client::with_max_staleness_ms(&path, 3000)
        .lookup("api.svc.emfor", None)
        .unwrap_err();

    // SAFETY: let the child exit, then reap it.
    unsafe { libc::write(done[1], b.as_ptr().cast(), 1) };
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);

    assert_eq!(e.code, ErrorCode::Unavailable, "{e}");
    assert!(e.message.contains("MDB_VERSION_MISMATCH"), "{e}");
    assert!(e.message.contains(&path.display().to_string()), "{e}");
    assert!(
        e.message
            .contains("install libemfor from the same emfor release as the daemon"),
        "{e}"
    );
}
