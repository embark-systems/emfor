//! The `smtp` plugin: read the greeting, `EHLO`, optionally `STARTTLS`
//! and `EHLO` again over TLS, then `QUIT`; no mail is sent.

use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::net::{self, Stream, io_failure};
use crate::options::{TlsSettings, boolean, string, tls_settings};

const KEYS: &[&str] = &[
    "helo",
    "tls",
    "starttls",
    "tls_verify",
    "server_name",
    "ca_file",
];

/// Longest reply line, CRLF included (RFC 5321 allows 512; servers write
/// longer ones).
const MAX_LINE: usize = 2048;

/// Longest reply, all lines together.
const MAX_REPLY: usize = 65536;

/// Validated `check_options` of the `smtp` plugin.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    /// EHLO argument; `None` means the connection's local address literal.
    pub helo: Option<String>,
    /// Upgrade with STARTTLS; `tls` then holds the TLS settings.
    pub starttls: bool,
    /// TLS settings, with `tls = true` (TLS from the start) or `starttls`.
    pub tls: Option<TlsSettings>,
}

/// Parse and validate the `smtp` plugin's options; `read_files` also
/// checks that `ca_file` is usable.
pub fn parse(options: &Options, read_files: bool) -> Result<Settings, Vec<OptionError>> {
    let mut errors = Vec::new();
    for key in options.keys() {
        if !KEYS.contains(&key.as_str()) {
            errors.push(OptionError::new(key, "unknown key"));
        }
    }
    let helo = match string(options, "helo", &mut errors) {
        Some(h) if is_host_name(h) => Some(h.to_owned()),
        Some(_) => {
            errors.push(OptionError::new("helo", "must be a host name"));
            None
        }
        None => None,
    };
    let tls_on = boolean(options, "tls", false, &mut errors);
    let starttls = boolean(options, "starttls", false, &mut errors);
    if tls_on && starttls {
        errors.push(OptionError::new("starttls", "conflicts with tls"));
    }
    let tls = tls_settings(
        options,
        tls_on || starttls,
        "requires tls = true or starttls = true",
        read_files,
        &mut errors,
    );
    if errors.is_empty() {
        Ok(Settings {
            helo,
            starttls,
            tls,
        })
    } else {
        Err(errors)
    }
}

fn is_host_name(s: &str) -> bool {
    (1..=255).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

/// The EHLO argument for a connection from `local`: an address literal
/// (RFC 5321 section 4.1.3).
fn address_literal(local: SocketAddr) -> String {
    match local.ip() {
        IpAddr::V4(ip) => format!("[{ip}]"),
        IpAddr::V6(ip) => format!("[IPv6:{ip}]"),
    }
}

/// Check one SMTP server with validated settings.
pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let start = Instant::now();
    let tcp = match net::connect(req.ip, req.port).await {
        Ok(s) => s,
        Err(outcome) => return outcome,
    };
    let helo = match (&settings.helo, tcp.local_addr()) {
        (Some(h), _) => h.clone(),
        (None, Ok(local)) => address_literal(local),
        (None, Err(e)) => return io_failure("connect: ", &e),
    };
    let tls = settings.tls.as_ref();

    if let (Some(tls), false) = (tls, settings.starttls) {
        let remaining = req.timeout.saturating_sub(start.elapsed());
        let stream: Box<dyn Stream> = match emfor_tls::handshake(tcp, tls, req.ip, remaining).await
        {
            Ok(s) => Box::new(s),
            Err(f) => return net::tls_outcome(f),
        };
        return converse(stream, &helo).await;
    }

    let mut tcp = tcp;
    let mut replies = Replies::default();
    let ehlo = match greet(&mut tcp, &mut replies, &helo).await {
        Ok(ehlo) => ehlo,
        Err(outcome) => return outcome,
    };
    let Some(tls) = tls.filter(|_| settings.starttls) else {
        return quit(tcp).await;
    };
    if !ehlo.offers("STARTTLS") {
        let _ = quit(tcp).await;
        return CheckOutcome::Down("tls: STARTTLS not offered".into());
    }
    match command(&mut tcp, &mut replies, "STARTTLS").await {
        Ok(r) if r.code == 220 => {}
        Ok(r) => {
            let _ = quit(tcp).await;
            return CheckOutcome::Down(format!("tls: STARTTLS refused: {}", r.summary()));
        }
        Err(outcome) => return outcome,
    }
    // Anything the server sent after its 220 is discarded (RFC 3207
    // section 4.2): it must not be read as a reply over TLS.
    let remaining = req.timeout.saturating_sub(start.elapsed());
    let stream = match emfor_tls::handshake(tcp, tls, req.ip, remaining).await {
        Ok(s) => s,
        Err(f) => return net::tls_outcome(f),
    };
    let mut stream = stream;
    let mut replies = Replies::default();
    match command(&mut stream, &mut replies, &format!("EHLO {helo}")).await {
        Ok(r) if r.code == 250 => quit(stream).await,
        Ok(r) => CheckOutcome::Down(format!("ehlo: {}", r.summary())),
        Err(outcome) => outcome,
    }
}

