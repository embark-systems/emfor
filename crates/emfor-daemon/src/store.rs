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

use heed::types::Str;
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

        let env = unsafe {
            let mut opts = EnvOpenOptions::new().read_txn_without_tls();
            opts.map_size(cfg.map_size);
            opts.max_readers(emfor_format::MAX_READERS);
            opts.flags(EnvFlags::NO_SUB_DIR);
            // SAFETY: only this daemon writes the file (guarded by the writer
            // lock above) and readers only read; nothing truncates it.
            opts.open(&cfg.path)?
        };
        std::fs::set_permissions(&cfg.path, Permissions::from_mode(cfg.file_mode))
            .map_err(io(&cfg.path))?;
        let lmdb_lock = lock_file_path(&cfg.path);
        std::fs::set_permissions(&lmdb_lock, Permissions::from_mode(cfg.lock_file_mode))
            .map_err(io(&lmdb_lock))?;

        let rtxn = env.read_txn()?;
        let db = env
            .open_database::<Str, Str>(&rtxn, None)?
            .expect("the unnamed LMDB database always exists");
        rtxn.commit()?;
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
