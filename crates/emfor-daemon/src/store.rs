//! LMDB state store: file layout, writer lock, and low-level access.
//!
//! Layout (format version 2, see `emfor_format`): one data file opened with
//! `MDB_NOSUBDIR` plus `<path>-lock`, the unnamed default database only, UTF-8
//! keys and values. A separate `<path>.writer` file is `flock`ed so that only
//! one daemon writes a given store.
//!
//! Only what the daemon itself just wrote is read as `Str`. Scans and
//! deletions of a whole prefix work on bytes, so one key or value that is
//! not UTF-8 (from a damaged page or another writer) affects only itself.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use heed::types::{Bytes, DecodeIgnore, Str};
use heed::{CommitOrAbort, Database, Env, EnvFlags, EnvOpenOptions, RwTxn, WithoutTls};
use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};

use crate::config::StoreConfig;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store is already in use by another emfor daemon: lock file {} is held", .0.display())]
    Locked(PathBuf),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("lmdb: {0}")]
    Lmdb(#[from] heed::Error),
    #[error("{}: {why}", path.display())]
    Truncated { path: PathBuf, why: String },
}

/// Why [`open_env`] failed.
#[derive(Debug)]
enum OpenError {
    Lmdb(heed::Error),
    /// The data file is shorter than LMDB records it; the reason names
    /// both lengths.
    Truncated(String),
}

impl OpenError {
    fn into_store_error(self, path: &Path) -> StoreError {
        match self {
            OpenError::Lmdb(e) => StoreError::Lmdb(e),
            OpenError::Truncated(why) => StoreError::Truncated {
                path: path.to_path_buf(),
                why,
            },
        }
    }
}

impl From<heed::Error> for OpenError {
    fn from(e: heed::Error) -> Self {
        OpenError::Lmdb(e)
    }
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Lmdb(e) => e.fmt(f),
            OpenError::Truncated(why) => f.write_str(why),
        }
    }
}

/// Why a store just opened cannot be read safely, or `None` when its data
/// file is as long as LMDB records it (see [`emfor_format::truncated_store`]).
/// Reads only the meta pages and the length of the file LMDB opened; call it
/// before any read transaction, which would read past the end of a shorter
/// file and end the process with `SIGBUS`.
pub(crate) fn truncation(env: &Env<WithoutTls>) -> heed::Result<Option<String>> {
    // The meta page first, the length after: LMDB writes the data pages a
    // meta page refers to before that meta page, so a length taken later
    // covers them even when the daemon commits in between (`emfor status`
    // opens the store while the daemon writes it).
    let last_page = env.info().last_page_number as u64;
    let page_size = u64::from(env.unnamed_database_stat().page_size);
    let len = env.real_disk_size()?;
    Ok(emfor_format::truncated_store(len, last_page, page_size))
}

pub fn lock_file_path(path: &Path) -> PathBuf {
    suffixed(path, "-lock")
}

pub fn writer_lock_path(path: &Path) -> PathBuf {
    suffixed(path, ".writer")
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

pub struct Store {
    env: Env<WithoutTls>,
    db: Database<'static, Str, Str>,
    /// LMDB's data file, to keep it as long as LMDB records it (see
    /// [`pad`]).
    data: File,
    path: PathBuf,
    _writer_lock: Flock<File>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("path", &self.path).finish()
    }
}

impl Store {
    /// Take the writer lock, then open (or create) the LMDB file.
    pub fn open(cfg: &StoreConfig) -> Result<Store, StoreError> {
        let io = |path: &Path| {
            let path = path.to_path_buf();
            move |source| StoreError::Io { path, source }
        };
        let lock_path = writer_lock_path(&cfg.path);
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&lock_path)
            .map_err(io(&lock_path))?;
        // A lock file made by an older version follows the umask; tighten it.
        // Best effort: a failure must not stop the start.
        if let Err(e) = lock_file.set_permissions(Permissions::from_mode(0o600)) {
            tracing::warn!("cannot set mode 0600 on {}: {e}", lock_path.display());
        }
        let writer_lock =
            Flock::lock(lock_file, FlockArg::LockExclusiveNonblock).map_err(|(_, errno)| {
                if errno == Errno::EWOULDBLOCK || errno == Errno::EAGAIN {
                    StoreError::Locked(lock_path.clone())
                } else {
                    StoreError::Io {
                        path: lock_path.clone(),
                        source: std::io::Error::from(errno),
                    }
                }
            })?;