/// The whole conversation on a connection that needs no upgrade.
async fn converse(mut stream: Box<dyn Stream>, helo: &str) -> CheckOutcome {
    let mut replies = Replies::default();
    match greet(&mut stream, &mut replies, helo).await {
        Ok(_) => quit(stream).await,
        Err(outcome) => outcome,
    }
}

/// Read the greeting and send EHLO; returns the EHLO reply.
async fn greet<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    replies: &mut Replies,
    helo: &str,
) -> Result<Reply, CheckOutcome> {
    let greeting = replies.read(stream).await?;
    if greeting.code != 220 {
        return Err(CheckOutcome::Down(format!(
            "greeting: {}",
            greeting.summary()
        )));
    }
    let ehlo = command(stream, replies, &format!("EHLO {helo}")).await?;
    if ehlo.code != 250 {
        return Err(CheckOutcome::Down(format!("ehlo: {}", ehlo.summary())));
    }
    Ok(ehlo)
}

/// Send `QUIT` without waiting for its answer, close, and report `up`.
async fn quit<S: AsyncWrite + Unpin>(mut stream: S) -> CheckOutcome {
    let _ = stream.write_all(b"QUIT\r\n").await;
    let _ = stream.shutdown().await;
    CheckOutcome::Up
}

/// Send one command line and read its reply.
async fn command<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    replies: &mut Replies,
    line: &str,
) -> Result<Reply, CheckOutcome> {
    let mut bytes = line.as_bytes().to_vec();
    bytes.extend_from_slice(b"\r\n");
    stream
        .write_all(&bytes)
        .await
        .map_err(|e| io_failure("query: ", &e))?;
    stream
        .flush()
        .await
        .map_err(|e| io_failure("query: ", &e))?;
    replies.read(stream).await
}

/// One SMTP reply: its code and the text of each line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reply {
    code: u16,
    lines: Vec<String>,
}

impl Reply {
    /// The code and the first line's text, for a reason.
    fn summary(&self) -> String {
        match self.lines.first().map(|t| t.trim()) {
            Some(text) if !text.is_empty() => format!("{} {text}", self.code),
            _ => self.code.to_string(),
        }
    }

    /// Whether a line of an EHLO reply names `keyword` (case-insensitive),
    /// with or without parameters.
    fn offers(&self, keyword: &str) -> bool {
        self.lines.iter().any(|line| {
            line.split_ascii_whitespace()
                .next()
                .is_some_and(|word| word.eq_ignore_ascii_case(keyword))
        })
    }
}

/// Reads replies from a connection, keeping bytes that arrived early.
#[derive(Default)]
struct Replies {
    buf: Vec<u8>,
}

