//! `tcp` checker: a completed TCP connect is `up`; with `tls = true`, a
//! completed TLS handshake; with `expect`, the backend's response (after
//! the optional `send`) must contain it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use emfor_tls::{TlsFailure, TlsSettings};

use futures::future::BoxFuture;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use super::{CheckRequest, CheckResult, Checker, CheckerFactory, OptionError, reject_unknown_keys};

/// Most bytes kept from the backend while looking for `expect`; also the
/// longest `expect` that can ever match.
pub const MAX_RESPONSE: usize = 16384;

/// Received bytes shown in the reason when `expect` is not found.
const EXCERPT: usize = 64;

pub struct TcpFactory;

impl CheckerFactory for TcpFactory {
    fn kind(&self) -> &str {
        "tcp"
    }

    fn validate(&self, options: &toml::Table) -> Result<Arc<dyn Checker>, Vec<OptionError>> {
        let mut errors = Vec::new();
        reject_unknown_keys(
            options,
            &[
                "send",
                "expect",
                "tls",
                "tls_verify",
                "server_name",
                "ca_file",
            ],
            &mut errors,
        );
        let tls = tls_settings(options, &mut errors);
        let send = string_option(options, "send", &mut errors);
        let expect = string_option(options, "expect", &mut errors);
        let expect = match (&send, expect) {
            (_, Some(e)) if e.is_empty() => {
                errors.push(OptionError::new("expect", "must not be empty"));
                None
            }
            (_, Some(e)) if e.len() > MAX_RESPONSE => {
                errors.push(OptionError::new(
                    "expect",
                    format!("must be at most {MAX_RESPONSE} bytes"),
                ));
                None
            }
            (Some(_), None) if !options.contains_key("expect") => {
                errors.push(OptionError::new("expect", "required when send is set"));
                None
            }
            (_, e) => e,
        };
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(Arc::new(TcpChecker {
            tls,
            expect: expect.map(|expect| Expect {
                send: send.unwrap_or_default().into_bytes(),
                expect: expect.into_bytes(),
            }),
        }))
    }
}

/// The string value of `key`, if set; anything else is an error.
fn string_option(
    options: &toml::Table,
    key: &str,
    errors: &mut Vec<OptionError>,
) -> Option<String> {
    match options.get(key) {
        None => None,
        Some(toml::Value::String(s)) => Some(s.clone()),
        Some(_) => {
            errors.push(OptionError::new(key, "must be a string"));
            None
        }
    }
}

/// The TLS options: `Some` with `tls = true`. `tls_verify`, `server_name`
/// and `ca_file` are checked with the same messages as in emfor-check.
fn tls_settings(options: &toml::Table, errors: &mut Vec<OptionError>) -> Option<TlsSettings> {
    let tls = bool_option(options, "tls", false, errors);
    let verify = bool_option(options, "tls_verify", true, errors);
    let server_name = match options.get("server_name") {
        None => None,
        Some(toml::Value::String(name)) if !name.is_empty() => {
            match emfor_tls::check_server_name(name) {
                Ok(()) => Some(name.clone()),
                Err(message) => {
                    errors.push(OptionError::new("server_name", message));
                    None
                }
            }
        }
        Some(_) => {
            errors.push(OptionError::new(
                "server_name",
                "must be a non-empty string",
            ));
            None
        }
    };
    let ca_file = match options.get("ca_file") {
        None => None,
        Some(toml::Value::String(path)) => match emfor_tls::check_ca_file(Path::new(path)) {
            Ok(()) => Some(PathBuf::from(path)),
            Err(message) => {
                errors.push(OptionError::new("ca_file", message));
                None
            }
        },
        Some(_) => {
            errors.push(OptionError::new("ca_file", "must be a string"));
            None
        }
    };
    if !tls {
        for key in ["tls_verify", "server_name", "ca_file"] {
            if options.contains_key(key) {
                errors.push(OptionError::new(key, "requires tls = true"));
            }
        }
        return None;
    }
    if verify && ca_file.is_none() {
        // Load the system's CA certificates now, while the config is being
        // validated, instead of in the first check.
        emfor_tls::preload_system_roots();
    }
    Some(TlsSettings {
        verify,
        server_name,
        ca_file,
    })
}

