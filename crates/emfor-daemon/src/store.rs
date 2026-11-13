//! LMDB state store: file layout, writer lock, and low-level access.
//!
//! Layout (format version 1, see `emfor_format`): one data file opened with
//! `MDB_NOSUBDIR` plus `<path>-lock`, the unnamed default database only, UTF-8
//! keys and values. A separate `<path>.writer` file is `flock`ed so that only
//! one daemon writes a given store.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use heed::types::{Bytes, Str};
use heed::{Database, Env, EnvFlags, EnvOpenOptions, RwTxn, WithoutTls};
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
    db: Database<Str, Str>,
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
            .open(&lock_path)
            .map_err(io(&lock_path))?;
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

        let (env, db) = match open_env(cfg) {
            Err(e) if is_damage(&e) => {
                let corrupt = set_aside(&cfg.path).map_err(io(&cfg.path))?;
                tracing::warn!(
                    "state store {} is damaged ({e}); moved it to {} and starting with an empty store",
                    cfg.path.display(),
                    corrupt.display()
                );
                open_env(cfg)?
            }
            r => r?,
        };
        std::fs::set_permissions(&cfg.path, Permissions::from_mode(cfg.file_mode))
            .map_err(io(&cfg.path))?;
        let lmdb_lock = lock_file_path(&cfg.path);
        std::fs::set_permissions(&lmdb_lock, Permissions::from_mode(cfg.lock_file_mode))
            .map_err(io(&lmdb_lock))?;

        let store = Store {
            env,
            db,
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
        let page = u64::from(self.env.stat().page_size);
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
        Ok(r)
    }

    pub fn get(&self, key: &str) -> heed::Result<Option<String>> {
        let rtxn = self.env.read_txn()?;
        Ok(self.db.get(&rtxn, key)?.map(str::to_string))
    }

    /// Every key starting with `prefix` and its value, from one read transaction.
    pub fn prefix(&self, prefix: &str) -> heed::Result<BTreeMap<String, String>> {
        let rtxn = self.env.read_txn()?;
        let mut out = BTreeMap::new();
        for item in self.db.prefix_iter(&rtxn, prefix)? {
            let (k, v) = item?;
            out.insert(k.to_string(), v.to_string());
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

/// Open the LMDB environment and read every key once, so damage anywhere in
/// the file shows up now rather than in the first commit.
fn open_env(cfg: &StoreConfig) -> heed::Result<(Env<WithoutTls>, Database<Str, Str>)> {
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
        // lock) and readers only read; nothing truncates it.
        opts.open(&cfg.path)?
    };
    let check = || -> heed::Result<Database<Str, Str>> {
        let rtxn = env.read_txn()?;
        let db = env
            .open_database::<Str, Str>(&rtxn, None)?
            .expect("LMDB always has the unnamed default database, so opening it returns Some");
        // As raw bytes: only LMDB's own structure is checked here. A value
        // that is not UTF-8 is left to whoever reads it (`load_history`
        // discards it with a warning; `svc/*` is rewritten at start).
        for item in db.remap_types::<Bytes, Bytes>().iter(&rtxn)? {
            item?;
        }
        rtxn.commit()?;
        Ok(db)
    };
    // On error `env` is dropped here, closing the file before it is moved.
    let db = check()?;
    Ok((env, db))
}

/// LMDB errors that mean the file itself is damaged, as after a power loss
/// with `MDB_NOSYNC`. `MDB_INVALID` is also what a file that is not LMDB at
/// all gives, which is why the file is moved aside, not deleted.
fn is_damage(e: &heed::Error) -> bool {
    use heed::MdbError;
    matches!(
        e,
        heed::Error::Mdb(MdbError::Corrupted | MdbError::PageNotFound | MdbError::Invalid)
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

/// Delete every key starting with `prefix`.
pub fn delete_prefix(txn: &mut RwTxn, db: &Database<Str, Str>, prefix: &str) -> heed::Result<()> {
    let mut iter = db.prefix_iter_mut(txn, prefix)?;
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
            [("hist/a".into(), "1".into()), ("hist/b".into(), "2".into())]
        );
    }

    #[test]
    fn creates_files_with_configured_modes() {
        let dir = tempfile::tempdir().unwrap();
        let c = cfg(dir.path());
        let _store = Store::open(&c).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&c.path), 0o644);
        assert_eq!(mode(&dir.path().join("state.mdb-lock")), 0o666);
        assert!(
            std::fs::metadata(&c.path).unwrap().is_file(),
            "single file, not a directory"
        );
    }

    #[test]
    fn custom_modes() {
        let dir = tempfile::tempdir().unwrap();
        let c = StoreConfig {
            file_mode: 0o640,
            lock_file_mode: 0o660,
            ..cfg(dir.path())
        };
        let _store = Store::open(&c).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&c.path), 0o640);
        assert_eq!(mode(&lock_file_path(&c.path)), 0o660);
    }

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
            u64::from(store.env.stat().page_size)
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