        let Opened { env, db, data } = match open_env(cfg) {
            Err(e) if is_damage(&e) => {
                let corrupt = set_aside(&cfg.path).map_err(io(&cfg.path))?;
                tracing::warn!(
                    "state store {} is damaged ({e}); moved it to {} and starting with an empty store",
                    cfg.path.display(),
                    corrupt.display()
                );
                open_env(cfg).map_err(|e| e.into_store_error(&cfg.path))?
            }
            r => r.map_err(|e| e.into_store_error(&cfg.path))?,
        };
        std::fs::set_permissions(&cfg.path, Permissions::from_mode(cfg.file_mode))
            .map_err(io(&cfg.path))?;
        let lmdb_lock = lock_file_path(&cfg.path);
        std::fs::set_permissions(&lmdb_lock, Permissions::from_mode(cfg.lock_file_mode))
            .map_err(io(&lmdb_lock))?;

        let store = Store {
            env,
            db,
            data,
            path: cfg.path.clone(),
            _writer_lock: writer_lock,
        };
        // Applications may have kept the store open across a daemon restart,
        // and with it slots of readers that died since.
        store.log_stale_readers()?;
        Ok(store)
    }

    /// Free the reader slots of processes that exited during a read
    /// transaction (killed in the middle of a lookup). LMDB frees them only
    /// when asked: until then each one pins the snapshot it was reading, no
    /// page freed after it can be reused, and the file grows to `map_size`.
    /// Returns how many slots were freed.
    pub fn clear_stale_readers(&self) -> heed::Result<usize> {
        self.env.clear_stale_readers()
    }

    /// Bytes in use (every page up to the last one used) and the map size.
    /// Commits fail with `MDB_MAP_FULL` when the first reaches the second.
    pub fn usage(&self) -> (u64, u64) {
        let info = self.env.info();
        let page = u64::from(self.env.unnamed_database_stat().page_size);
        (
            (info.last_page_number as u64 + 1) * page,
            info.map_size as u64,
        )
    }

    /// [`Store::clear_stale_readers`], logging a warning when any were freed.
    pub fn log_stale_readers(&self) -> heed::Result<usize> {
        let n = self.clear_stale_readers()?;
        if n > 0 {
            tracing::warn!(
                "freed {n} store reader slot(s) left by processes that exited during a lookup"
            );
        }
        Ok(n)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run `f` in one write transaction and commit it.
    pub fn write<R>(
        &self,
        f: impl FnOnce(&mut RwTxn, &Database<Str, Str>) -> heed::Result<R>,
    ) -> heed::Result<R> {
        let mut wtxn = self.env.write_txn()?;
        let r = f(&mut wtxn, &self.db)?;
        wtxn.commit()?;
        // The commit is done whatever happens here: a failure only leaves
        // clients opening the store now to refuse it, until the next commit.
        if let Err(e) = pad(&self.env, &self.data) {
            tracing::warn!(
                "cannot extend store {} to the length LMDB records: {e}",
                self.path.display()
            );
        }
        Ok(r)
    }

    pub fn get(&self, key: &str) -> heed::Result<Option<String>> {
        let rtxn = self.env.read_txn()?;
        Ok(self.db.get(&rtxn, key)?.map(str::to_string))
    }

    /// Every key starting with `prefix` and its value, from one read
    /// transaction. A value that is not UTF-8 is `None`, for the caller to
    /// discard; a key that is not UTF-8 is logged and left out, since no
    /// caller could name it.
    pub fn prefix(&self, prefix: &str) -> heed::Result<BTreeMap<String, Option<String>>> {
        let rtxn = self.env.read_txn()?;
        let raw = self.db.remap_types::<Bytes, Bytes>();
        let mut out = BTreeMap::new();
        for item in raw.prefix_iter(&rtxn, prefix.as_bytes())? {
            let (k, v) = item?;
            let Ok(k) = std::str::from_utf8(k) else {
                tracing::warn!("ignoring store key {} that is not UTF-8", k.escape_ascii());
                continue;
            };
            let v = std::str::from_utf8(v).ok().map(str::to_owned);
            out.insert(k.to_owned(), v);
        }
        Ok(out)
    }

    /// Every key and value, for tests and debugging.
    pub fn dump(&self) -> heed::Result<BTreeMap<String, String>> {
        let rtxn = self.env.read_txn()?;
        let mut out = BTreeMap::new();
        for item in self.db.iter(&rtxn)? {
            let (k, v) = item?;
            out.insert(k.to_string(), v.to_string());
        }
        Ok(out)
    }
}

