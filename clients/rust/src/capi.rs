//! C API (feature `capi`), built into `libemfor` with cargo-c:
//! `cargo cinstall --release --features capi --prefix <prefix>`, then link
//! with `pkg-config --cflags --libs emfor` and `#include <emfor.h>`.
//!
//! No Rust panic unwinds into C: each function catches it and reports
//! `EMFOR_UNAVAILABLE` with an `internal error` message.
#![allow(non_camel_case_types)]

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int};
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::{Error, ErrorCode};

/// Result of a C call.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum emfor_status {
    EMFOR_OK = 0,
    EMFOR_UNAVAILABLE = 1,
    EMFOR_INCOMPATIBLE = 2,
    EMFOR_NOT_FOUND = 3,
    EMFOR_ALL_DOWN = 4,
    /// C only: NULL `name` or `out`, a string that is not UTF-8, or
    /// `out_len` too small.
    EMFOR_INVALID_ARGUMENT = 5,
}

/// Buffer size that holds any backend address, `[IPv6]:port`, plus NUL.
pub const EMFOR_ADDR_MAX: usize = 64;

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

/// Never panics, so it may run outside `catch_unwind`: in a thread-local
/// destructor that runs after `LAST_ERROR`'s, the message is dropped
/// (`emfor_last_error` then returns `"internal error"`).
fn set_last_error(msg: &str) {
    // Every NUL byte is replaced, so this never falls back to the empty string.
    let c = CString::new(msg.replace('\0', " ")).unwrap_or_default();
    let _ = LAST_ERROR.try_with(|e| {
        if let Ok(mut e) = e.try_borrow_mut() {
            *e = c;
        }
    });
}

fn status_of(code: ErrorCode) -> emfor_status {
    match code {
        ErrorCode::Unavailable => emfor_status::EMFOR_UNAVAILABLE,
        ErrorCode::Incompatible => emfor_status::EMFOR_INCOMPATIBLE,
        ErrorCode::NotFound => emfor_status::EMFOR_NOT_FOUND,
        ErrorCode::AllDown => emfor_status::EMFOR_ALL_DOWN,
    }
}

fn fail(status: emfor_status, msg: &str) -> emfor_status {
    set_last_error(msg);
    status
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    let m = p
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into());
    format!("internal error: {m}")
}

/// # Safety
/// `s` is NULL or a valid NUL-terminated string.
unsafe fn arg<'a>(s: *const c_char, what: &str) -> Result<Option<&'a str>, String> {
    if s.is_null() {
        return Ok(None);
    }
    // SAFETY: the caller passes a valid C string.
    unsafe { CStr::from_ptr(s) }
        .to_str()
        .map(Some)
        .map_err(|_| format!("{what} is not valid UTF-8"))
}

/// Look up one backend of service `name` and write its address (`ip:port`,
/// or `[ip]:port` for IPv6) with a terminating NUL to `out`, which holds
/// `out_len` bytes; `EMFOR_ADDR_MAX` is always enough. `affinity_key` may be
/// NULL. On failure `out` is not changed and `emfor_last_error()` describes
/// the cause. Safe to call from many threads and after `fork()`.
///
/// # Safety
/// `name` and a non-NULL `affinity_key` are NUL-terminated strings; `out`
/// points to `out_len` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn emfor_lookup(
    name: *const c_char,
    affinity_key: *const c_char,
    out: *mut c_char,
    out_len: usize,
) -> emfor_status {
    let run = || -> emfor_status {
        // SAFETY: per this function's contract.
        let name = match unsafe { arg(name, "name") } {
            Ok(Some(n)) => n,
            Ok(None) => return fail(emfor_status::EMFOR_INVALID_ARGUMENT, "name is NULL"),
            Err(m) => return fail(emfor_status::EMFOR_INVALID_ARGUMENT, &m),
        };
        // SAFETY: per this function's contract.
        let key = match unsafe { arg(affinity_key, "affinity_key") } {
            Ok(k) => k,
            Err(m) => return fail(emfor_status::EMFOR_INVALID_ARGUMENT, &m),
        };
        if out.is_null() {
            return fail(emfor_status::EMFOR_INVALID_ARGUMENT, "out is NULL");
        }
        match crate::lookup(name, key) {
            Ok(addr) if addr.len() < out_len => {
                // SAFETY: `out` has `out_len` > `addr.len()` writable bytes.
                unsafe {
                    std::ptr::copy_nonoverlapping(addr.as_ptr(), out.cast::<u8>(), addr.len());
                    *out.add(addr.len()) = 0;
                }
                emfor_status::EMFOR_OK
            }
            Ok(addr) => fail(
                emfor_status::EMFOR_INVALID_ARGUMENT,
                &format!(
                    "out_len {out_len} is too small for {addr:?}, which needs {} bytes; use EMFOR_ADDR_MAX",
                    addr.len() + 1
                ),
            ),
            Err(Error { code, message }) => fail(status_of(code), &message),
        }
    };
    catch_unwind(AssertUnwindSafe(run))
        .unwrap_or_else(|p| fail(emfor_status::EMFOR_UNAVAILABLE, &panic_message(&*p)))
}

/// Name of a status, such as `"UNAVAILABLE"`; `"UNKNOWN"` for other values.
/// The string is static.
#[unsafe(no_mangle)]
pub extern "C" fn emfor_status_name(status: c_int) -> *const c_char {
    let s: &'static CStr = match status {
        0 => c"OK",
        1 => c"UNAVAILABLE",
        2 => c"INCOMPATIBLE",
        3 => c"NOT_FOUND",
        4 => c"ALL_DOWN",
        5 => c"INVALID_ARGUMENT",
        _ => c"UNKNOWN",
    };
    s.as_ptr()
}

/// Message of the last failed call on this thread (empty if none). Valid
/// until the next emfor call on the same thread.
#[unsafe(no_mangle)]
pub extern "C" fn emfor_last_error() -> *const c_char {
    catch_unwind(|| LAST_ERROR.with(|e| e.borrow().as_ptr())).unwrap_or(c"internal error".as_ptr())
}
