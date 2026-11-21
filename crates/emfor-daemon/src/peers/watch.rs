//! A watch on the directory that holds a secret file, so that a change of
//! the file is seen when it happens and not at the next poll.
//!
//! A change is a write in place or another file renamed over the secret
//! file (how a secret is usually replaced); the directory is watched, not
//! only the file, because a watch on the file's own inode ends when the
//! file is replaced. An event only means "the file may have changed": the
//! caller reads the file and compares the content, so an event for another
//! file of the directory costs one read of a small file and nothing else.
//!
//! Linux uses inotify and FreeBSD and macOS use kqueue (the directory, and
//! the file itself, because a kqueue event of a directory does not tell of
//! a write to a file in it). Anywhere else, and wherever the watch cannot be
//! set up, [`SecretWatch::start`] fails and the caller reads the file on
//! every heartbeat instead. A file system that makes no event on this
//! machine for a change made elsewhere (a network file system) is not
//! helped by a watch; the caller's periodic read covers it.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

/// How long the watcher thread waits for an event before it looks at the
/// stop flag: the longest a dropped watch's thread lives on.
const WAIT: Duration = Duration::from_secs(1);

/// A running watch. Dropping it stops the thread within [`WAIT`].
pub struct SecretWatch {
    changed: Arc<Notify>,
    stop: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
}