/// What [`open_env`] opened.
struct Opened {
    env: Env<WithoutTls>,
    db: Database<'static, Str, Str>,
    /// LMDB's data file, for [`pad`].
    data: File,
}

/// Open the LMDB environment and read every key once, so damage anywhere in
/// the file shows up now rather than in the first commit. A data file
/// shorter than LMDB records it is refused before anything is read.
fn open_env(cfg: &StoreConfig) -> Result<Opened, OpenError> {
    let env = unsafe {
        let mut opts = EnvOpenOptions::new().read_txn_without_tls();
        opts.map_size(cfg.map_size);
        opts.max_readers(emfor_format::MAX_READERS);
        let mut flags = EnvFlags::NO_SUB_DIR;
        if !cfg.sync {
            // The store is rebuilt on every start; a commit need not wait
            // for the disk (see `[store] sync`).
            flags |= EnvFlags::NO_SYNC;
        }
        opts.flags(flags);
        // SAFETY: only this daemon writes the file (guarded by the writer
        // lock) and readers only read. A file shorter than LMDB records is
        // refused below before anything is read; one truncated in place
        // while the daemon runs is outside what it can guard against.
        opts.open(&cfg.path)?
    };
    // On error `env` is dropped here, closing the file before it is moved.
    // Reading a truncated file would go past its end (`SIGBUS`). Nor can it
    // be extended with zero bytes and read: LMDB reads a zeroed page as a
    // leaf with thousands of entries, past the page (an assertion abort in
    // debug builds). A file LMDB left short only by pages nobody reads,
    // which this daemon pads after every commit, cannot be told apart, so
    // it is moved aside too; only the state history is lost with it.
    if let Some(why) = truncation(&env)? {
        return Err(OpenError::Truncated(why));
    }
    let data = env.try_clone_inner_file()?;
    let check = || -> heed::Result<Database<'static, Str, Str>> {
        let rtxn = env.read_txn()?;
        // heed 0.23 opens a database in a transaction it then commits, and
        // gives back a handle that outlives that transaction.
        let committed = env.open_databases_and_commit(rtxn, |opener, rtxn| -> heed::Result<_> {
            let db = opener
                .open_database::<Str, Str>(rtxn, None)?
                .expect("LMDB always has the unnamed default database, so opening it returns Some");
            // As raw bytes: only LMDB's own structure is checked here. A key or
            // value that is not UTF-8 is left to whoever reads it
            // (`load_history` discards it with a warning; `svc/*` is deleted
            // and rewritten at start, see `delete_prefix`).
            for item in db.remap_types::<Bytes, Bytes>().iter(rtxn)? {
                item?;
            }
            Ok(CommitOrAbort::<_, ()>::Commit(db))
        })?;
        Ok(committed.unwrap_commit())
    };
    // On error `env` is dropped here, closing the file before it is moved.
    let db = check()?;
    Ok(Opened { env, db, data })
}

/// Extend LMDB's data file `data` with zero bytes to the length LMDB
/// records (`(last page + 1) × page size`) when it is shorter. LMDB does not
/// write pages allocated and freed again within one transaction, yet counts
/// them in its last page number, so a store it commits can end before that;
/// readers, which refuse a store shorter than recorded (see [`truncation`]),
/// would take it for a truncated one. Holds LMDB's write lock, so no commit
/// writes the file meanwhile and it is only ever made longer.
fn pad(env: &Env<WithoutTls>, data: &File) -> heed::Result<()> {
    let txn = env.write_txn()?;
    let recorded = (env.info().last_page_number as u64 + 1)
        .checked_mul(u64::from(env.unnamed_database_stat().page_size));
    if let Some(recorded) = recorded
        && data.metadata()?.len() < recorded
    {
        data.set_len(recorded)?;
    }
    txn.abort();
    Ok(())
}

