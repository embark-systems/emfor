//! Read-only access to the store through LMDB's C API (lmdb-master-sys, the
//! binding heed uses, so the daemon's LMDB build). heed itself is not used:
//! it keeps a process-wide list of open files and only forgets a file when
//! its environment is closed, but a forked child must never close the
//! environment it inherited, so it could never open the store again.

use std::ffi::{CStr, CString, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;

use lmdb_master_sys as ffi;

use crate::{Error, ErrorCode};

/// An open read-only LMDB environment (`MDB_RDONLY | MDB_NOSUBDIR | MDB_NOTLS`).
pub struct Env {
    env: *mut ffi::MDB_env,
    dbi: ffi::MDB_dbi,
    path: PathBuf,
    /// Device and inode of the file at `path` when it was opened.
    file: (u64, u64),
    /// Device and inode of `<path>-lock` once opened; `None` if it could not
    /// be read.
    lock: Option<(u64, u64)>,
}

/// LMDB's lock file of a store opened with `MDB_NOSUBDIR`.
fn lock_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push("-lock");
    PathBuf::from(p)
}

fn file_id(m: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (m.dev(), m.ino())
}

// SAFETY: with MDB_NOTLS, read transactions may be created on any thread,
// and the handle itself is immutable after opening.
unsafe impl Send for Env {}
unsafe impl Sync for Env {}

/// `ENOENT`, the same on Linux, FreeBSD and macOS.
const ENOENT: i32 = 2;

fn strerror(rc: i32) -> String {
    // SAFETY: mdb_strerror returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(ffi::mdb_strerror(rc)) }
        .to_string_lossy()
        .into_owned()
}

fn unavailable(msg: String) -> Error {
    Error::new(ErrorCode::Unavailable, msg)
}

fn open_error(path: &Path, rc: i32) -> Error {
    let p = path.display();
    unavailable(match rc {
        ffi::MDB_VERSION_MISMATCH | ffi::MDB_INVALID => format!(
            "LMDB format of {p} differs from this emfor client ({}); install libemfor from the same emfor release as the daemon",
            strerror(rc)
        ),
        ENOENT => format!("store {p} does not exist; is the emfor daemon running?"),
        _ => format!("cannot open store {p}: {}", strerror(rc)),
    })
}

/// Values read in one read transaction.
pub struct Snapshot {
    pub schema_version: Option<String>,
    pub heartbeat: Option<String>,
    pub record: Option<String>,
    /// `meta/site`: the host's site, for `locality`.
    pub site: Option<String>,
}

/// Why [`Env::snapshot`] failed.
pub enum SnapshotError {
    /// The daemon grew the store past this environment's map size
    /// (`MDB_MAP_RESIZED`): call [`Env::adopt_map_size`], then read again.
    MapResized,
    Failed(Error),
}

impl From<Error> for SnapshotError {
    fn from(e: Error) -> Self {
        SnapshotError::Failed(e)
    }
}

impl SnapshotError {
    /// The error a lookup reports for this failure on the store at `path`.
    pub fn into_error(self, path: &Path) -> Error {
        match self {
            SnapshotError::MapResized => unavailable(format!(
                "cannot read store {}: {}",
                path.display(),
                strerror(ffi::MDB_MAP_RESIZED)
            )),
            SnapshotError::Failed(e) => e,
        }
    }
}

/// Aborts a read transaction when dropped.
struct Txn(*mut ffi::MDB_txn);

impl Drop for Txn {
    fn drop(&mut self) {
        // SAFETY: a live read-only transaction owned by this guard.
        unsafe { ffi::mdb_txn_abort(self.0) }
    }
}

