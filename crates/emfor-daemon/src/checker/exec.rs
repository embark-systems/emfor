//! `exec` checker: run a command per check; the exit status is the result
//! (Nagios convention).

use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::{CheckRequest, CheckResult, Checker, CheckerFactory, OptionError, reject_unknown_keys};

/// Bytes of stdout kept for the reason.
const STDOUT_LIMIT: usize = 4096;
/// How long to keep reading stdout after the command exited.
const STDOUT_GRACE: Duration = Duration::from_millis(100);
/// Longest reason taken from stdout, in characters.
const REASON_CHARS: usize = 200;

/// Validate a `command` value: a non-empty array of strings whose first
/// element is an absolute path to an existing executable file. Used by
/// `exec` and by `[plugins."<name>"] command`.
pub(crate) fn parse_command(value: Option<&toml::Value>) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Err("missing required key".into());
    };
    let Some(items) = value.as_array() else {
        return Err("must be an array of strings".into());
    };
    if items.is_empty() {
        return Err("must not be empty".into());
    }
    let mut argv = Vec::with_capacity(items.len());
    for item in items {
        match item.as_str() {
            Some(s) => argv.push(s.to_string()),
            None => return Err("must be an array of strings".into()),
        }
    }
    let program = Path::new(&argv[0]);
    if !program.is_absolute() {
        return Err(format!("{:?} is not an absolute path", argv[0]));
    }
    match std::fs::metadata(program) {
        Ok(m) if m.is_file() && m.permissions().mode() & 0o111 != 0 => Ok(argv),
        Ok(_) => Err(format!("{} is not an executable file", argv[0])),
        Err(e) => Err(format!("cannot use {}: {e}", argv[0])),
    }
}

pub struct ExecFactory;

impl CheckerFactory for ExecFactory {
    fn kind(&self) -> &str {
        "exec"
    }

    fn validate(&self, options: &toml::Table) -> Result<Arc<dyn Checker>, Vec<OptionError>> {
        let mut errors = Vec::new();
        reject_unknown_keys(options, &["command"], &mut errors);
        match parse_command(options.get("command")) {
            Ok(argv) if errors.is_empty() => Ok(Arc::new(ExecChecker { argv })),
            Ok(_) => Err(errors),
            Err(m) => {
                errors.push(OptionError::new("command", m));
                Err(errors)
            }
        }
    }
}

#[derive(Debug)]
pub struct ExecChecker {
    argv: Vec<String>,
}

/// Kills the command's whole process group when dropped, so a check that
/// times out (its future is dropped) leaves no processes behind.
struct GroupGuard(Option<Pid>);

impl GroupGuard {
    /// Kill the group now and disarm. Called right after a normal exit to
    /// remove background children; afterwards the group id may be reused by
    /// an unrelated process, so it is never signalled again.
    fn kill_now(&mut self) {
        if let Some(pgid) = self.0.take() {
            let _ = killpg(pgid, Signal::SIGKILL);
        }
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        self.kill_now();
    }
}

impl Checker for ExecChecker {
    fn check(&self, req: CheckRequest) -> BoxFuture<'_, CheckResult> {
        Box::pin(async move {
            let program = &self.argv[0];
            let mut cmd = Command::new(program);
            cmd.args(&self.argv[1..])
                .current_dir("/")
                .env("EMFOR_IP", req.target.ip().to_string())
                .env("EMFOR_PORT", req.target.port().to_string())
                .env("EMFOR_SERVICE", &*req.service)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .process_group(0);
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => return CheckResult::Error(format!("cannot run {program}: {e}")),
            };
            let mut guard = GroupGuard(child.id().map(|id| Pid::from_raw(id as i32)));
            let stdout = child.stdout.take().expect("stdout is piped");
            let mut reader = tokio::spawn(read_limited(stdout));
            let status = match child.wait().await {
                Ok(s) => s,
                Err(e) => return CheckResult::Error(format!("cannot wait for {program}: {e}")),
            };
            guard.kill_now();
            // Background children may keep stdout open; don't wait for them.
            let out = match tokio::time::timeout(STDOUT_GRACE, &mut reader).await {
                Ok(Ok(bytes)) => bytes,
                _ => {
                    reader.abort();
                    Vec::new()
                }
            };
            outcome(status, &first_line(&out))
        })
    }
}