/// LMDB errors that mean the file itself is damaged, as after a power loss
/// with `MDB_NOSYNC`. `MDB_INVALID` is also what a file that is not LMDB at
/// all gives, which is why the file is moved aside, not deleted. A data
/// file shorter than LMDB records it is damaged too.
fn is_damage(e: &OpenError) -> bool {
    use heed::MdbError;
    matches!(
        e,
        OpenError::Truncated(_)
            | OpenError::Lmdb(heed::Error::Mdb(
                MdbError::Corrupted | MdbError::PageNotFound | MdbError::Invalid
            ))
    )
}

/// Move a damaged data file to `<path>.corrupt` (replacing an older one)
/// and delete its lock file, so the next open starts a new store.
fn set_aside(path: &Path) -> std::io::Result<PathBuf> {
    let corrupt = suffixed(path, ".corrupt");
    std::fs::rename(path, &corrupt)?;
    match std::fs::remove_file(lock_file_path(path)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    Ok(corrupt)
}

/// Delete every key starting with `prefix`, whether or not its key and
/// value are UTF-8: neither is decoded.
pub fn delete_prefix(txn: &mut RwTxn, db: &Database<Str, Str>, prefix: &str) -> heed::Result<()> {
    let raw = db.remap_types::<Bytes, DecodeIgnore>();
    let mut iter = raw.prefix_iter_mut(txn, prefix.as_bytes())?;
    while let Some(item) = iter.next() {
        item?;
        // SAFETY: no reference into the database is held across this call.
        unsafe { iter.del_current()? };
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn cfg(dir: &Path) -> StoreConfig {
        StoreConfig {
            path: dir.join("state.mdb"),
            map_size: 4 << 20,
            ..StoreConfig::default()
        }
    }

    #[test]
    fn prefix_returns_only_matching_keys() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&cfg(dir.path())).unwrap();
        store
            .write(|txn, db| {
                db.put(txn, "hist/a", "1")?;
                db.put(txn, "hist/b", "2")?;
                db.put(txn, "svc/a", "3")?;
                db.put(txn, "histx", "4")?;
                Ok(())
            })
            .unwrap();
        let got = store.prefix("hist/").unwrap();
        assert_eq!(
            got.into_iter().collect::<Vec<_>>(),
            [
                ("hist/a".into(), Some("1".into())),
                ("hist/b".into(), Some("2".into()))
            ]
        );
    }

    /// Spec state-store "Writer lock file mode": new and old lock files end 0600.
    /// Not covered: the chmod failure path (it only warns); a failing chmod on
    /// a file we just opened cannot be made in a test without a special fs.
    // spectra-scenario: state-store / Writer lock file mode
    #[test]
    fn writer_lock_file_is_0600() {
        let dir = tempfile::tempdir().unwrap();
        let lock = writer_lock_path(&cfg(dir.path()).path);
        for pre_existing in [false, true] {
            if pre_existing {
                std::fs::remove_file(&lock).unwrap();
                std::fs::write(&lock, "").unwrap();
                std::fs::set_permissions(&lock, Permissions::from_mode(0o644)).unwrap();
            }
            let store = Store::open(&cfg(dir.path())).unwrap();
            let mode = std::fs::metadata(&lock).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "pre_existing={pre_existing}");
            drop(store);
        }
    }

    // spectra-scenario: state-store / Files created on first start
    #[test]
    fn creates_files_with_configured_modes() {
        let dir = tempfile::tempdir().unwrap();
        let c = cfg(dir.path());
        let _store = Store::open(&c).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&c.path), 0o640);
        assert_eq!(mode(&dir.path().join("state.mdb-lock")), 0o660);
        assert!(
            std::fs::metadata(&c.path).unwrap().is_file(),
            "single file, not a directory"
        );
    }

    // spectra-scenario: state-store / Existing files get the modes at start
    #[test]
    fn existing_files_get_the_default_modes_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let c = cfg(dir.path());
        let lock = lock_file_path(&c.path);
        // Files as 0.6 left them: the old defaults.
        let old = StoreConfig {
            file_mode: 0o644,
            lock_file_mode: 0o666,
            ..cfg(dir.path())
        };
        drop(Store::open(&old).unwrap());
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&c.path), mode(&lock)), (0o644, 0o666));
        // A start without file_mode and lock_file_mode changes them.
        drop(Store::open(&c).unwrap());
        assert_eq!((mode(&c.path), mode(&lock)), (0o640, 0o660));
    }

    // spectra-scenario: state-store / Modes set explicitly
    #[test]
    fn custom_modes() {
        let dir = tempfile::tempdir().unwrap();
        let c = StoreConfig {
            file_mode: 0o644,
            lock_file_mode: 0o666,
            ..cfg(dir.path())
        };
        let _store = Store::open(&c).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&c.path), 0o644);
        assert_eq!(mode(&lock_file_path(&c.path)), 0o666);
    }

    // spectra-scenario: state-store / Commits do not wait for the disk by default
    // spectra-scenario: state-store / Commits do not wait for the disk by default / sync setting and environment flags
    #[test]
    fn sync_setting_and_environment_flags() {
        for (sync, no_sync) in [(None, true), (Some(false), true), (Some(true), false)] {
            let dir = tempfile::tempdir().unwrap();
            let mut c = cfg(dir.path());
            if let Some(s) = sync {
                c.sync = s;
            }
            let store = Store::open(&c).unwrap();
            let flags = store.env.get_flags().unwrap();
            assert_eq!(
                flags & EnvFlags::NO_SYNC.bits() != 0,
                no_sync,
                "sync = {sync:?}"
            );
        }
    }

    #[test]
    fn opens_with_more_reader_slots() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&cfg(dir.path())).unwrap();
        assert_eq!(store.env.max_readers(), emfor_format::MAX_READERS);
    }

    /// Not a test by itself: run by `frees_slots_of_killed_readers` in a
    /// child process, which it kills while the read transaction is open.
    #[test]
    fn reader_child() {
        let Some(path) = std::env::var_os("EMFOR_TEST_READER_CHILD") else {
            return;
        };
        let env = unsafe {
            let mut opts = EnvOpenOptions::new().read_txn_without_tls();
            opts.flags(EnvFlags::NO_SUB_DIR | EnvFlags::READ_ONLY);
            // SAFETY: read-only; the parent test owns the file.
            opts.open(path).unwrap()
        };
        let _txn = env.read_txn().unwrap();
        println!("reader holds a transaction");
        std::thread::sleep(std::time::Duration::from_secs(60));
    }

    #[test]
    fn frees_slots_of_killed_readers() {
        use std::io::BufRead;
        use std::process::{Command, Stdio};

        let dir = tempfile::tempdir().unwrap();
        let c = cfg(dir.path());
        let store = Store::open(&c).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "store::tests::reader_child", "--nocapture"])
            .env("EMFOR_TEST_READER_CHILD", &c.path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = std::io::BufReader::new(child.stdout.take().unwrap());
        let ready = out
            .lines()
            .map_while(Result::ok)
            .any(|l| l.contains("reader holds a transaction"));
        assert!(ready, "child did not start its read transaction");
        assert_eq!(store.clear_stale_readers().unwrap(), 0, "child still alive");

        child.kill().unwrap(); // SIGKILL: the slot is never released
        child.wait().unwrap();
        assert_eq!(store.clear_stale_readers().unwrap(), 1);
        assert_eq!(store.clear_stale_readers().unwrap(), 0);
    }

    /// Write a store with some keys, close it, damage it with `damage`
    /// (given the file and the page size), and open it again.
    fn reopen_damaged(damage: impl FnOnce(&mut std::fs::File, u64)) -> (tempfile::TempDir, Store) {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let c = cfg(dir.path());
        let page = {
            let store = Store::open(&c).unwrap();
            store
                .write(|txn, db| {
                    for i in 0..200 {
                        db.put(txn, &format!("svc/{i}"), &"x".repeat(100))?;
                    }
                    Ok(())
                })
                .unwrap();
            u64::from(store.env.unnamed_database_stat().page_size)
        };
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(&c.path)
            .unwrap();
        damage(&mut f, page);
        f.flush().unwrap();
        drop(f);
        let store = Store::open(&c).unwrap();
        (dir, store)
    }

    fn assert_replaced(dir: &tempfile::TempDir, store: &Store) {
        let path = dir.path().join("state.mdb");
        assert!(dir.path().join("state.mdb.corrupt").exists());
        assert_eq!(store.get("svc/0").unwrap(), None, "a new, empty store");
        store
            .write(|txn, db| db.put(txn, "meta/heartbeat", "1"))
            .unwrap();
        assert_eq!(store.get("meta/heartbeat").unwrap().as_deref(), Some("1"));
        assert!(path.exists());
    }

    // spectra-scenario: state-store / Damaged store after a power loss
    #[test]
    #[tracing_test::traced_test]
    fn zeroed_store_is_replaced() {
        let (dir, store) = reopen_damaged(|f, _| {
            let len = f.metadata().unwrap().len();
            std::io::Write::write_all(f, &vec![0; len as usize]).unwrap();
        });
        assert_replaced(&dir, &store);
        assert!(logs_contain("is damaged"));
        assert!(logs_contain("state.mdb.corrupt"));
    }

    #[test]
    fn store_with_zeroed_data_pages_is_replaced() {
        let (dir, store) = reopen_damaged(|f, page| {
            use std::io::{Seek, SeekFrom, Write};
            let len = f.metadata().unwrap().len();
            // Keep the two meta pages; zero every page they point to.
            f.seek(SeekFrom::Start(2 * page)).unwrap();
            f.write_all(&vec![0; (len - 2 * page) as usize]).unwrap();
        });
        assert_replaced(&dir, &store);
    }

    /// Spec "Damaged store is replaced at start", truncated: reading the
    /// store to check it would end the daemon with `SIGBUS`.
    // spectra-scenario: state-store / Truncated store at start
    #[test]
    #[tracing_test::traced_test]
    fn truncated_store_is_replaced() {
        let (dir, store) = reopen_damaged(|f, page| {
            let len = f.metadata().unwrap().len();
            assert!(len >= 3 * page, "a data page after the two meta pages");
            f.set_len(len - page).unwrap();
        });
        assert_replaced(&dir, &store);
        assert!(logs_contain("truncated"));
        assert!(logs_contain("state.mdb.corrupt"));
    }

    /// Commit `i` of a workload after which LMDB leaves the file shorter
    /// than it records now and then: records of varying size added and
    /// deleted again in one transaction. LMDB does not write pages freed
    /// in the transaction that allocated them, yet counts them in its last
    /// page number.
    fn churn(i: u64, txn: &mut RwTxn, db: &Database<Str, Str>) -> heed::Result<()> {
        db.put(txn, "meta/heartbeat", &i.to_string())?;
        let n = 50 + i * 37 % 400;
        let size = 300 + (i * 13 % 3000) as usize;
        for j in 0..n {
            db.put(
                txn,
                &format!("tmp/{j:05}"),
                &"z".repeat(size + j as usize % 50),
            )?;
        }
        for j in 0..n {
            db.delete(txn, &format!("tmp/{j:05}"))?;
        }
        for j in (i % 5..300).step_by(5) {
            db.put(
                txn,
                &format!("svc/{j:04}"),
                &"s".repeat(200 + (i % 9) as usize),
            )?;
        }
        Ok(())
    }

    fn churn_cfg(dir: &Path) -> StoreConfig {
        StoreConfig {
            map_size: 64 << 20,
            ..cfg(dir)
        }
    }

    /// Bytes LMDB records for `env`, and the data file's length.
    fn recorded_and_len(env: &Env<WithoutTls>, path: &Path) -> (u64, u64) {
        let recorded = (env.info().last_page_number as u64 + 1)
            * u64::from(env.unnamed_database_stat().page_size);
        (recorded, std::fs::metadata(path).unwrap().len())
    }

    /// Spec "Damaged store is replaced at start", scenario "Commits keep
    /// the file as long as LMDB records".
    // spectra-scenario: state-store / Commits keep the file as long as LMDB records
    #[test]
    fn commits_keep_the_file_as_long_as_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let c = churn_cfg(dir.path());
        let store = Store::open(&c).unwrap();
        for i in 0..1000 {
            store.write(|txn, db| churn(i, txn, db)).unwrap();
            let (recorded, len) = recorded_and_len(&store.env, &c.path);
            assert!(
                len >= recorded,
                "commit {i}: {len} bytes, LMDB records {recorded}"
            );
        }
    }

    /// Spec "Damaged store is replaced at start", scenario "Store left
    /// short by pages nobody reads is set aside": it cannot be told from a
    /// truncated one without reading past the end.
    // spectra-scenario: state-store / Store left short by pages nobody reads is set aside
    #[test]
    fn short_file_with_unread_pages_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let c = churn_cfg(dir.path());
        {
            // Plain LMDB, as an older daemon wrote the store.
            // SAFETY: only this test opens the file.
            let env = unsafe {
                let mut o = EnvOpenOptions::new().read_txn_without_tls();
                o.map_size(c.map_size);
                o.flags(EnvFlags::NO_SUB_DIR);
                o.open(&c.path).unwrap()
            };
            let db: Database<Str, Str> = env
                .create_databases_and_commit(env.write_txn().unwrap(), |o, w| -> heed::Result<_> {
                    Ok(CommitOrAbort::<_, ()>::Commit(
                        o.create_database::<Str, Str>(w, None)?,
                    ))
                })
                .unwrap()
                .unwrap_commit();
            let mut short = false;
            for i in 0..2000 {
                let mut txn = env.write_txn().unwrap();
                churn(i, &mut txn, &db).unwrap();
                txn.commit().unwrap();
                let (recorded, len) = recorded_and_len(&env, &c.path);
                if len < recorded {
                    short = true;
                    break;
                }
            }
            if !short {
                // Whether the file ends up short depends on the page size
                // and the file system: on macOS (16 KiB pages) this workload
                // never leaves it short, on Linux and FreeBSD it does.
                if cfg!(target_os = "macos") {
                    eprintln!("skipped: this workload does not leave the file short here");
                    return;
                }
                panic!("the workload leaves the file short");
            }
        }
        let store = Store::open(&c).unwrap();
        assert_replaced(&dir, &store);
    }

    #[test]
    fn value_that_is_not_utf8_is_not_damage() {
        let dir = tempfile::tempdir().unwrap();
        let c = cfg(dir.path());
        {
            let store = Store::open(&c).unwrap();
            let raw = store.db.remap_types::<Str, Bytes>();
            let mut txn = store.env.write_txn().unwrap();
            raw.put(&mut txn, "hist/a", &[0xff, 0xfe]).unwrap();
            txn.commit().unwrap();
        }
        let store = Store::open(&c).unwrap();
        assert!(!dir.path().join("state.mdb.corrupt").exists());
        assert!(store.get("hist/a").is_err(), "left to the reader");
    }

    #[test]
    fn healthy_store_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let c = cfg(dir.path());
        Store::open(&c)
            .unwrap()
            .write(|txn, db| db.put(txn, "hist/a", "[]"))
            .unwrap();
        let store = Store::open(&c).unwrap();
        assert_eq!(store.get("hist/a").unwrap().as_deref(), Some("[]"));
        assert!(!dir.path().join("state.mdb.corrupt").exists());
    }

    // spectra-scenario: state-store / Second daemon refused
    #[test]
    fn second_writer_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let c = cfg(dir.path());
        let _first = Store::open(&c).unwrap();
        let err = Store::open(&c).unwrap_err();
        assert!(matches!(err, StoreError::Locked(_)));
        assert!(
            err.to_string()
                .contains(&format!("{}.writer", c.path.display())),
            "{err}"
        );
    }

    /// Write `value` under `key` as raw bytes, as a damaged or foreign
    /// writer might have.
    pub(crate) fn put_raw(store: &Store, key: &[u8], value: &[u8]) {
        let raw = store.db.remap_types::<Bytes, Bytes>();
        let mut txn = store.env.write_txn().unwrap();
        raw.put(&mut txn, key, value).unwrap();
        txn.commit().unwrap();
    }

    /// A key or value that is not UTF-8 is deleted like any other.
    #[test]
    fn delete_prefix_removes_keys_and_values_that_are_not_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&cfg(dir.path())).unwrap();
        put_raw(&store, b"svc/a", &[0xff, 0xfe]);
        put_raw(&store, b"svc/\xff", b"1");
        put_raw(&store, b"svc/b", b"2");
        put_raw(&store, b"meta/x", b"3");
        store
            .write(|txn, db| delete_prefix(txn, db, "svc/"))
            .unwrap();
        assert_eq!(
            store.dump().unwrap().into_iter().collect::<Vec<_>>(),
            vec![("meta/x".into(), "3".into())]
        );
    }

    #[test]
    fn write_and_delete_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&cfg(dir.path())).unwrap();
        store
            .write(|txn, db| {
                db.put(txn, "svc/a", "1")?;
                db.put(txn, "svc/b", "2")?;
                db.put(txn, "meta/x", "3")
            })
            .unwrap();
        store
            .write(|txn, db| delete_prefix(txn, db, "svc/"))
            .unwrap();
        assert_eq!(
            store.dump().unwrap().into_iter().collect::<Vec<_>>(),
            vec![("meta/x".into(), "3".into())]
        );
    }
}