impl Replies {
    async fn read<S: AsyncRead + Unpin>(&mut self, stream: &mut S) -> Result<Reply, CheckOutcome> {
        let mut code = None;
        let mut lines = Vec::new();
        let mut total = 0;
        loop {
            let line = self.line(stream).await?;
            total += line.len();
            if total > MAX_REPLY {
                return Err(CheckOutcome::Down("query: reply too long".into()));
            }
            let (this_code, last, text) = parse_line(&line)
                .ok_or_else(|| CheckOutcome::Down("query: malformed reply".into()))?;
            if code.is_some_and(|c| c != this_code) {
                return Err(CheckOutcome::Down("query: malformed reply".into()));
            }
            code = Some(this_code);
            lines.push(text);
            if last {
                return Ok(Reply {
                    code: this_code,
                    lines,
                });
            }
        }
    }

    /// One line, without its LF and an optional CR before it.
    async fn line<S: AsyncRead + Unpin>(
        &mut self,
        stream: &mut S,
    ) -> Result<Vec<u8>, CheckOutcome> {
        loop {
            if let Some(end) = self.buf.iter().position(|&b| b == b'\n') {
                if end + 1 > MAX_LINE {
                    return Err(CheckOutcome::Down("query: reply too long".into()));
                }
                let mut line: Vec<u8> = self.buf.drain(..=end).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(line);
            }
            if self.buf.len() >= MAX_LINE {
                return Err(CheckOutcome::Down("query: reply too long".into()));
            }
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk).await {
                Ok(0) => return Err(CheckOutcome::Down("query: connection closed".into())),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(io_failure("query: ", &e)),
            }
        }
    }
}

