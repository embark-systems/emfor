//! Spike 2 probe. `probe_heartbeat(path)` opens the store read-only the way a
//! client will (`MDB_RDONLY | MDB_NOSUBDIR | MDB_NOTLS`) and returns
//! `meta/heartbeat`, or a negative number on failure: the LMDB error code
//! (`MDB_VERSION_MISMATCH` is -30794), or -1 for anything else.

use std::ffi::{CStr, c_char};
use std::sync::Mutex;

use heed::types::Str;
use heed::{Database, Env, EnvFlags, EnvOpenOptions, WithoutTls};

/// One environment per process: heed refuses to open the same path twice.
static ENV: Mutex<Option<(String, Env<WithoutTls>)>> = Mutex::new(None);

fn code(e: heed::Error) -> i64 {
    match e {
        heed::Error::Mdb(m) => m.to_err_code() as i64,
        _ => -1,
    }
}

fn read(path: &str) -> Result<i64, i64> {
    let mut slot = ENV.lock().unwrap_or_else(|p| p.into_inner());
    if slot.as_ref().is_none_or(|(p, _)| p != path) {
        let env = unsafe {
            let mut o = EnvOpenOptions::new().read_txn_without_tls();
            o.flags(EnvFlags::NO_SUB_DIR | EnvFlags::READ_ONLY);
            // SAFETY: read-only; the daemon is the only writer.
            o.open(path).map_err(code)?
        };
        *slot = Some((path.to_string(), env));
    }
    let env = &slot.as_ref().expect("set above").1;
    let txn = env.read_txn().map_err(code)?;
    let db: Database<Str, Str> = env.open_database(&txn, None).map_err(code)?.ok_or(-1)?;
    let hb = db.get(&txn, "meta/heartbeat").map_err(code)?.ok_or(-1)?;
    hb.parse().map_err(|_| -1)
}

/// Heartbeat of the store at `path`, or a negative error (see the module docs).
///
/// # Safety
/// `path` must be a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn probe_heartbeat(path: *const c_char) -> i64 {
    std::panic::catch_unwind(|| {
        if path.is_null() {
            return -1;
        }
        // SAFETY: the caller passes a valid C string.
        let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else {
            return -1;
        };
        read(path).unwrap_or_else(|e| e)
    })
    .unwrap_or(-1)
}