fn bool_option(
    options: &toml::Table,
    key: &str,
    default: bool,
    errors: &mut Vec<OptionError>,
) -> bool {
    match options.get(key) {
        None => default,
        Some(toml::Value::Boolean(b)) => *b,
        Some(_) => {
            errors.push(OptionError::new(key, "must be a boolean"));
            default
        }
    }
}

#[derive(Debug)]
pub struct TcpChecker {
    /// `Some` with `tls = true`.
    tls: Option<TlsSettings>,
    expect: Option<Expect>,
}

/// Validated `send` / `expect`.
#[derive(Debug)]
struct Expect {
    /// Written once after connecting; may be empty.
    send: Vec<u8>,
    /// Non-empty, at most [`MAX_RESPONSE`] bytes.
    expect: Vec<u8>,
}

impl Checker for TcpChecker {
    fn check(&self, req: CheckRequest) -> BoxFuture<'_, CheckResult> {
        Box::pin(async move {
            let start = Instant::now();
            let stream = match TcpStream::connect(req.target).await {
                Ok(stream) => stream,
                Err(e) => return io_result("", &e),
            };
            let Some(tls) = &self.tls else {
                return match &self.expect {
                    None => CheckResult::Up,
                    Some(expect) => expect.run(stream).await,
                };
            };
            let remaining = req.timeout.saturating_sub(start.elapsed());
            let mut stream =
                match emfor_tls::handshake(stream, tls, req.target.ip(), remaining).await {
                    Ok(stream) => stream,
                    Err(f) => return tls_result(f),
                };
            match &self.expect {
                None => {
                    // Send close_notify; the result does not depend on it.
                    let _ = stream.shutdown().await;
                    CheckResult::Up
                }
                Some(expect) => expect.run(stream).await,
            }
        })
    }
}

impl Expect {
    /// Write `send`, then read until `expect` shows up, the backend closes
    /// the connection, or [`MAX_RESPONSE`] bytes have arrived.
    async fn run(&self, mut stream: impl AsyncRead + AsyncWrite + Unpin) -> CheckResult {
        if !self.send.is_empty()
            && let Err(e) = stream.write_all(&self.send).await
        {
            return io_result("send: ", &e);
        }
        let mut got = Vec::with_capacity(1024);
        let mut chunk = [0u8; 4096];
        while got.len() < MAX_RESPONSE {
            let want = chunk.len().min(MAX_RESPONSE - got.len());
            let n = match stream.read(&mut chunk[..want]).await {
                Ok(0) => break,
                Ok(n) => n,
                // A TLS backend that closes without close_notify.
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return io_result("expect: ", &e),
            };
            // Only the new bytes and the tail that could start a match.
            let from = got.len().saturating_sub(self.expect.len() - 1);
            got.extend_from_slice(&chunk[..n]);
            if contains(&got[from..], &self.expect) {
                return CheckResult::Up;
            }
        }
        let excerpt = &got[..got.len().min(EXCERPT)];
        CheckResult::Down(format!("expect: got \"{}\"", excerpt.escape_ascii()))
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// `error` for a local TLS failure (such as an unreadable `ca_file`),
/// `down` otherwise.
fn tls_result(f: TlsFailure) -> CheckResult {
    if f.local {
        CheckResult::Error(f.reason)
    } else {
        CheckResult::Down(f.reason)
    }
}

/// `error` for a local failure, `down` otherwise, with `prefix` in front.
fn io_result(prefix: &str, e: &std::io::Error) -> CheckResult {
    if is_local_io_error(e) {
        CheckResult::Error(format!("{prefix}{e}"))
    } else {
        CheckResult::Down(format!("{prefix}{e}"))
    }
}

/// True when an I/O error is caused by the emfor host itself (out of file
/// descriptors, buffers, memory, or local ports) rather than the backend.
/// The same function decides this for TLS handshakes and emfor-check.
pub(crate) use emfor_tls::is_local_io_error;

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::net::TcpListener;

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest::new("a", addr, Duration::from_secs(1))
    }

    fn checker(opts: &str) -> Result<Arc<dyn Checker>, Vec<OptionError>> {
        TcpFactory.validate(&opts.parse::<toml::Table>().unwrap())
    }