/// Read up to [`STDOUT_LIMIT`] bytes, then drain the rest so the command
/// never blocks on a full pipe.
async fn read_limited(mut r: impl AsyncRead + Unpin) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => return kept,
            Ok(n) => {
                let room = STDOUT_LIMIT.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
}

/// First line of `out`, trimmed and cut to [`REASON_CHARS`] characters.
fn first_line(out: &[u8]) -> String {
    let text = String::from_utf8_lossy(out);
    let line = text.lines().next().unwrap_or("").trim();
    line.chars().take(REASON_CHARS).collect()
}

/// Map an exit status and the first stdout line to a check result.
fn outcome(status: ExitStatus, line: &str) -> CheckResult {
    match status.code() {
        Some(0) => CheckResult::Up,
        Some(code @ (1 | 2)) => CheckResult::Down(if line.is_empty() {
            format!("exit code {code}")
        } else {
            line.to_string()
        }),
        Some(code) if line.is_empty() => CheckResult::Error(format!("exit code {code}")),
        Some(code) => CheckResult::Error(format!("exit code {code}: {line}")),
        None => match status.signal() {
            Some(sig) => CheckResult::Error(format!("killed by signal {sig}")),
            None => CheckResult::Error("exited without a status".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::run_check;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Write an executable `/bin/sh` script into `dir`.
    fn script(dir: &TempDir, name: &str, body: &str) -> PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn options(argv: &[&str]) -> toml::Table {
        let mut t = toml::Table::new();
        t.insert(
            "command".into(),
            toml::Value::Array(argv.iter().map(|s| toml::Value::from(*s)).collect()),
        );
        t
    }

    fn checker_for(path: &Path) -> Arc<dyn Checker> {
        ExecFactory
            .validate(&options(&[path.to_str().unwrap()]))
            .unwrap()
    }

    fn req(timeout: Duration) -> CheckRequest {
        let addr: SocketAddr = "10.0.1.5:5432".parse().unwrap();
        CheckRequest::new("db.svc.emfor", addr, timeout)
    }

    async fn run(body: &str, timeout: Duration) -> CheckResult {
        let dir = TempDir::new().unwrap();
        let p = script(&dir, "check", body);
        run_check(checker_for(&p).as_ref(), req(timeout)).await
    }

    #[tokio::test]
    async fn exit_status_mapping() {
        let t = Duration::from_secs(5);
        let rows: &[(&str, CheckResult)] = &[
            ("exit 0", CheckResult::Up),
            ("echo slow; exit 1", CheckResult::Down("slow".into())),
            ("exit 2", CheckResult::Down("exit code 2".into())),
            (
                "echo 'replication lag 120s'; exit 2",
                CheckResult::Down("replication lag 120s".into()),
            ),
            (
                "echo 'unknown state'; exit 3",
                CheckResult::Error("exit code 3: unknown state".into()),
            ),
            ("exit 7", CheckResult::Error("exit code 7".into())),
            (
                "kill -9 $$",
                CheckResult::Error("killed by signal 9".into()),
            ),
        ];
        for (body, want) in rows {
            assert_eq!(&run(body, t).await, want, "script: {body}");
        }
    }

    /// Last row of the "exit status mapping" example (sleeps longer than
    /// `timeout` → down, `timeout`), kept apart because it needs a short
    /// timeout.
    #[tokio::test]
    async fn sleeping_past_timeout_is_down_timeout() {
        let r = run("sleep 30", Duration::from_millis(300)).await;
        assert_eq!(r, CheckResult::Down("timeout".into()));
    }

    #[tokio::test]
    async fn reason_is_first_line_trimmed_and_cut() {
        let long = "x".repeat(300);
        let r = run(
            &format!("echo '  {long}  '; echo second; exit 1"),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(r, CheckResult::Down("x".repeat(200)));
    }

    #[tokio::test]
    async fn large_output_does_not_block() {
        let r = run(
            "i=0; while [ $i -lt 2000 ]; do echo 'line of output padding padding padding'; i=$((i+1)); done; exit 1",
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            r,
            CheckResult::Down("line of output padding padding padding".into())
        );
    }

    #[tokio::test]
    async fn backend_is_passed_in_environment() {
        let r = run(
            r#"echo "$EMFOR_IP $EMFOR_PORT $EMFOR_SERVICE"; exit 1"#,
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(r, CheckResult::Down("10.0.1.5 5432 db.svc.emfor".into()));
    }

    #[tokio::test]
    async fn arguments_are_passed_without_shell() {
        let dir = TempDir::new().unwrap();
        let p = script(&dir, "check", r#"echo "$1|$2"; exit 1"#);
        let c = ExecFactory
            .validate(&options(&[p.to_str().unwrap(), "a b", "$HOME"]))
            .unwrap();
        let r = run_check(c.as_ref(), req(Duration::from_secs(5))).await;
        assert_eq!(r, CheckResult::Down("a b|$HOME".into()));
    }

    fn alive(pid: i32) -> bool {
        nix::sys::signal::kill(Pid::from_raw(pid), None).is_ok()
    }

    #[tokio::test]
    async fn background_child_is_killed_after_normal_exit() {
        let dir = TempDir::new().unwrap();
        let pidfile = dir.path().join("child.pid");
        let body = format!(
            "sleep 30 > /dev/null &\necho $! > {0}.tmp && mv {0}.tmp {0}\nexit 0",
            pidfile.display()
        );
        let p = script(&dir, "check", &body);
        let r = run_check(checker_for(&p).as_ref(), req(Duration::from_secs(5))).await;
        assert_eq!(r, CheckResult::Up);
        let child: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        for _ in 0..50 {
            if !alive(child) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("background child {child} still running");
    }

    #[tokio::test]
    async fn timeout_kills_child_processes() {
        let dir = TempDir::new().unwrap();
        let pidfile = dir.path().join("child.pid");
        // Write the pid atomically so a slow start never leaves a partial file.
        let body = format!(
            "sleep 30 &\necho $! > {0}.tmp && mv {0}.tmp {0}\nsleep 30",
            pidfile.display()
        );
        let p = script(&dir, "check", &body);
        let r = run_check(checker_for(&p).as_ref(), req(Duration::from_secs(2))).await;
        assert_eq!(r, CheckResult::Down("timeout".into()));
        let child: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // The orphaned child is reaped by init; give it a moment.
        for _ in 0..50 {
            if !alive(child) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("background child {child} still running");
    }

    #[test]
    fn invalid_command_option() {
        let dir = TempDir::new().unwrap();
        let not_exec = dir.path().join("plain");
        std::fs::write(&not_exec, "x").unwrap();
        let cases: Vec<(toml::Table, &str)> = vec![
            (toml::Table::new(), "missing required key"),
            (options(&[]), "must not be empty"),
            (options(&["check_pg"]), "not an absolute path"),
            (
                options(&["/nonexistent/check_pg"]),
                "cannot use /nonexistent/check_pg",
            ),
            (
                options(&[not_exec.to_str().unwrap()]),
                "is not an executable file",
            ),
            (
                "command = \"/bin/sh\"".parse().unwrap(),
                "must be an array of strings",
            ),
        ];
        for (opts, want) in cases {
            let errs = ExecFactory.validate(&opts).unwrap_err();
            assert_eq!(errs.len(), 1, "{opts:?}: {errs:?}");
            assert_eq!(errs[0].key, "command");
            assert!(
                errs[0].message.contains(want),
                "{opts:?}: {}",
                errs[0].message
            );
        }
    }

    #[test]
    fn unknown_key_is_rejected() {
        let mut opts = options(&["/bin/sh"]);
        opts.insert("args".into(), toml::Value::from("x"));
        let errs = ExecFactory.validate(&opts).unwrap_err();
        assert_eq!(errs, vec![OptionError::new("args", "unknown key")]);
    }
}
