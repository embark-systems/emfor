//! `emfor_lookup` called from a thread-local destructor that runs after the
//! C API's own (the last-error message) returns a status instead of
//! aborting the process.
#![cfg(feature = "capi")]

use std::ffi::c_char;
use std::sync::atomic::{AtomicI32, Ordering};

use emfor_client::capi::{EMFOR_ADDR_MAX, emfor_lookup, emfor_status};

static STATUS: AtomicI32 = AtomicI32::new(-1);

/// A NULL name: `EMFOR_INVALID_ARGUMENT` without touching any store.
fn call() -> emfor_status {
    let mut out = [0 as c_char; EMFOR_ADDR_MAX];
    // SAFETY: `out` holds `out.len()` bytes; NULL name and key are allowed.
    unsafe {
        emfor_lookup(
            std::ptr::null(),
            std::ptr::null(),
            out.as_mut_ptr(),
            out.len(),
        )
    }
}

struct LookupOnDrop;

impl Drop for LookupOnDrop {
    fn drop(&mut self) {
        STATUS.store(call() as i32, Ordering::SeqCst);
    }
}

thread_local! {
    static LATE: LookupOnDrop = const { LookupOnDrop };
}

#[test]
fn lookup_from_a_late_thread_local_destructor() {
    std::thread::spawn(|| {
        // Destructors run in reverse order of first use: LATE's after the
        // C API's last-error message, which `call` sets up.
        LATE.with(|_| {});
        assert_eq!(call(), emfor_status::EMFOR_INVALID_ARGUMENT);
    })
    .join()
    .unwrap();
    assert_eq!(
        STATUS.load(Ordering::SeqCst),
        emfor_status::EMFOR_INVALID_ARGUMENT as i32
    );
}