impl SecretWatch {
    /// Start watching the directory of `path`. The watch exists when this
    /// returns, so a change made after that is not missed.
    pub fn start(path: &Path) -> Result<Self, String> {
        let mut backend = Backend::new(path)?;
        let changed = Arc::new(Notify::new());
        let stop = Arc::new(AtomicBool::new(false));
        let failure = Arc::new(Mutex::new(None));
        let (c, s, f) = (changed.clone(), stop.clone(), failure.clone());
        std::thread::Builder::new()
            .name("emfor-secret-watch".into())
            .spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    match backend.wait(WAIT) {
                        Ok(true) => c.notify_one(),
                        Ok(false) => {}
                        Err(e) => {
                            *f.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
                            c.notify_one();
                            return;
                        }
                    }
                }
            })
            .map_err(|e| format!("cannot start the watch thread: {e}"))?;
        Ok(Self {
            changed,
            stop,
            failure,
        })
    }

    /// Notified when the file may have changed (once for any number of
    /// events since the last wait) and when the watch stopped working.
    pub fn notifier(&self) -> Arc<Notify> {
        self.changed.clone()
    }

    /// Why the watch stopped working, if it did (the directory was removed,
    /// say); it then produces no more events.
    pub fn failure(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

impl Drop for SecretWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The directory to watch for a file: its parent, or the working directory
/// for a bare file name.
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
fn directory_of(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

#[cfg(target_os = "linux")]
use inotify_backend::Backend;
#[cfg(target_os = "linux")]
mod inotify_backend {
    use std::os::fd::AsFd;
    use std::path::Path;
    use std::time::Duration;

    use nix::errno::Errno;
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};

    pub struct Backend {
        inotify: Inotify,
    }

    impl Backend {
        pub fn new(path: &Path) -> Result<Self, String> {
            let inotify = Inotify::init(InitFlags::IN_CLOEXEC | InitFlags::IN_NONBLOCK)
                .map_err(|e| format!("inotify_init: {e}"))?;
            let dir = super::directory_of(path);
            // A file is complete when it is closed after a write, so a
            // write in place is not read half way (no IN_MODIFY).
            inotify
                .add_watch(
                    dir,
                    AddWatchFlags::IN_CLOSE_WRITE
                        | AddWatchFlags::IN_MOVED_TO
                        | AddWatchFlags::IN_MOVED_FROM
                        | AddWatchFlags::IN_CREATE
                        | AddWatchFlags::IN_DELETE
                        | AddWatchFlags::IN_ATTRIB
                        | AddWatchFlags::IN_DELETE_SELF
                        | AddWatchFlags::IN_MOVE_SELF,
                )
                .map_err(|e| format!("inotify_add_watch {}: {e}", dir.display()))?;
            Ok(Self { inotify })
        }

        /// Wait for events; true when there were some.
        pub fn wait(&mut self, limit: Duration) -> Result<bool, String> {
            let mut fds = [PollFd::new(self.inotify.as_fd(), PollFlags::POLLIN)];
            let timeout = PollTimeout::try_from(limit).unwrap_or(PollTimeout::MAX);
            match poll(&mut fds, timeout) {
                Ok(0) => return Ok(false),
                Ok(_) => {}
                Err(Errno::EINTR) => return Ok(false),
                Err(e) => return Err(format!("poll: {e}")),
            }
            match self.inotify.read_events() {
                Ok(events) => {
                    // The watch is gone when its directory was removed, and
                    // the queue overflowed when events were lost: the
                    // second only means "read".
                    if events.iter().any(|e| {
                        e.mask.intersects(
                            AddWatchFlags::IN_IGNORED
                                | AddWatchFlags::IN_DELETE_SELF
                                | AddWatchFlags::IN_MOVE_SELF
                                | AddWatchFlags::IN_UNMOUNT,
                        )
                    }) {
                        return Err("the watched directory is gone".into());
                    }
                    Ok(!events.is_empty())
                }
                Err(Errno::EAGAIN) => Ok(false),
                Err(e) => Err(format!("read of inotify events: {e}")),
            }
        }
    }
}

#[cfg(any(
    target_os = "freebsd",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
use kqueue_backend::Backend;
#[cfg(any(
    target_os = "freebsd",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
mod kqueue_backend {
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};

    /// After an event, wait this long for the writer to be done (kqueue
    /// reports every write, not the close), and take what came meanwhile
    /// as the same change.
    const SETTLE: Duration = Duration::from_millis(50);

    pub struct Backend {
        kq: Kqueue,
        file_path: PathBuf,
        /// Open only to be watched; closing it ends its watch.
        dir: File,
        file: Option<File>,
    }

    /// Open for watching. O_NONBLOCK: a path that was replaced by a FIFO
    /// must not make the open wait for a writer; it has no effect on a
    /// regular file or a directory.
    fn open_nonblocking(path: &Path) -> std::io::Result<File> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
    }

    fn vnode(fd: &File, flags: FilterFlag) -> KEvent {
        KEvent::new(
            fd.as_raw_fd() as usize,
            EventFilter::EVFILT_VNODE,
            EvFlags::EV_ADD | EvFlags::EV_CLEAR,
            flags,
            0,
            0,
        )
    }

    fn timespec(d: Duration) -> libc::timespec {
        libc::timespec {
            tv_sec: d.as_secs() as _,
            tv_nsec: d.subsec_nanos() as _,
        }
    }

    impl Backend {
        pub fn new(path: &Path) -> Result<Self, String> {
            let kq = Kqueue::new().map_err(|e| format!("kqueue: {e}"))?;
            let dir_path = super::directory_of(path);
            let dir = open_nonblocking(dir_path)
                .map_err(|e| format!("open {}: {e}", dir_path.display()))?;
            let mut me = Self {
                kq,
                file_path: path.to_path_buf(),
                dir,
                file: None,
            };
            me.register()?;
            Ok(me)
        }

        /// Watch the directory, and the file when it exists (open it again
        /// whenever this is called: the file may be another one now).
        fn register(&mut self) -> Result<(), String> {
            let mut changes = vec![vnode(
                &self.dir,
                FilterFlag::NOTE_WRITE
                    | FilterFlag::NOTE_DELETE
                    | FilterFlag::NOTE_RENAME
                    | FilterFlag::NOTE_ATTRIB
                    | FilterFlag::NOTE_LINK
                    | FilterFlag::NOTE_REVOKE,
            )];
            self.file = open_nonblocking(&self.file_path).ok();
            if let Some(f) = &self.file {
                changes.push(vnode(
                    f,
                    FilterFlag::NOTE_WRITE
                        | FilterFlag::NOTE_EXTEND
                        | FilterFlag::NOTE_ATTRIB
                        | FilterFlag::NOTE_DELETE
                        | FilterFlag::NOTE_RENAME
                        | FilterFlag::NOTE_LINK
                        | FilterFlag::NOTE_REVOKE,
                ));
            }
            let mut none: [KEvent; 0] = [];
            self.kq
                .kevent(&changes, &mut none, Some(timespec(Duration::ZERO)))
                .map_err(|e| format!("kevent: {e}"))?;
            Ok(())
        }

        fn poll(&self, limit: Duration) -> Result<Vec<KEvent>, String> {
            let blank = KEvent::new(
                0,
                EventFilter::EVFILT_VNODE,
                EvFlags::empty(),
                FilterFlag::empty(),
                0,
                0,
            );
            let mut out = [blank; 8];
            let n = match self.kq.kevent(&[], &mut out, Some(timespec(limit))) {
                Ok(n) => n,
                Err(nix::errno::Errno::EINTR) => 0,
                Err(e) => return Err(format!("kevent: {e}")),
            };
            Ok(out[..n].to_vec())
        }

        /// An error when the directory itself was removed or renamed: the
        /// watch is useless then.
        fn check_dir(&self, events: &[KEvent]) -> Result<(), String> {
            let dir_fd = self.dir.as_raw_fd() as usize;
            if events.iter().any(|e| {
                e.ident() == dir_fd
                    && e.fflags().intersects(
                        FilterFlag::NOTE_DELETE | FilterFlag::NOTE_RENAME | FilterFlag::NOTE_REVOKE,
                    )
            }) {
                return Err("the watched directory is gone".into());
            }
            Ok(())
        }

        /// Wait for events; true when there were some.
        pub fn wait(&mut self, limit: Duration) -> Result<bool, String> {
            let events = self.poll(limit)?;
            if events.is_empty() {
                return Ok(false);
            }
            self.check_dir(&events)?;
            std::thread::sleep(SETTLE);
            // What came during the settle may be the removal of the
            // directory, after events of the file only.
            let more = self.poll(Duration::ZERO)?;
            self.check_dir(&more)?;
            // The file may be a new one (renamed over, or created): watch
            // what is there now.
            self.register()?;
            Ok(true)
        }
    }
}