/// Split a reply line into its code, whether it ends the reply, and its
/// text; `None` when it does not start with three digits followed by `-`,
/// a space or nothing.
fn parse_line(line: &[u8]) -> Option<(u16, bool, String)> {
    let digits = line.get(..3)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let code = digits
        .iter()
        .fold(0u16, |n, d| n * 10 + u16::from(d - b'0'));
    let (last, text) = match line.get(3) {
        None => (true, &[][..]),
        Some(b' ') => (true, &line[4..]),
        Some(b'-') => (false, &line[4..]),
        Some(_) => return None,
    };
    Some((code, last, String::from_utf8_lossy(text).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::net::TcpListener;

    fn options(v: Value) -> Options {
        let Value::Object(map) = v else {
            panic!("options must be an object")
        };
        map
    }

    fn errors(v: Value) -> Vec<(String, String)> {
        match parse(&options(v), true) {
            Ok(_) => Vec::new(),
            Err(errs) => errs.into_iter().map(|e| (e.key, e.message)).collect(),
        }
    }

    fn one(key: &str, message: &str) -> Vec<(String, String)> {
        vec![(key.to_owned(), message.to_owned())]
    }

    #[test]
    fn smtp_option_validation() {
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, include_str!("../tests/data/test-ca.pem")).unwrap();
        let ca = ca.to_str().unwrap();
        let rows = [
            (json!({}), vec![]),
            (json!({"helo": "app1.example.org"}), vec![]),
            (
                json!({"helo": "bad name"}),
                one("helo", "must be a host name"),
            ),
            (json!({"helo": ""}), one("helo", "must be a host name")),
            (json!({"starttls": true, "ca_file": ca}), vec![]),
            (
                json!({"tls": true, "starttls": true}),
                one("starttls", "conflicts with tls"),
            ),
            (
                json!({"server_name": "mx.example.org"}),
                one("server_name", "requires tls = true or starttls = true"),
            ),
            (
                json!({"starttls": "yes"}),
                one("starttls", "must be a boolean"),
            ),
            (json!({"user": "emfor"}), one("user", "unknown key")),
        ];
        for (opts, want) in rows {
            assert_eq!(errors(opts.clone()), want, "{opts}");
        }
    }

    #[test]
    fn helo_defaults_to_an_address_literal() {
        assert_eq!(
            address_literal("10.0.0.5:4321".parse().unwrap()),
            "[10.0.0.5]"
        );
        assert_eq!(
            address_literal("[2001:db8::5]:4321".parse().unwrap()),
            "[IPv6:2001:db8::5]"
        );
    }

    /// How the fake server answers a command line (upper-cased first
    /// word): the reply bytes, or nothing to close the connection.
    type Answer = fn(&str) -> Vec<u8>;

    /// A fake SMTP server: writes `greeting`, then answers each command
    /// with `answer`; after a `220` to STARTTLS it continues over TLS when
    /// `acceptor` is set. Returns the address and the command lines seen.
    async fn fake_smtp(
        greeting: Vec<u8>,
        answer: Answer,
        acceptor: Option<tokio_rustls::TlsAcceptor>,
    ) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let mut stream: Box<dyn Stream> = Box::new(tcp);
            if stream.write_all(&greeting).await.is_err() || greeting.is_empty() {
                return;
            }
            let mut buf = Vec::new();
            loop {
                let mut chunk = [0u8; 1024];
                let n = match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&chunk[..n]);
                while let Some(end) = buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=end).collect();
                    let line = String::from_utf8_lossy(&line).trim_end().to_owned();
                    log.lock().unwrap().push(line.clone());
                    let word = line
                        .split(' ')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_uppercase();
                    let reply = answer(&word);
                    if reply.is_empty() || stream.write_all(&reply).await.is_err() {
                        return;
                    }
                    if word == "STARTTLS" && reply.starts_with(b"220") {
                        let Some(acceptor) = acceptor.clone() else {
                            return;
                        };
                        // `stream` is still the plain TCP stream here.
                        let tcp = stream;
                        let Ok(tls) = acceptor.accept(tcp).await else {
                            return;
                        };
                        stream = Box::new(tls);
                        buf.clear();
                    }
                }
            }
        });
        (addr, seen)
    }

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest {
            service: "mail".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: Default::default(),
            timeout: Duration::from_secs(2),
        }
    }

    fn plain(word: &str) -> Vec<u8> {
        match word {
            "EHLO" => b"250 mx\r\n".to_vec(),
            "QUIT" => b"221 bye\r\n".to_vec(),
            _ => b"500 what\r\n".to_vec(),
        }
    }

    #[tokio::test]
    async fn smtp_replies_without_tls() {
        let down = |r: &str| CheckOutcome::Down(r.into());
        let long = format!("220 {}\r\n", "x".repeat(3000)).into_bytes();
        let rows: Vec<(Vec<u8>, Answer, CheckOutcome)> = vec![
            (b"220 mx ESMTP\r\n".to_vec(), plain, CheckOutcome::Up),
            (
                b"554 5.3.2 no service\r\n".to_vec(),
                plain,
                down("greeting: 554 5.3.2 no service"),
            ),
            (
                b"421 4.3.2 busy\r\n".to_vec(),
                plain,
                down("greeting: 421 4.3.2 busy"),
            ),
            (b"220-mx\r\n220 ESMTP\r\n".to_vec(), plain, CheckOutcome::Up),
            (
                b"220 mx\r\n".to_vec(),
                |w| match w {
                    "EHLO" => b"550 5.7.1 go away\r\n".to_vec(),
                    _ => b"221 bye\r\n".to_vec(),
                },
                down("ehlo: 550 5.7.1 go away"),
            ),
            (
                b"220-mx\r\n250 x\r\n".to_vec(),
                plain,
                down("query: malformed reply"),
            ),
            (b"hello\r\n".to_vec(), plain, down("query: malformed reply")),
            (
                b"220 mx\r\n".to_vec(),
                |_| Vec::new(),
                down("query: connection closed"),
            ),
            (long, plain, down("query: reply too long")),
        ];
        for (greeting, answer, want) in rows {
            let shown = String::from_utf8_lossy(&greeting[..greeting.len().min(40)]).into_owned();
            let (addr, _) = fake_smtp(greeting, answer, None).await;
            assert_eq!(
                check(&req(addr), &Settings::default()).await,
                want,
                "{shown:?}"
            );
        }
    }

    #[tokio::test]
    async fn ehlo_names_the_local_address() {
        let (addr, seen) = fake_smtp(b"220 mx\r\n".to_vec(), plain, None).await;
        assert_eq!(
            check(&req(addr), &Settings::default()).await,
            CheckOutcome::Up
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen, vec!["EHLO [127.0.0.1]", "QUIT"]);
    }

    fn tls_settings_for(ca: &std::path::Path) -> Option<TlsSettings> {
        Some(TlsSettings {
            verify: true,
            server_name: Some("mx.test".into()),
            ca_file: Some(ca.to_path_buf()),
        })
    }

    #[tokio::test]
    async fn smtp_replies_with_starttls() {
        use emfor_tls::testing::pki;
        let pki = pki("mx.test");
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, &pki.ca_pem).unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(pki.server_config);
        let settings = Settings {
            starttls: true,
            tls: tls_settings_for(&ca),
            ..Settings::default()
        };
        let down = |r: &str| CheckOutcome::Down(r.into());
        let rows: Vec<(Answer, CheckOutcome, bool)> = vec![
            (
                |w| match w {
                    "EHLO" => b"250-mx\r\n250 starttls\r\n".to_vec(),
                    "STARTTLS" => b"220 ready\r\n".to_vec(),
                    _ => b"221 bye\r\n".to_vec(),
                },
                CheckOutcome::Up,
                true,
            ),
            (plain, down("tls: STARTTLS not offered"), false),
            (
                |w| match w {
                    "EHLO" => b"250-mx\r\n250 STARTTLS\r\n".to_vec(),
                    "STARTTLS" => b"454 4.7.0 TLS not available\r\n".to_vec(),
                    _ => b"221 bye\r\n".to_vec(),
                },
                down("tls: STARTTLS refused: 454 4.7.0 TLS not available"),
                true,
            ),
        ];
        for (answer, want, sends_starttls) in rows {
            let (addr, seen) =
                fake_smtp(b"220 mx ESMTP\r\n".to_vec(), answer, Some(acceptor.clone())).await;
            assert_eq!(check(&req(addr), &settings).await, want);
            tokio::time::sleep(Duration::from_millis(50)).await;
            let seen = seen.lock().unwrap().clone();
            assert_eq!(
                seen.iter().any(|l| l == "STARTTLS"),
                sends_starttls,
                "{seen:?}"
            );
            if want == CheckOutcome::Up {
                assert_eq!(
                    seen,
                    vec!["EHLO [127.0.0.1]", "STARTTLS", "EHLO [127.0.0.1]", "QUIT"]
                );
            }
        }
    }

    #[tokio::test]
    async fn tls_against_a_plaintext_server_is_tls() {
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, include_str!("../tests/data/test-ca.pem")).unwrap();
        let (addr, _) = fake_smtp(b"220 mx\r\n".to_vec(), plain, None).await;
        let settings = Settings {
            tls: tls_settings_for(&ca),
            ..Settings::default()
        };
        match check(&req(addr), &settings).await {
            CheckOutcome::Down(r) => assert!(r.starts_with("tls: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn implicit_tls_is_up() {
        use emfor_tls::testing::pki;
        let pki = pki("mx.test");
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, &pki.ca_pem).unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(pki.server_config);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut tls = acceptor.accept(tcp).await.unwrap();
            tls.write_all(b"220 mx\r\n").await.unwrap();
            let mut buf = [0u8; 256];
            let _ = tls.read(&mut buf).await;
            tls.write_all(b"250 mx\r\n").await.unwrap();
            let _ = tls.read(&mut buf).await;
        });
        let settings = Settings {
            tls: tls_settings_for(&ca),
            ..Settings::default()
        };
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
    }
}