impl Env {
    pub fn open(path: &Path) -> Result<Env, Error> {
        // Check first: LMDB would create `<path>-lock` even when the data
        // file is missing, and a lock file owned by an application user can
        // stop the daemon from starting later.
        // `Path::exists` would report a directory without search permission
        // as a missing file, so keep the real cause.
        // The file identity is taken before opening: if the file is replaced
        // in between, `replaced` reports it on the next stale lookup.
        let file = match std::fs::metadata(path) {
            Ok(m) => file_id(&m),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(open_error(path, ENOENT));
            }
            Err(e) => {
                return Err(unavailable(format!(
                    "cannot open store {}: {e}",
                    path.display()
                )));
            }
        };
        let cpath = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            unavailable(format!("store path {} contains a NUL byte", path.display()))
        })?;
        let mut env = ptr::null_mut();
        // SAFETY: plain LMDB C calls; `env` is closed on every error path.
        unsafe {
            let rc = ffi::mdb_env_create(&mut env);
            if rc != 0 {
                return Err(open_error(path, rc));
            }
            // Matches the daemon, in case this process sizes the lock file.
            let rc = ffi::mdb_env_set_maxreaders(env, emfor_format::MAX_READERS);
            if rc != 0 {
                ffi::mdb_env_close(env);
                return Err(open_error(path, rc));
            }
            let flags = ffi::MDB_RDONLY | ffi::MDB_NOSUBDIR | ffi::MDB_NOTLS;
            let rc = ffi::mdb_env_open(env, cpath.as_ptr(), flags, 0o644);
            if rc != 0 {
                ffi::mdb_env_close(env);
                return Err(open_error(path, rc));
            }
            let mut txn = ptr::null_mut();
            let mut dbi = 0;
            let mut rc = ffi::mdb_txn_begin(env, ptr::null_mut(), ffi::MDB_RDONLY, &mut txn);
            if rc == 0 {
                rc = ffi::mdb_dbi_open(txn, ptr::null(), 0, &mut dbi);
                if rc == 0 {
                    rc = ffi::mdb_txn_commit(txn);
                } else {
                    ffi::mdb_txn_abort(txn);
                }
            }
            if rc != 0 {
                ffi::mdb_env_close(env);
                return Err(open_error(path, rc));
            }
            let lock = std::fs::metadata(lock_path(path)).ok().map(|m| file_id(&m));
            Ok(Env {
                env,
                dbi,
                path: path.to_path_buf(),
                file,
                lock,
            })
        }
    }

    /// True when the file now at the store path is another file than the
    /// one this environment reads: the daemon set a damaged store aside, or
    /// the store was deleted and created again. A missing file is not a
    /// replacement: the open file stays the best there is.
    pub fn replaced(&self) -> bool {
        std::fs::metadata(&self.path).is_ok_and(|m| file_id(&m) != self.file)
    }

    /// True when `<path>-lock` is no longer the lock file this environment
    /// uses (removed, or another file), as when the daemon sets a damaged
    /// store aside. The store at the path can then be opened while this one
    /// is still open: LMDB only forbids two environments on one lock file in
    /// a process.
    pub fn lock_file_replaced(&self) -> bool {
        match (std::fs::metadata(lock_path(&self.path)), self.lock) {
            (Err(e), Some(_)) => e.kind() == std::io::ErrorKind::NotFound,
            (Ok(m), Some(lock)) => file_id(&m) != lock,
            (_, None) => false,
        }
    }

    /// Map the store with the size the daemon now uses, after a read failed
    /// with [`SnapshotError::MapResized`] (the daemon was restarted with a
    /// larger `map_size` and the store grew past the old one). Takes
    /// `&mut self` because LMDB allows this only while no transaction of
    /// this environment is active.
    pub fn adopt_map_size(&mut self) -> Result<(), Error> {
        // SAFETY: the environment is open, and `&mut self` means no read
        // transaction is active (they live only inside `snapshot`). Size 0
        // takes the size from the store's newest meta page, and never less
        // than the file's last page.
        let rc = unsafe { ffi::mdb_env_set_mapsize(self.env, 0) };
        if rc != 0 {
            return Err(unavailable(format!(
                "cannot map store {} with its new size: {}",
                self.path.display(),
                strerror(rc)
            )));
        }
        Ok(())
    }

    /// Read schema version, heartbeat and one service record in a single
    /// read transaction, which ends before this returns.
    pub fn snapshot(&self, record_key: &str) -> Result<Snapshot, SnapshotError> {
        let mut txn = ptr::null_mut();
        // SAFETY: the environment is open; the guard ends the transaction.
        let rc =
            unsafe { ffi::mdb_txn_begin(self.env, ptr::null_mut(), ffi::MDB_RDONLY, &mut txn) };
        if rc == ffi::MDB_MAP_RESIZED {
            return Err(SnapshotError::MapResized);
        }
        if rc != 0 {
            return Err(unavailable(format!(
                "cannot read store {}: {}",
                self.path.display(),
                strerror(rc)
            ))
            .into());
        }
        let txn = Txn(txn);
        Ok(Snapshot {
            schema_version: self.get(&txn, emfor_format::KEY_SCHEMA_VERSION)?,
            heartbeat: self.get(&txn, emfor_format::KEY_HEARTBEAT)?,
            record: self.get(&txn, record_key)?,
            // Only `locality` uses the site: an unreadable value counts as
            // no site rather than failing lookups of every service.
            site: self.get(&txn, emfor_format::KEY_SITE).ok().flatten(),
        })
    }

    fn get(&self, txn: &Txn, key: &str) -> Result<Option<String>, Error> {
        let mut k = ffi::MDB_val {
            mv_size: key.len(),
            mv_data: key.as_ptr() as *mut c_void,
        };
        let mut v = ffi::MDB_val {
            mv_size: 0,
            mv_data: ptr::null_mut(),
        };
        // SAFETY: `txn` is live; the value is copied before it ends.
        let rc = unsafe { ffi::mdb_get(txn.0, self.dbi, &mut k, &mut v) };
        match rc {
            0 => {
                // SAFETY: LMDB returned a valid slice inside the map.
                let bytes =
                    unsafe { std::slice::from_raw_parts(v.mv_data as *const u8, v.mv_size) };
                String::from_utf8(bytes.to_vec()).map(Some).map_err(|_| {
                    Error::new(
                        ErrorCode::Incompatible,
                        format!("value of {key} is not UTF-8"),
                    )
                })
            }
            ffi::MDB_NOTFOUND => Ok(None),
            rc => Err(unavailable(format!(
                "cannot read {key} from {}: {}",
                self.path.display(),
                strerror(rc)
            ))),
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // SAFETY: opened by this process (inherited handles are leaked, never dropped).
        unsafe { ffi::mdb_env_close(self.env) }
    }
}