    #[tokio::test]
    async fn listening_backend_is_up() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        assert_eq!(checker("").unwrap().check(req(addr)).await, CheckResult::Up);
    }

    #[tokio::test]
    async fn closed_port_is_down_refused() {
        // A test running in parallel may grab the freed port and listen on
        // it, which makes the check Up, or accept and then reset the
        // connection, which makes it Down for a reason other than refused.
        // Try a fresh port on any result that is not a refused connection.
        // (Binding without listening would hold the port, but macOS drops
        // the SYN instead of refusing it.)
        let refused = |r: &CheckResult| match r {
            CheckResult::Down(reason) => reason.to_lowercase().contains("refused"),
            _ => false,
        };
        let mut result = CheckResult::Up;
        for _ in 0..10 {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();
            drop(l);
            result = checker("").unwrap().check(req(addr)).await;
            if refused(&result) {
                break;
            }
        }
        match result {
            CheckResult::Down(r) => assert!(r.to_lowercase().contains("refused"), "reason: {r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[test]
    fn rejects_any_option() {
        let errs = checker(r#"path = "/""#).unwrap_err();
        assert_eq!(errs, vec![OptionError::new("path", "unknown key")]);
    }

    #[test]
    fn send_and_expect_validation() {
        let long = format!("expect = \"{}\"", "a".repeat(MAX_RESPONSE + 1));
        let cases: [(&str, &str, &str); 5] = [
            (
                r#"send = "PING\r\n""#,
                "expect",
                "required when send is set",
            ),
            (r#"expect = """#, "expect", "must not be empty"),
            ("expect = 220", "expect", "must be a string"),
            ("send = [\"a\"]\nexpect = \"b\"", "send", "must be a string"),
            (&long, "expect", "must be at most 16384 bytes"),
        ];
        for (opts, key, message) in cases {
            assert_eq!(
                checker(opts).unwrap_err(),
                vec![OptionError::new(key, message)],
                "{opts}"
            );
        }
        let max = format!("expect = \"{}\"", "a".repeat(MAX_RESPONSE));
        assert!(checker(&max).is_ok());
        assert!(checker("send = \"\"\nexpect = \"x\"").is_ok());
    }

    /// One step of a fake backend.
    enum Step {
        Write(Vec<u8>),
        Sleep(u64),
        /// Read exactly this many bytes into the received bytes.
        Read(usize),
        /// Read until the checker closes the connection.
        ReadToEnd,
        /// Keep the connection open until the checker closes it.
        Hold,
    }
    use Step::*;

    /// A backend that accepts one connection, runs `steps`, then closes;
    /// the handle returns what the checker sent.
    async fn backend(steps: Vec<Step>) -> (SocketAddr, tokio::task::JoinHandle<Vec<u8>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut received = Vec::new();
            for step in steps {
                match step {
                    Write(b) => s.write_all(&b).await.unwrap(),
                    Sleep(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                    Read(n) => {
                        let mut buf = vec![0; n];
                        s.read_exact(&mut buf).await.unwrap();
                        received.extend(buf);
                    }
                    ReadToEnd | Hold => {
                        let _ = s.read_to_end(&mut received).await;
                    }
                }
            }
            received
        });
        (addr, task)
    }

    async fn run(opts: &str, steps: Vec<Step>) -> (CheckResult, Vec<u8>) {
        let (addr, task) = backend(steps).await;
        let result = crate::checker::run_check(&*checker(opts).unwrap(), req(addr)).await;
        (result, task.await.unwrap())
    }

    #[tokio::test]
    async fn banner_matches_without_sending() {
        let (result, received) = run(
            r#"expect = "220""#,
            vec![Write(b"220 mail.example ESMTP\r\n".to_vec()), ReadToEnd],
        )
        .await;
        assert_eq!(result, CheckResult::Up);
        assert_eq!(received, b"");
    }

    #[tokio::test]
    async fn empty_send_is_banner_mode() {
        let (result, received) = run(
            "send = \"\"\nexpect = \"220\"",
            vec![Write(b"220 ready\r\n".to_vec()), ReadToEnd],
        )
        .await;
        assert_eq!(result, CheckResult::Up);
        assert_eq!(received, b"");
    }

    #[tokio::test]
    async fn request_and_response_match() {
        let (result, received) = run(
            r#"send = "stats\r\n"
expect = "STAT pid""#,
            vec![Read(7), Write(b"STAT pid 4242\r\n".to_vec()), ReadToEnd],
        )
        .await;
        assert_eq!(result, CheckResult::Up);
        assert_eq!(received, [0x73, 0x74, 0x61, 0x74, 0x73, 0x0d, 0x0a]);
    }

    #[tokio::test]
    async fn expected_text_split_across_reads() {
        let (result, _) = run(
            r#"expect = "+PONG""#,
            vec![
                Write(b"+PO".to_vec()),
                Sleep(50),
                Write(b"NG\r\n".to_vec()),
                ReadToEnd,
            ],
        )
        .await;
        assert_eq!(result, CheckResult::Up);
    }

    #[tokio::test]
    async fn closed_without_expected_text() {
        let a64 = "a".repeat(64);
        let cases: [(&[u8], String); 4] = [
            (b"ERROR\r\n", r#"expect: got "ERROR\r\n""#.into()),
            (b"", r#"expect: got """#.into()),
            (b"say \"hi\"\x00", r#"expect: got "say \"hi\"\x00""#.into()),
            (&[b'a'; 100], format!("expect: got \"{a64}\"")),
        ];
        for (sent, reason) in cases {
            let (result, _) = run(
                r#"send = "stats\r\n"
expect = "STAT pid""#,
                vec![Read(7), Write(sent.to_vec())],
            )
            .await;
            assert_eq!(result, CheckResult::Down(reason));
        }
    }

    #[tokio::test]
    async fn too_much_data_is_down_before_timeout() {
        let start = std::time::Instant::now();
        let (addr, task) = backend(vec![Write(vec![b'a'; MAX_RESPONSE]), Hold]).await;
        let c = checker(r#"expect = "PONG""#).unwrap();
        let r = CheckRequest::new("a", addr, Duration::from_secs(5));
        match crate::checker::run_check(&*c, r).await {
            CheckResult::Down(reason) => {
                assert!(reason.starts_with("expect: got \"aaaa"), "{reason}")
            }
            other => panic!("expected down, got {other:?}"),
        }
        assert!(start.elapsed() < Duration::from_secs(4));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn silent_backend_is_timeout() {
        let (addr, task) = backend(vec![Hold]).await;
        let c = checker(r#"expect = "220""#).unwrap();
        let r = CheckRequest::new("a", addr, Duration::from_millis(300));
        assert_eq!(
            crate::checker::run_check(&*c, r).await,
            CheckResult::Down("timeout".into())
        );
        task.await.unwrap();
    }

    /// A backend that resets the connection (RST) right after accepting.
    async fn resetting_backend() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let (s, _) = l.accept().await.unwrap();
            s.set_zero_linger().unwrap();
            drop(s);
        });
        addr
    }

    #[tokio::test]
    async fn read_failure_is_expect_reason() {
        let addr = resetting_backend().await;
        let c = checker(r#"expect = "220""#).unwrap();
        match crate::checker::run_check(&*c, req(addr)).await {
            CheckResult::Down(r) => {
                assert!(
                    r.starts_with("expect: ") && !r.starts_with("expect: got"),
                    "{r}"
                )
            }
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_failure_is_send_reason() {
        // 8 MiB cannot fit in the socket buffers, so the write is still
        // going when the reset arrives.
        let addr = resetting_backend().await;
        let mut opts = toml::Table::new();
        opts.insert("send".into(), "x".repeat(8 << 20).into());
        opts.insert("expect".into(), "y".into());
        let c = TcpFactory.validate(&opts).unwrap();
        match crate::checker::run_check(&*c, req(addr)).await {
            CheckResult::Down(r) => assert!(r.starts_with("send: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[test]
    fn tls_option_validation() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.pem");
        let not_pem = dir.path().join("not-pem.txt");
        std::fs::write(&not_pem, "hello").unwrap();
        let (missing, not_pem) = (missing.display(), not_pem.display());
        let cases = [
            (r#"tls = "yes""#.to_string(), "tls", "must be a boolean"),
            (
                r#"server_name = "db.test""#.into(),
                "server_name",
                "requires tls = true",
            ),
            (
                "tls = false\ntls_verify = false".into(),
                "tls_verify",
                "requires tls = true",
            ),
            (
                "tls = true\nserver_name = \"not a name\"".into(),
                "server_name",
                "not a valid host name or IP address",
            ),
            (
                "tls = true\nca_file = \"ca.pem\"".into(),
                "ca_file",
                "not an absolute path",
            ),
            (
                format!("tls = true\nca_file = \"{missing}\""),
                "ca_file",
                "file not found",
            ),
            (
                format!("tls = true\nca_file = \"{not_pem}\""),
                "ca_file",
                "no certificate found",
            ),
        ];
        for (opts, key, message) in cases {
            assert_eq!(
                checker(&opts).unwrap_err(),
                vec![OptionError::new(key, message)],
                "{opts}"
            );
        }
    }

    /// A TLS backend for `db.test` that sends `greeting`, and its CA file.
    async fn tls_backend(greeting: &'static [u8]) -> (SocketAddr, tempfile::TempDir, String) {
        let pki = emfor_tls::testing::pki("db.test");
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, &pki.ca_pem).unwrap();
        let addr = emfor_tls::testing::tls_server(pki.server_config, greeting).await;
        let ca = ca.display().to_string();
        (addr, dir, ca)
    }

    async fn check_with(opts: &str, addr: SocketAddr) -> CheckResult {
        crate::checker::run_check(&*checker(opts).unwrap(), req(addr)).await
    }

    #[tokio::test]
    async fn tls_handshake_succeeds() {
        let (addr, _dir, ca) = tls_backend(b"").await;
        let opts = format!("tls = true\nserver_name = \"db.test\"\nca_file = \"{ca}\"");
        assert_eq!(check_with(&opts, addr).await, CheckResult::Up);
    }

    #[tokio::test]
    async fn tls_with_expected_text() {
        let (addr, _dir, ca) = tls_backend(b"+OK ready\r\n").await;
        let opts =
            format!("tls = true\nserver_name = \"db.test\"\nca_file = \"{ca}\"\nexpect = \"+OK\"");
        assert_eq!(check_with(&opts, addr).await, CheckResult::Up);
    }

    #[tokio::test]
    async fn tls_close_without_close_notify_keeps_excerpt() {
        let pki = emfor_tls::testing::pki("db.test");
        let addr = emfor_tls::testing::tls_server_without_close_notify(
            pki.server_config,
            b"-ERR auth\r\n",
        )
        .await;
        let opts = "tls = true\ntls_verify = false\nexpect = \"+OK\"";
        assert_eq!(
            check_with(opts, addr).await,
            CheckResult::Down(r#"expect: got "-ERR auth\r\n""#.into())
        );
    }

    #[tokio::test]
    async fn tls_name_mismatch_is_down() {
        let (addr, _dir, ca) = tls_backend(b"").await;
        let opts = format!("tls = true\nserver_name = \"other.test\"\nca_file = \"{ca}\"");
        match check_with(&opts, addr).await {
            CheckResult::Down(r) => assert!(r.starts_with("tls: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tls_verification_turned_off() {
        let (addr, _dir, _ca) = tls_backend(b"").await;
        let opts = "tls = true\ntls_verify = false\nserver_name = \"other.test\"";
        assert_eq!(check_with(opts, addr).await, CheckResult::Up);
    }

    #[tokio::test]
    async fn tls_to_a_plaintext_port() {
        let (addr, task) = backend(vec![Hold]).await;
        let c = checker("tls = true\ntls_verify = false").unwrap();
        let start = std::time::Instant::now();
        let r = CheckRequest::new("a", addr, Duration::from_secs(1));
        assert_eq!(
            crate::checker::run_check(&*c, r).await,
            CheckResult::Down(
                "tls: no answer to the TLS handshake (does the port speak TLS?)".into()
            )
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn ca_file_removed_after_validation_is_error() {
        let (addr, dir, ca) = tls_backend(b"").await;
        let c = checker(&format!(
            "tls = true\nserver_name = \"db.test\"\nca_file = \"{ca}\""
        ))
        .unwrap();
        std::fs::remove_file(dir.path().join("ca.pem")).unwrap();
        match crate::checker::run_check(&*c, req(addr)).await {
            CheckResult::Error(r) => assert!(r.starts_with("ca_file: "), "{r}"),
            other => panic!("expected error, got {other:?}"),
        }
    }

    use nix::libc as libc_errno;

    #[test]
    fn local_errno_classification() {
        assert!(is_local_io_error(&std::io::Error::from_raw_os_error(
            libc_errno::EMFILE
        )));
        assert!(!is_local_io_error(&std::io::Error::from_raw_os_error(
            libc_errno::ECONNREFUSED
        )));
    }
}
