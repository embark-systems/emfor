//! Processes the daemon starts (`exec` checks and plugins) get only stdin,
//! stdout and stderr from it, none of its other descriptors.

use std::ffi::{OsStr, OsString};

use tokio::process::Command;

/// Variables of the daemon's environment that every `exec` check and plugin
/// gets, besides the ones its `env_pass` names: these names, and every name
/// that starts with `LC_`.
const PASSED: &[&str] = &["PATH", "LANG", "TZ"];

fn passed_by_default(name: &OsStr) -> bool {
    let name = name.as_encoded_bytes();
    name.starts_with(b"LC_") || PASSED.iter().any(|p| p.as_bytes() == name)
}

/// Give `cmd` the environment of an `exec` check or a plugin: nothing but the
/// variables of the daemon's environment that [`passed_by_default`] allows
/// and the ones named in `pass`.
pub(crate) fn set_environment(cmd: &mut Command, pass: &[String]) {
    set_environment_from(cmd, std::env::vars_os(), pass);
}

/// [`set_environment`] with the daemon's environment given, so that a test
/// does not have to change its own.
pub(crate) fn set_environment_from(
    cmd: &mut Command,
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    pass: &[String],
) {
    cmd.env_clear();
    for (name, value) in vars {
        if passed_by_default(&name) || pass.iter().any(|p| OsStr::new(p) == name) {
            cmd.env(name, value);
        }
    }
}

/// Validate an `env_pass` value: absent is empty; otherwise an array of
/// variable names, `[A-Za-z_][A-Za-z0-9_]*`, none twice.
pub(crate) fn parse_env_pass(value: Option<&toml::Value>) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err("must be an array of strings".into());
    };
    let mut names: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let Some(name) = item.as_str() else {
            return Err("must be an array of strings".into());
        };
        let mut chars = name.chars();
        let valid = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(format!("invalid variable name {name:?}"));
        }
        if names.iter().any(|n| n == name) {
            return Err(format!("duplicate variable name {name:?}"));
        }
        names.push(name.to_string());
    }
    Ok(names)
}

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

#[cfg(test)]
mod env_tests {
    use std::os::unix::ffi::OsStringExt;

    use super::*;

    fn daemon_env() -> Vec<(OsString, OsString)> {
        let mut v: Vec<(OsString, OsString)> = [
            ("SECRET_TOKEN", "abc"),
            ("HOME", "/root"),
            ("PATH", "/usr/bin:/bin"),
            ("LANG", "C.UTF-8"),
            ("LC_ALL", "C"),
            ("TZ", "UTC"),
            ("LCX", "not a locale"),
            ("http_proxy", "http://proxy:3128"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        v.push(("RAW".into(), OsString::from_vec(vec![b'a', 0xff, b'b'])));
        v
    }

    /// The variables a process started with `pass` sees, as `NAME=value`
    /// lines (`env -0`, so a value may hold anything but NUL).
    async fn seen(pass: &[&str]) -> Vec<Vec<u8>> {
        let pass: Vec<String> = pass.iter().map(|s| s.to_string()).collect();
        let mut cmd = Command::new("/usr/bin/env");
        cmd.arg("-0");
        set_environment_from(&mut cmd, daemon_env(), &pass);
        let out = cmd.output().await.expect("env runs");
        assert!(out.status.success());
        let mut lines: Vec<Vec<u8>> = out
            .stdout
            .split(|b| *b == 0)
            .filter(|l| !l.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        lines.sort();
        lines
    }

    fn has(lines: &[Vec<u8>], line: &str) -> bool {
        lines.iter().any(|l| l == line.as_bytes())
    }

    // spectra-scenario: checker-plugins / Variable of the daemon is not passed to a plugin
    // spectra-scenario: checker-plugins / Locale, time zone and PATH are passed to a plugin
    // spectra-scenario: health-checking / Variable of the daemon is not passed
    // spectra-scenario: health-checking / Locale, time zone and PATH are passed
    #[tokio::test]
    async fn only_the_default_variables_are_passed() {
        let lines = seen(&[]).await;
        for want in ["PATH=/usr/bin:/bin", "LANG=C.UTF-8", "LC_ALL=C", "TZ=UTC"] {
            assert!(has(&lines, want), "{want} is missing from {lines:?}");
        }
        assert_eq!(lines.len(), 4, "only the four defaults: {lines:?}");
    }

    // spectra-scenario: checker-plugins / Variable named in env_pass reaches a plugin, also after a restart
    // spectra-scenario: health-checking / Variable named in env_pass is passed
    #[tokio::test]
    async fn named_variables_are_passed_and_missing_ones_are_not() {
        let lines = seen(&["HOME", "http_proxy", "NOT_SET_ANYWHERE"]).await;
        assert!(has(&lines, "HOME=/root"));
        assert!(has(&lines, "http_proxy=http://proxy:3128"));
        assert!(!lines.iter().any(|l| l.starts_with(b"NOT_SET_ANYWHERE")));
        assert!(!lines.iter().any(|l| l.starts_with(b"SECRET_TOKEN")));
        assert_eq!(lines.len(), 6, "four defaults and two named: {lines:?}");
    }

    #[tokio::test]
    async fn a_value_that_is_not_utf8_is_passed_as_it_is() {
        let lines = seen(&["RAW"]).await;
        assert!(
            lines
                .iter()
                .any(|l| l == &[b'R', b'A', b'W', b'=', b'a', 0xff, b'b'])
        );
    }

    fn parse(text: &str) -> Result<Vec<String>, String> {
        let t: toml::Table = format!("env_pass = {text}").parse().unwrap();
        parse_env_pass(t.get("env_pass"))
    }

    // spectra-scenario: checker-plugins / Service uses a plugin / plugin declaration validation
    // spectra-scenario: health-checking / Variable named in env_pass is passed / env_pass validation
    #[test]
    fn env_pass_is_validated() {
        assert_eq!(parse_env_pass(None), Ok(vec![]));
        assert_eq!(parse("[]"), Ok(vec![]));
        assert_eq!(parse(r#"["HOME"]"#), Ok(vec!["HOME".to_string()]));
        assert_eq!(
            parse(r#"["http_proxy", "HTTPS_PROXY", "_x1"]"#),
            Ok(vec![
                "http_proxy".into(),
                "HTTPS_PROXY".into(),
                "_x1".into()
            ])
        );
        assert_eq!(
            parse(r#""HOME""#),
            Err("must be an array of strings".into())
        );
        assert_eq!(parse("[1]"), Err("must be an array of strings".into()));
        for bad in ["HOME=/root", "1ABC", "A B", "", "\u{e9}"] {
            assert_eq!(
                parse(&format!("[{bad:?}]")),
                Err(format!("invalid variable name {bad:?}"))
            );
        }
        assert_eq!(
            parse(r#"["HOME", "HOME"]"#),
            Err(r#"duplicate variable name "HOME""#.into())
        );
    }
}
