//! The record of the secrets a daemon has used (spec peer-group "Peer secret
//! reload"): a small file next to the store that holds a fingerprint of the
//! current secret, and, once the daemon has replaced one, the fingerprint of
//! the one it replaced and when. At start it tells which secret
//! `previous_secret_file` may hold to be used as the one that was replaced;
//! a secret that was retired earlier, or that the daemon never used, is not
//! used again because a file holds it or `secret_file` was touched.
//!
//! Only SHA-256 fingerprints are stored, never a secret.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

use super::crypto::ACCEPT_PREVIOUS_FOR;

/// How often a failure to write the record is logged at most.
const WARN_EVERY: Duration = Duration::from_secs(60);

/// Hex SHA-256 of a secret.
pub fn fingerprint(secret: &[u8]) -> String {
    let mut out = String::with_capacity(64);
    for b in Sha256::digest(secret) {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The record file of a store: the store's path with `-peers` appended, as
/// `-lock` is for the lock file.
pub fn path_of(store: &Path) -> PathBuf {
    let mut p = store.as_os_str().to_owned();
    p.push("-peers");
    PathBuf::from(p)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Fingerprint of the secret the daemon last ran with.
    pub current: String,
    /// Fingerprint of the secret it replaced, and when (Unix seconds).
    pub replaced: Option<(String, u64)>,
}

fn is_fingerprint(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Record {
    pub fn render(&self) -> String {
        let mut out = format!("current={}\n", self.current);
        if let Some((fp, at)) = &self.replaced {
            let _ = writeln!(out, "replaced={fp}\nchanged_at={at}");
        }
        out
    }

    /// `None` for anything that is not a record.
    pub fn parse(text: &str) -> Option<Record> {
        let (mut current, mut replaced, mut at) = (None, None, None);
        for line in text.lines() {
            let (key, value) = line.split_once('=')?;
            match key {
                "current" => current = Some(value.to_string()),
                "replaced" => replaced = Some(value.to_string()),
                "changed_at" => at = Some(value.parse::<u64>().ok()?),
                _ => return None,
            }
        }
        let current = current.filter(|c| is_fingerprint(c))?;
        let replaced = match (replaced, at) {
            (Some(fp), Some(at)) if is_fingerprint(&fp) => Some((fp, at)),
            (None, None) => None,
            _ => return None,
        };
        Some(Record { current, replaced })
    }

    /// The record at `path`, or `None` when it is missing or not a record.
    pub fn load(path: &Path) -> Option<Record> {
        Record::parse(&std::fs::read_to_string(path).ok()?)
    }

    /// Write the record: to a file of its own, then moved over `path`, so
    /// that it is never half written. Mode 0600.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".new");
        let tmp = PathBuf::from(tmp);
        let _ = std::fs::remove_file(&tmp);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(self.render().as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
}

/// Why `previous_secret_file` was not used.
#[derive(Debug, PartialEq, Eq)]
pub enum Ignored {
    /// There was no record, or it was not one: the first start after an
    /// upgrade, or the file is gone.
    NoRecord,
    /// The record does not say that the daemon replaced this secret, or
    /// that was more than [`ACCEPT_PREVIOUS_FOR`] ago.
    NotReplaced,
}

/// What a start does with the record.
pub struct Resumed {
    /// The record to hold from now on.
    pub record: Record,
    /// The previous secret to use, and how long ago it was replaced.
    pub previous: Option<Duration>,
    pub ignored: Option<Ignored>,
}

/// Decide, at start, whether `previous` (the secret in `previous_secret_file`,
/// when one is set) is the secret the daemon replaced less than
/// [`ACCEPT_PREVIOUS_FOR`] ago. `modified` is when `secret_file` was last
/// modified, which stands for the time of a change made while the daemon
/// was not running; times are Unix seconds.
pub fn resume(
    loaded: Option<Record>,
    current: &[u8],
    previous: Option<&[u8]>,
    modified: u64,
    now: u64,
) -> Resumed {
    let fp = fingerprint(current);
    let had_record = loaded.is_some();
    let record = match loaded {
        Some(r) if r.current == fp => r,
        Some(r) => Record {
            current: fp.clone(),
            replaced: Some((r.current, modified)),
        },
        None => Record {
            current: fp.clone(),
            replaced: None,
        },
    };
    let Some(previous) = previous else {
        return Resumed {
            record,
            previous: None,
            ignored: None,
        };
    };
    let prev_fp = fingerprint(previous);
    // A time ahead of the clock counts as now.
    let ago = record
        .replaced
        .as_ref()
        .filter(|(replaced, _)| *replaced == prev_fp && prev_fp != fp)
        .map(|(_, at)| Duration::from_secs(now.saturating_sub(*at)))
        .filter(|ago| *ago < ACCEPT_PREVIOUS_FOR);
    let ignored = match (ago, prev_fp == fp, had_record) {
        (Some(_), ..) => None,
        // The same secret as the current one: nothing to resume.
        (None, true, _) => None,
        (None, false, false) => Some(Ignored::NoRecord),
        (None, false, true) => Some(Ignored::NotReplaced),
    };
    Resumed {
        record,
        previous: ago,
        ignored,
    }
}

/// Unix seconds now.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The record a running daemon holds and writes when its secret changes.
/// Writes are made on a thread of their own (a directory that is slow must
/// not stop the peer loop), and each writes the latest record, so that two
/// changes in a row end with the newer one on disk.
pub struct RecordFile {
    path: PathBuf,
    /// The record to write; held only to read or replace it, never while a
    /// file is written, so that the peer loop (which replaces it) does not
    /// wait for a slow directory.
    latest: Mutex<Record>,
    /// Held by the one that writes, so that two writes do not overlap; the
    /// peer loop never takes it.
    writing: Mutex<()>,
    warned: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for RecordFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordFile")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl RecordFile {
    pub fn new(path: PathBuf, record: Record) -> Arc<RecordFile> {
        Arc::new(RecordFile {
            path,
            latest: Mutex::new(record),
            writing: Mutex::new(()),
            warned: Mutex::new(None),
        })
    }

    /// The secret `old` was replaced by `new` at Unix second `at`.
    pub fn replaced(self: &Arc<Self>, old: &[u8], new: &[u8], at: u64) {
        {
            let mut latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
            *latest = Record {
                current: fingerprint(new),
                replaced: Some((fingerprint(old), at)),
            };
        }
        self.write_in_background();
    }

    /// Write the record held now, on a thread of its own.
    pub fn write_in_background(self: &Arc<Self>) {
        let me = Arc::clone(self);
        std::thread::spawn(move || me.write());
    }

    /// Write the record held now.
    pub fn write(&self) {
        let _one_at_a_time = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        let latest = self
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Err(e) = latest.save(&self.path) {
            let mut warned = self.warned.lock().unwrap_or_else(|e| e.into_inner());
            if warned.is_none_or(|w| w.elapsed() >= WARN_EVERY) {
                *warned = Some(Instant::now());
                tracing::warn!(
                    "cannot write {}: {e}; previous_secret_file is ignored at the next start",
                    self.path.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const K0: &[u8] = &[b'a'; 40];
    const K1: &[u8] = &[b'b'; 40];
    const K2: &[u8] = &[b'c'; 40];

    fn rec(current: &[u8], replaced: Option<(&[u8], u64)>) -> Record {
        Record {
            current: fingerprint(current),
            replaced: replaced.map(|(k, at)| (fingerprint(k), at)),
        }
    }

    #[test]
    fn fingerprint_is_sha256_hex() {
        assert_eq!(
            fingerprint(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn path_is_the_store_path_with_peers_appended() {
        assert_eq!(
            path_of(Path::new("/var/lib/emfor/state.mdb")),
            Path::new("/var/lib/emfor/state.mdb-peers")
        );
    }

    #[test]
    fn record_is_written_and_read_back_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("state.mdb-peers");
        let r = rec(K1, Some((K0, 1_763_000_000)));
        r.save(&path).expect("the record can be written");
        assert_eq!(Record::load(&path), Some(r.clone()));
        let mode = std::fs::metadata(&path)
            .expect("the file exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let text = std::fs::read_to_string(&path).expect("the file can be read");
        assert!(!text.contains("bbbbbbbb"), "no secret in the record");
        // Written again over the old one.
        let r2 = rec(K2, Some((K1, 1_763_000_100)));
        r2.save(&path).expect("the record can be written again");
        assert_eq!(Record::load(&path), Some(r2));
    }

    #[test]
    fn missing_or_garbled_record_is_none() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("rec");
        assert_eq!(Record::load(&path), None);
        for text in [
            "",
            "current=zz\n",
            "hello\n",
            &format!(
                "current={}\nreplaced={}\n",
                fingerprint(K0),
                fingerprint(K1)
            ),
            &format!("current={}\nchanged_at=5\n", fingerprint(K0)),
            &format!("current={}\nextra=1\n", fingerprint(K0)),
        ] {
            std::fs::write(&path, text).expect("the file can be written");
            assert_eq!(Record::load(&path), None, "{text:?}");
        }
    }

    /// Spec peer-group "Restart during a secret change": the daemon ran with
    /// K0, replaced it by K1 30 seconds ago, and restarts.
    #[test]
    fn secret_replaced_by_this_daemon_is_resumed() {
        let now = 10_000;
        let r = resume(
            Some(rec(K1, Some((K0, now - 30)))),
            K1,
            Some(K0),
            now - 500,
            now,
        );
        assert_eq!(r.previous, Some(Duration::from_secs(30)));
        assert_eq!(r.ignored, None);
        assert_eq!(r.record, rec(K1, Some((K0, now - 30))));
        // The 120 seconds are over.
        let r = resume(
            Some(rec(K1, Some((K0, now - 120)))),
            K1,
            Some(K0),
            now - 500,
            now,
        );
        assert_eq!(r.previous, None);
        assert_eq!(r.ignored, Some(Ignored::NotReplaced));
    }

    /// Spec peer-group "Stop, change the secret, start": the record names
    /// K0 as current, `secret_file` now holds K1, and was modified 30
    /// seconds ago.
    #[test]
    fn secret_changed_while_stopped_is_resumed_from_the_file_time() {
        let now = 10_000;
        let r = resume(Some(rec(K0, None)), K1, Some(K0), now - 30, now);
        assert_eq!(r.previous, Some(Duration::from_secs(30)));
        assert_eq!(r.record, rec(K1, Some((K0, now - 30))));
        // Modified long ago: not resumed, but the record is brought up to date.
        let r = resume(Some(rec(K0, None)), K1, Some(K0), now - 500, now);
        assert_eq!(r.previous, None);
        assert_eq!(r.record, rec(K1, Some((K0, now - 500))));
    }

    /// Spec peer-group "Previous secret retired by an earlier rotation":
    /// the file still holds K0, the record says K1 was replaced by K2.
    #[test]
    fn secret_retired_earlier_is_not_used_again() {
        let now = 10_000;
        let r = resume(
            Some(rec(K2, Some((K1, now - 30)))),
            K2,
            Some(K0),
            now - 30,
            now,
        );
        assert_eq!(r.previous, None);
        assert_eq!(r.ignored, Some(Ignored::NotReplaced));
        // The same when the stop was during the change K1 to K2.
        let r = resume(Some(rec(K1, None)), K2, Some(K0), now - 30, now);
        assert_eq!(r.previous, None);
        assert_eq!(r.ignored, Some(Ignored::NotReplaced));
    }

    /// Spec peer-group "Secret file touched": same content, modified now.
    #[test]
    fn touching_the_secret_file_changes_nothing() {
        let now = 10_000;
        let old = rec(K1, Some((K0, now - 1000)));
        let r = resume(Some(old.clone()), K1, Some(K0), now, now);
        assert_eq!(r.previous, None);
        assert_eq!(r.record, old, "the record is kept");
    }

    /// Spec peer-group "First start after an upgrade".
    #[test]
    fn no_record_means_no_previous_secret() {
        let now = 10_000;
        let r = resume(None, K1, Some(K0), now - 30, now);
        assert_eq!(r.previous, None);
        assert_eq!(r.ignored, Some(Ignored::NoRecord));
        assert_eq!(r.record, rec(K1, None));
        // Nothing to say without previous_secret_file.
        let r = resume(None, K1, None, now - 30, now);
        assert_eq!(r.ignored, None);
        // The same secret in both files is not a change.
        let r = resume(Some(rec(K1, None)), K1, Some(K1), now - 30, now);
        assert_eq!((r.previous, r.ignored), (None, None));
    }

    /// A write that is stuck does not stop `replaced`, which the peer loop
    /// calls.
    #[test]
    fn replaced_does_not_wait_for_a_write_in_progress() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let file = RecordFile::new(dir.path().join("rec"), rec(K0, None));
        // As a write that has not finished holds it.
        let stuck = file.writing.lock().expect("the lock is free");
        let (tx, rx) = std::sync::mpsc::channel();
        let me = Arc::clone(&file);
        std::thread::spawn(move || {
            me.replaced(K0, K1, 5);
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("replaced returned while a write was stuck");
        drop(stuck);
    }

    #[test]
    fn a_change_made_after_the_record_is_written_in_the_background() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("rec");
        let file = RecordFile::new(path.clone(), rec(K0, None));
        file.replaced(K0, K1, 77);
        file.replaced(K1, K2, 88);
        let end = Instant::now() + Duration::from_secs(5);
        while Record::load(&path) != Some(rec(K2, Some((K1, 88)))) {
            assert!(Instant::now() < end, "the latest record was not written");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A rotation of the running daemon is recorded.
    #[test]
    fn rotation_of_the_keys_is_recorded() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("rec");
        let file = RecordFile::new(path.clone(), rec(K0, None));
        let mut keys = super::super::crypto::Keys::new(K0.to_vec()).with_record(file);
        assert!(keys.rotate(K1.to_vec(), tokio::time::Instant::now()));
        let end = Instant::now() + Duration::from_secs(5);
        let wanted = |r: &Record| {
            r.current == fingerprint(K1)
                && r.replaced.as_ref().map(|x| &x.0) == Some(&fingerprint(K0))
        };
        while !Record::load(&path).is_some_and(|r| wanted(&r)) {
            assert!(Instant::now() < end, "the rotation was not recorded");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    #[tracing_test::traced_test]
    fn failure_to_write_is_warned_about_once_a_minute() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("no-such-directory").join("rec");
        let file = RecordFile::new(path, rec(K0, None));
        file.write();
        file.write();
        logs_assert(|lines: &[&str]| {
            match lines.iter().filter(|l| l.contains("cannot write")).count() {
                1 => Ok(()),
                n => Err(format!("{n} warnings")),
            }
        });
    }
}
