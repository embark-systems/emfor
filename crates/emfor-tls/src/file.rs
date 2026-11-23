//! Reading the files that options name (passwords, secrets, keys, CA
//! bundles) without waiting on them and without reading an unbounded file.
//!
//! A path that names a FIFO would make a plain read wait for a writer, and a
//! path pointed at a log would read all of it. Both are found here: the file
//! is opened without waiting, its type is taken from the file that was
//! opened (not from the name, which could be pointed elsewhere in between),
//! and at most one byte more than the limit is read.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Most a `password_file` or a RADIUS `secret_file` may hold.
pub const MAX_PASSWORD_FILE: u64 = 64 * 1024;
/// Most a `tls_cert_file` or a `tls_key_file` may hold.
pub const MAX_TLS_FILE: u64 = 1024 * 1024;
/// Most a `ca_file` may hold.
pub const MAX_CA_FILE: u64 = 8 * 1024 * 1024;

/// Why a file could not be read. It never holds the file's content.
#[derive(Debug)]
pub enum FileError {
    /// The file could not be opened or read.
    Io(io::Error),
    /// The path names a FIFO, a device, a directory or another file that is
    /// not a regular one (after following symbolic links).
    NotRegular,
    /// The file holds more than the limit.
    TooLarge(u64),
}

impl FileError {
    /// The message of an option error, without the option's name: the
    /// `kind` of an I/O error is named, the content never.
    pub fn reason(&self) -> String {
        match self {
            FileError::Io(e) => e.kind().to_string(),
            FileError::NotRegular => "not a regular file".into(),
            FileError::TooLarge(max) => format!("file is too large (more than {max} bytes)"),
        }
    }
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason())
    }
}

/// Open `path` for reading without waiting for a writer, and check the file
/// that was opened: it must be a regular file. Returns it with its metadata,
/// taken from the same open file.
pub fn open_regular(path: &Path) -> Result<(File, Metadata), FileError> {
    // O_NONBLOCK: opening a FIFO for reading must not wait for a writer; it
    // has no effect on a regular file.
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(FileError::Io)?;
    let meta = f.metadata().map_err(FileError::Io)?;
    if !meta.is_file() {
        return Err(FileError::NotRegular);
    }
    Ok((f, meta))
}

/// Read the rest of `file`, at most `max` bytes. At most one byte more is
/// read, so a file much larger than the limit is not read into memory.
pub fn read_limited(file: &mut File, max: u64) -> Result<Vec<u8>, FileError> {
    let mut bytes = Vec::new();
    file.by_ref()
        .take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(FileError::Io)?;
    if bytes.len() as u64 > max {
        return Err(FileError::TooLarge(max));
    }
    Ok(bytes)
}

/// [`open_regular`] and [`read_limited`] in one.
pub fn read_regular(path: &Path, max: u64) -> Result<Vec<u8>, FileError> {
    let (mut f, _) = open_regular(path)?;
    read_limited(&mut f, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::time::Duration;

    fn fifo(dir: &Path) -> std::path::PathBuf {
        let p = dir.join("fifo");
        let c = CString::new(p.as_os_str().as_bytes()).expect("a path has no NUL");
        // SAFETY: `c` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        p
    }

    /// Run `f` on another thread: a read that waits for a writer would hang
    /// the test, so it fails after 5 seconds instead.
    fn within_5s<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the read returned within 5 seconds")
    }

    #[test]
    fn fifo_is_refused_without_waiting() {
        let d = tempfile::tempdir().expect("a temporary directory");
        let p = fifo(d.path());
        let r = within_5s(move || read_regular(&p, 100));
        assert!(matches!(r, Err(FileError::NotRegular)), "{r:?}");
    }

    #[test]
    fn directory_is_refused() {
        let d = tempfile::tempdir().expect("a temporary directory");
        let p = d.path().to_path_buf();
        let r = read_regular(&p, 100);
        assert!(matches!(r, Err(FileError::NotRegular)), "{r:?}");
    }

    #[test]
    fn exactly_the_limit_passes_and_one_more_does_not() {
        let d = tempfile::tempdir().expect("a temporary directory");
        let p = d.path().join("f");
        std::fs::write(&p, [b'x'; 100]).expect("the file can be written");
        assert_eq!(read_regular(&p, 100).expect("100 bytes pass").len(), 100);
        std::fs::write(&p, [b'x'; 101]).expect("the file can be written");
        let r = read_regular(&p, 100);
        assert!(matches!(r, Err(FileError::TooLarge(100))), "{r:?}");
        assert_eq!(
            r.expect_err("too large").reason(),
            "file is too large (more than 100 bytes)"
        );
    }

    #[test]
    fn link_to_a_regular_file_passes() {
        let d = tempfile::tempdir().expect("a temporary directory");
        let target = d.path().join("target");
        let link = d.path().join("link");
        std::fs::write(&target, b"secret").expect("the file can be written");
        std::os::unix::fs::symlink(&target, &link).expect("a symbolic link can be made");
        assert_eq!(read_regular(&link, 100).expect("a link passes"), b"secret");
    }

    #[test]
    fn missing_file_is_an_io_error_not_found() {
        let d = tempfile::tempdir().expect("a temporary directory");
        let r = read_regular(&d.path().join("none"), 100);
        match r {
            Err(FileError::Io(e)) => assert_eq!(e.kind(), io::ErrorKind::NotFound),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn errors_name_no_content() {
        let d = tempfile::tempdir().expect("a temporary directory");
        let p = d.path().join("f");
        std::fs::write(&p, b"hunter2hunter2").expect("the file can be written");
        let e = read_regular(&p, 4).expect_err("too large");
        assert!(!e.reason().contains("hunter2"), "{}", e.reason());
    }
}
