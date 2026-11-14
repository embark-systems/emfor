//! Processes the daemon starts (`exec` checks and plugins) get only stdin,
//! stdout and stderr from it, none of its other descriptors.

use tokio::process::Command;

/// Make `cmd` start its process with no descriptor of the daemon but 0, 1
/// and 2.
///
/// macOS has no `SOCK_CLOEXEC`, `pipe2` or `accept4`: a socket or pipe is
/// created first and marked close-on-exec only afterwards, so a process that
/// another thread starts between the two steps inherits it. A plugin runs for
/// as long as the daemon, so a check connection it inherited stays open
/// after the check ended (the backend never sees it closed), and a plugin's
/// pipe it inherited keeps that plugin from seeing end of file. On macOS
/// every descriptor above 2 is therefore marked close-on-exec in the new
/// process, after its stdin, stdout and stderr are set up and before it
/// runs the program. Linux and FreeBSD create every descriptor close-on-exec
/// at once, so elsewhere this does nothing.
pub(crate) fn inherit_stdio_only(cmd: &mut Command) {
    #[cfg(target_os = "macos")]
    macos::install(cmd);
    #[cfg(not(target_os = "macos"))]
    let _ = cmd;
}

#[cfg(target_os = "macos")]
mod macos {
    use std::mem::size_of;

    use libc::{c_int, c_void, proc_fdinfo};
    use tokio::process::Command;

    /// Most entries the descriptor list buffer holds (8 MiB).
    const MAX_ENTRIES: usize = 1 << 20;

    pub(super) fn install(cmd: &mut Command) {
        // Size the buffer here: the new process may not allocate. Descriptors
        // opened before the fork make the list longer than this estimate,
        // hence the room to spare; a list that still does not fit falls back
        // to every possible descriptor number.
        // SAFETY: getpid has no preconditions; proc_pidinfo with a null
        // buffer of size 0 only returns the size the list needs.
        let needed = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                std::ptr::null_mut(),
                0,
            )
        };
        let entries = (usize::try_from(needed).unwrap_or(0) / size_of::<proc_fdinfo>() * 2 + 64)
            .min(MAX_ENTRIES);
        install_with(cmd, entries);
    }

    /// [`install`] with a list buffer of `entries` entries.
    pub(super) fn install_with(cmd: &mut Command, entries: usize) {
        let mut buf = vec![
            proc_fdinfo {
                proc_fd: 0,
                proc_fdtype: 0,
            };
            entries
        ];
        // SAFETY: getdtablesize has no preconditions.
        let table = unsafe { libc::getdtablesize() };
        // SAFETY: the closure runs in the new process between fork and exec,
        // where only async-signal-safe calls are allowed. It calls getpid,
        // proc_pidinfo (a system call writing into the buffer allocated
        // above) and fcntl, and allocates nothing.
        unsafe {
            cmd.pre_exec(move || {
                mark_all(&mut buf, table);
                Ok(())
            });
        }
    }

    /// Mark every open descriptor above 2 close-on-exec: those `proc_pidinfo`
    /// lists, or every number below `table` when the list does not fit `buf`.
    fn mark_all(buf: &mut [proc_fdinfo], table: c_int) {
        let bytes = c_int::try_from(std::mem::size_of_val(buf))
            .expect("the list buffer holds at most MAX_ENTRIES entries of 8 bytes, below 2 GiB");
        // SAFETY: buf is valid for writes of `bytes` bytes.
        let got = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                buf.as_mut_ptr().cast::<c_void>(),
                bytes,
            )
        };
        // A full buffer may have cut the list short.
        if got > 0 && got < bytes {
            let listed = usize::try_from(got).unwrap_or(0) / size_of::<proc_fdinfo>();
            for entry in &buf[..listed] {
                mark(entry.proc_fd);
            }
        } else {
            for fd in 3..table {
                mark(fd);
            }
        }
    }

    fn mark(fd: c_int) {
        if fd <= 2 {
            return;
        }
        // SAFETY: fcntl on a number that is not an open descriptor only
        // fails with EBADF.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
}

// The race only exists on macOS; see `inherit_stdio_only`.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    /// A socket that another thread created but has not yet marked
    /// close-on-exec when a process is started: on macOS the process must not
    /// keep it open.
    #[tokio::test]
    async fn started_process_does_not_inherit_unmarked_descriptors() {
        assert_not_inherited(inherit_stdio_only).await;
    }

    /// The same when the descriptor list does not fit the buffer and every
    /// descriptor number is marked instead.
    #[tokio::test]
    async fn started_process_does_not_inherit_when_the_list_is_cut_short() {
        assert_not_inherited(|cmd| super::macos::install_with(cmd, 1)).await;
    }

    async fn assert_not_inherited(setup: impl FnOnce(&mut Command)) {
        let (ours, theirs) = UnixStream::pair().expect("a socket pair can be created");
        // Set before the other end is closed: macOS refuses socket options
        // on a socket whose peer is gone.
        ours.set_read_timeout(Some(Duration::from_secs(3)))
            .expect("a read timeout can be set");
        let fd = theirs.as_raw_fd();
        // SAFETY: fd is the open descriptor owned by `theirs`.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            assert!(flags >= 0, "fcntl F_GETFD on an open socket succeeds");
            libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
        }
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("10").kill_on_drop(true);
        setup(&mut cmd);
        let child = cmd.spawn().expect("/bin/sleep can be started");
        drop(theirs);

        let read = (&ours).read(&mut [0u8; 1]);
        drop(child);
        assert!(
            matches!(read, Ok(0)),
            "the started process kept the other end open: {read:?}"
        );
    }
}