/// No file watching on this platform.
#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
)))]
struct Backend;

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
)))]
impl Backend {
    fn new(_: &Path) -> Result<Self, String> {
        Err("this platform has no file watching".into())
    }

    fn wait(&mut self, _: Duration) -> Result<bool, String> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Wait (real time) for a change to be notified.
    async fn notified(w: &SecretWatch) -> bool {
        tokio::time::timeout(Duration::from_secs(3), w.notifier().notified())
            .await
            .is_ok()
    }

    /// Nothing is notified for a while (a settled directory).
    async fn quiet(w: &SecretWatch) -> bool {
        tokio::time::timeout(Duration::from_millis(400), w.notifier().notified())
            .await
            .is_err()
    }

    /// A write in place is noticed.
    #[tokio::test]
    async fn write_in_place_is_noticed() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("secret");
        fs::write(&path, [1u8; 40]).expect("the secret file can be written");
        let w = SecretWatch::start(&path).expect("the directory can be watched");
        assert!(quiet(&w).await, "nothing changed yet");
        fs::write(&path, [2u8; 40]).expect("the secret file can be overwritten");
        assert!(notified(&w).await, "a write in place is noticed");
    }

    /// Another file renamed over the secret file is noticed, and so is a
    /// second one: the watch goes on after the file was replaced.
    #[tokio::test]
    async fn rename_over_the_file_is_noticed_each_time() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("secret");
        fs::write(&path, [1u8; 40]).expect("the secret file can be written");
        let w = SecretWatch::start(&path).expect("the directory can be watched");
        for n in 2u8..=3 {
            let new = dir.path().join("secret.new");
            fs::write(&new, [n; 40]).expect("the new file can be written");
            fs::rename(&new, &path).expect("the new file can replace the secret file");
            assert!(notified(&w).await, "rename number {n} is noticed");
            // The create, the close and the rename can come as several
            // notifications; it settles.
            let mut settled = false;
            for _ in 0..10 {
                if quiet(&w).await {
                    settled = true;
                    break;
                }
            }
            assert!(settled, "and then it is quiet");
        }
        // The file itself written in place after it was replaced.
        fs::write(&path, [9u8; 40]).expect("the secret file can be overwritten");
        assert!(
            notified(&w).await,
            "a write after the replacements is noticed"
        );
    }

    /// The secret file does not exist yet: its creation is noticed.
    #[tokio::test]
    async fn creation_is_noticed() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("secret");
        let w = SecretWatch::start(&path).expect("the directory can be watched");
        fs::write(&path, [1u8; 40]).expect("the secret file can be created");
        assert!(notified(&w).await);
    }

    /// A directory that does not exist cannot be watched; the caller reads
    /// the file on every heartbeat instead.
    #[test]
    fn missing_directory_cannot_be_watched() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let err = SecretWatch::start(&dir.path().join("none").join("secret"))
            .err()
            .expect("a missing directory cannot be watched");
        assert!(!err.is_empty());
    }

    /// The watch stops working when its directory is removed, and says so.
    #[tokio::test]
    async fn removed_directory_is_a_failure() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).expect("the directory can be created");
        let path = sub.join("secret");
        fs::write(&path, [1u8; 40]).expect("the secret file can be written");
        let w = SecretWatch::start(&path).expect("the directory can be watched");
        fs::remove_dir_all(&sub).expect("the directory can be removed");
        assert!(notified(&w).await);
        assert!(w.failure().is_some(), "the watch reports its failure");
    }
}
