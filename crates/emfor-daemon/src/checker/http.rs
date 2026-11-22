//! `http` checker: one `GET` per check, judged by response status and,
//! with `expect_body`, by a piece of text in the response body.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use emfor_tls::{FileVersion, TlsSettings};
use futures::future::BoxFuture;
use reqwest::redirect::Policy;

use super::tcp::is_local_io_error;
use super::{CheckRequest, CheckResult, Checker, CheckerFactory, OptionError, reject_unknown_keys};

const ALLOWED: &[&str] = &[
    "path",
    "scheme",
    "host_header",
    "expect_status",
    "tls_verify",
    "expect_body",
    "ca_file",
];

/// Longest `expect_body`, as for the tcp checker's `expect`.
const MAX_EXPECT_BODY: usize = 16384;

/// Most body bytes read when looking for `expect_body`.
const MAX_BODY_READ: usize = 65536;

/// Body bytes quoted in the reason when `expect_body` is not found.
const EXCERPT: usize = 64;

const HOST_HEADER_RULE: &str =
    "must be a host with an optional port, such as \"api.internal\" or \"api.internal:8443\"";

const PATH_RULE: &str = "must not contain '#', '\\' or control characters";

pub struct HttpFactory;

/// Inclusive status range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusRange(pub u16, pub u16);

#[derive(Debug, Clone)]
pub struct HttpOptions {
    pub path: String,
    pub https: bool,
    pub host_header: Option<String>,
    pub expect_status: Vec<StatusRange>,
    pub tls_verify: bool,
    /// PEM file of the CA certificates to trust instead of the system's.
    pub ca_file: Option<PathBuf>,
    /// Bytes the body must contain; the body is read only when this is set.
    pub expect_body: Option<Vec<u8>>,
}

impl CheckerFactory for HttpFactory {
    fn kind(&self) -> &str {
        "http"
    }

    fn validate(&self, options: &toml::Table) -> Result<Arc<dyn Checker>, Vec<OptionError>> {
        let opts = parse_options(options)?;
        Ok(Arc::new(HttpChecker {
            opts,
            clients: Mutex::new(HashMap::new()),
        }))
    }
}

fn parse_options(options: &toml::Table) -> Result<HttpOptions, Vec<OptionError>> {
    let mut errors = Vec::new();
    reject_unknown_keys(options, ALLOWED, &mut errors);

    let path = match options.get("path") {
        None => "/".to_string(),
        Some(toml::Value::String(p)) if !p.starts_with('/') => {
            errors.push(OptionError::new("path", "must start with '/'"));
            "/".into()
        }
        Some(toml::Value::String(p)) if valid_path(p) => p.clone(),
        Some(toml::Value::String(_)) => {
            errors.push(OptionError::new("path", PATH_RULE));
            "/".into()
        }
        Some(_) => {
            errors.push(OptionError::new("path", "must be a string"));
            "/".into()
        }
    };

    let https = match options.get("scheme") {
        None => false,
        Some(toml::Value::String(s)) if s == "http" => false,
        Some(toml::Value::String(s)) if s == "https" => true,
        Some(_) => {
            errors.push(OptionError::new("scheme", "must be \"http\" or \"https\""));
            false
        }
    };

    let host_header = match options.get("host_header") {
        None => None,
        Some(toml::Value::String(h)) if valid_host_header(h) => Some(h.clone()),
        Some(toml::Value::String(_)) => {
            errors.push(OptionError::new("host_header", HOST_HEADER_RULE));
            None
        }
        Some(_) => {
            errors.push(OptionError::new("host_header", "must be a string"));
            None
        }
    };

    let expect_status = match options.get("expect_status") {
        None => vec![StatusRange(200, 399)],
        Some(toml::Value::Array(items)) if !items.is_empty() => {
            let mut ranges = Vec::new();
            for (i, item) in items.iter().enumerate() {
                match parse_status_item(item) {
                    Ok(r) => ranges.push(r),
                    Err(msg) => errors.push(OptionError::new(format!("expect_status[{i}]"), msg)),
                }
            }
            ranges
        }
        Some(_) => {
            errors.push(OptionError::new(
                "expect_status",
                "must be a non-empty array",
            ));
            Vec::new()
        }
    };

    let tls_verify = match options.get("tls_verify") {
        None => true,
        Some(toml::Value::Boolean(b)) => *b,
        Some(_) => {
            errors.push(OptionError::new("tls_verify", "must be a boolean"));
            true
        }
    };

    let ca_file = match options.get("ca_file") {
        None => None,
        Some(toml::Value::String(_)) if !https => {
            errors.push(OptionError::new("ca_file", "requires scheme = \"https\""));
            None
        }
        Some(toml::Value::String(file)) => match emfor_tls::check_ca_file(Path::new(file)) {
            Ok(()) => Some(PathBuf::from(file)),
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

    let expect_body = match options.get("expect_body") {
        None => None,
        Some(toml::Value::String(b)) if b.is_empty() => {
            errors.push(OptionError::new("expect_body", "must not be empty"));
            None
        }
        Some(toml::Value::String(b)) if b.len() > MAX_EXPECT_BODY => {
            errors.push(OptionError::new(
                "expect_body",
                format!("must be at most {MAX_EXPECT_BODY} bytes"),
            ));
            None
        }
        Some(toml::Value::String(b)) => Some(b.clone().into_bytes()),
        Some(_) => {
            errors.push(OptionError::new("expect_body", "must be a string"));
            None
        }
    };

    if errors.is_empty() {
        Ok(HttpOptions {
            path,
            https,
            host_header,
            expect_status,
            tls_verify,
            ca_file,
            expect_body,
        })
    } else {
        Err(errors)
    }
}

fn parse_status_item(item: &toml::Value) -> Result<StatusRange, String> {
    let code = |n: i64| -> Result<u16, String> {
        if (100..=599).contains(&n) {
            Ok(n as u16)
        } else {
            Err(format!("status {n} outside 100-599"))
        }
    };
    let parse_num = |s: &str| {
        s.trim()
            .parse::<i64>()
            .map_err(|_| format!("invalid status \"{s}\""))
    };
    match item {
        toml::Value::Integer(n) => {
            let c = code(*n)?;
            Ok(StatusRange(c, c))
        }
        toml::Value::String(s) => match s.split_once('-') {
            Some((lo, hi)) => {
                let (lo, hi) = (code(parse_num(lo)?)?, code(parse_num(hi)?)?);
                if lo > hi {
                    Err(format!("range \"{s}\": lower bound exceeds upper bound"))
                } else {
                    Ok(StatusRange(lo, hi))
                }
            }
            None => {
                let c = code(parse_num(s)?)?;
                Ok(StatusRange(c, c))
            }
        },
        _ => Err("must be a status code or \"lo-hi\" range".into()),
    }
}

/// A client and the version of the `ca_file` it was built from.
type BuiltClient = (Option<FileVersion>, reqwest::Client);

#[derive(Debug)]
pub struct HttpChecker {
    opts: HttpOptions,
    /// With a domain `host_header`, one client per backend (key: its
    /// address), so that the name resolves to that backend's address
    /// (correct SNI and certificate check) while the connection still goes
    /// to the backend. Otherwise one client shared by all backends (key:
    /// `None`), since the URL already holds the address.
    clients: Mutex<HashMap<Option<SocketAddr>, BuiltClient>>,
}

impl HttpChecker {
    /// `host_header` without port, when it is a domain name (not an IP literal).
    fn host_name(&self) -> Option<&str> {
        let h = host_only(self.opts.host_header.as_deref()?);
        h.parse::<IpAddr>().is_err().then_some(h)
    }

    async fn client_for(&self, target: SocketAddr) -> Result<reqwest::Client, String> {
        let name = self.host_name();
        let key = name.map(|_| target);
        // A replaced or rewritten `ca_file` makes the cached client stale:
        // it is built again, so the file takes effect without a reload.
        let version = self
            .opts
            .ca_file
            .as_deref()
            // Not with verification off: the file is not used then.
            .filter(|_| !skip_cert_verification(self.opts.https, self.opts.tls_verify))
            .and_then(emfor_tls::file_version);
        if let Some((built_from, c)) = self
            .clients
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            && *built_from == version
        {
            return Ok(c.clone());
        }
        // A plain `http` request starts no TLS handshake, so it is built
        // without certificate verification: building the verifier reads the
        // system's CA certificates, which a minimal install may not have.
        // That is safe only because redirects are off (`Policy::none()`
        // below): a followed redirect to `https` would accept any certificate.
        //
        // Built without the lock, and off the async workers: building reads
        // and parses the system's CA certificates (rustls-platform-verifier
        // on Linux and FreeBSD), and with a domain `host_header` that
        // happens once per backend.
        let skip = skip_cert_verification(self.opts.https, self.opts.tls_verify);
        let mut b = reqwest::Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .pool_max_idle_per_host(0)
            .tls_danger_accept_invalid_certs(skip);
        if let (false, Some(ca_file)) = (skip, &self.opts.ca_file) {
            // Only the certificates of `ca_file` are trusted, not the
            // system's. Read here, not at validation, so a file that went
            // bad since then is an `error` of this check.
            let tls = TlsSettings {
                verify: true,
                server_name: None,
                ca_file: Some(ca_file.clone()),
            };
            let certs = emfor_tls::trusted_certificates(&tls).map_err(|f| f.reason)?;
            let certs = certs
                .iter()
                .map(|der| reqwest::Certificate::from_der(der))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("ca_file: {e}"))?;
            b = b.tls_certs_only(certs);
        }
        if let Some(name) = name {
            b = b.resolve(name, target);
        }
        let c = match tokio::task::spawn_blocking(move || b.build()).await {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return Err(e.to_string()),
            Err(e) => return Err(e.to_string()),
        };
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = clients.entry(key).or_insert_with(|| (version, c.clone()));
        if entry.0 != version {
            *entry = (version, c.clone());
        }
        Ok(entry.1.clone())
    }

    /// Request URL. The connection always goes to `target`: either the URL
    /// holds `target` itself, or a domain that [`Self::client_for`] resolves
    /// to `target`.
    fn url(&self, target: SocketAddr) -> String {
        let scheme = if self.opts.https { "https" } else { "http" };
        match self.host_name() {
            Some(name) => format!("{scheme}://{name}:{}{}", target.port(), self.opts.path),
            None => format!("{scheme}://{target}{}", self.opts.path),
        }
    }

    fn status_matches(&self, status: u16) -> bool {
        self.opts
            .expect_status
            .iter()
            .any(|r| (r.0..=r.1).contains(&status))
    }
}

/// Whether the client is built without certificate verification: for
/// `tls_verify = false`, and for the plain `http` scheme, which has no
/// certificate to verify (see [`HttpChecker::client_for`]).
fn skip_cert_verification(https: bool, tls_verify: bool) -> bool {
    !tls_verify || !https
}

/// The reason for a client that cannot be built. For `https` with
/// certificate verification on, the usual cause is a host without system CA
/// certificates, which the error text of the client does not say.
fn build_failure(error: &str, verified_https: bool, ca_file: bool) -> String {
    let hint = match (verified_https, ca_file) {
        (true, false) => {
            "; https checks need the system CA certificates, or a ca_file, unless tls_verify = false"
        }
        _ => "",
    };
    format!("cannot build http client: {error}{hint}")
}

/// Whether `h` is a valid `host_header`: an IPv4 address, an IPv6 address
/// in brackets, or a domain name, each with an optional `:port`. A domain
/// must come out of the request URL that [`HttpChecker::url`] builds as the
/// same host, which is also the name [`HttpChecker::client_for`] resolves to
/// the backend; otherwise the check would reach whatever that URL's host
/// resolves to, or fail every time.
fn valid_host_header(h: &str) -> bool {
    if reqwest::header::HeaderValue::from_str(h).is_err() {
        return false;
    }
    let (host, port) = match h.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((ip, after)) if ip.parse::<std::net::Ipv6Addr>().is_ok() => match after {
                "" => return true,
                _ => return after.strip_prefix(':').is_some_and(valid_port),
            },
            _ => return false,
        },
        None => match h.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (h, None),
        },
    };
    if port.is_some_and(|p| !valid_port(p)) {
        return false;
    }
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return true;
    }
    // As a domain: the URL must hold it as its host, unchanged but for case
    // (which both the URL and the resolver override ignore).
    reqwest::Url::parse(&format!("http://{host}:1/")).is_ok_and(|u| {
        u.domain()
            .is_some_and(|d| d.eq_ignore_ascii_case(host) && u.port() == Some(1))
            && u.path() == "/"
            && u.username().is_empty()
    })
}

/// Whether `path` (starting with `/`) reaches the backend as written when
/// [`HttpChecker::url`] puts it in the request URL. The URL parser cuts
/// it at `#` (the rest is a fragment, never sent), drops tabs and line
/// breaks, and reads `\` as `/`; such a path would request another one.
fn valid_path(path: &str) -> bool {
    !path.contains(['#', '\\'])
        && !path.chars().any(char::is_control)
        && reqwest::Url::parse(&format!("http://127.0.0.1:1{path}"))
            .is_ok_and(|u| u.fragment().is_none() && u.host_str() == Some("127.0.0.1"))
}

/// A port: 1 to 5 digits, at most 65535.
fn valid_port(p: &str) -> bool {
    !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok()
}

/// `host_header` without any `:port` suffix or IPv6 brackets.
fn host_only(h: &str) -> &str {
    if let Some(rest) = h.strip_prefix('[') {
        return rest.split_once(']').map(|(a, _)| a).unwrap_or(h);
    }
    h.rsplit_once(':')
        .filter(|(_, p)| p.parse::<u16>().is_ok())
        .map(|(a, _)| a)
        .unwrap_or(h)
}

impl Checker for HttpChecker {
    fn check(&self, req: CheckRequest) -> BoxFuture<'_, CheckResult> {
        let target = req.target;
        Box::pin(async move {
            let client = match self.client_for(target).await {
                Ok(c) => c,
                Err(e) => {
                    let verified_https = self.opts.https && self.opts.tls_verify;
                    return CheckResult::Error(build_failure(
                        &e,
                        verified_https,
                        self.opts.ca_file.is_some(),
                    ));
                }
            };
            let mut req = client.get(self.url(target));
            if let Some(h) = &self.opts.host_header {
                req = req.header(reqwest::header::HOST, h);
            }
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if !self.status_matches(status) {
                        return CheckResult::Down(format!("unexpected status {status}"));
                    }
                    match &self.opts.expect_body {
                        Some(want) => body_contains(resp, want).await,
                        None => CheckResult::Up,
                    }
                }
                Err(e) if has_local_io_error(&e) => CheckResult::Error(client_reason(e)),
                Err(e) => CheckResult::Down(client_reason(e)),
            }
        })
    }

    fn prepare(&self, target: SocketAddr) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            // A build error is reported by the check that follows.
            let _ = self.client_for(target).await;
        })
    }

    fn forget(&self, target: SocketAddr) {
        self.clients
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&Some(target));
    }
}

/// Read the body until it contains `want`, it ends, or [`MAX_BODY_READ`]
/// bytes have been read.
async fn body_contains(mut resp: reqwest::Response, want: &[u8]) -> CheckResult {
    let mut got: Vec<u8> = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let room = MAX_BODY_READ - got.len();
                let before = got.len();
                got.extend_from_slice(&chunk[..chunk.len().min(room)]);
                if found_in_new(&got, before, want) {
                    return CheckResult::Up;
                }
                if got.len() == MAX_BODY_READ {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) if has_local_io_error(&e) => {
                return CheckResult::Error(format!("body: {}", client_reason(e)));
            }
            Err(e) => return CheckResult::Down(format!("body: {}", client_reason(e))),
        }
    }
    let excerpt = &got[..got.len().min(EXCERPT)];
    CheckResult::Down(format!("expect_body: got \"{}\"", excerpt.escape_ascii()))
}

/// Whether `want` is in `got`, given that it was not in `got[..before]`:
/// only the bytes from `before` on and the `want.len() - 1` bytes ahead of
/// them are searched, so a long body read in many chunks is not scanned
/// again and again.
fn found_in_new(got: &[u8], before: usize, want: &[u8]) -> bool {
    let from = before.saturating_sub(want.len().saturating_sub(1));
    got[from..].windows(want.len()).any(|w| w == want)
}

fn has_local_io_error(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && is_local_io_error(io)
        {
            return true;
        }
        cur = err.source();
    }
    false
}

/// The reason for an error of the HTTP client. Its message ends with the
/// request URL, which holds `path` and so may hold a token in the query
/// string; the reason goes to the store and the log, so the URL is left out.
fn client_reason(e: reqwest::Error) -> String {
    error_chain(&e.without_url())
}

fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(err) = cur {
        let s = err.to_string();
        if !parts.last().is_some_and(|p| p.contains(&s)) {
            parts.push(s);
        }
        cur = err.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    // spectra-scenario: health-checking / Plain http without CA certificates
    #[test]
    fn verification_is_skipped_for_plain_http_and_tls_verify_false_only() {
        // (https, tls_verify) -> skipped
        assert!(super::skip_cert_verification(false, true), "http");
        assert!(super::skip_cert_verification(false, false), "http");
        assert!(
            !super::skip_cert_verification(true, true),
            "https, verified"
        );
        assert!(
            super::skip_cert_verification(true, false),
            "https, tls_verify = false"
        );
    }

    // spectra-scenario: health-checking / https without CA certificates
    #[test]
    fn build_failure_names_the_missing_ca_certificates_for_verified_https_only() {
        let hinted = super::build_failure("builder error", true, false);
        assert!(hinted.starts_with("cannot build http client: builder error"));
        assert!(hinted.contains("system CA certificates"), "{hinted}");
        assert!(hinted.contains("tls_verify = false"), "{hinted}");
        // http, or https without verification, have no such cause
        // With a ca_file the system CA certificates are not the cause.
        assert_eq!(
            super::build_failure("builder error", true, true),
            "cannot build http client: builder error"
        );
        assert_eq!(
            super::build_failure("builder error", false, false),
            "cannot build http client: builder error"
        );
    }

    fn ca_options(toml_text: &str) -> Result<HttpOptions, Vec<OptionError>> {
        let table: toml::Table = toml_text.parse().unwrap();
        super::parse_options(&table)
    }

    /// Spec health-checking Example "ca_file options".
    // spectra-scenario: health-checking / ca_file validation
    // spectra-scenario: health-checking / ca_file validation / ca_file options
    #[test]
    fn ca_file_options_are_validated() {
        let dir = tempfile::tempdir().unwrap();
        let pem = dir.path().join("ca.pem");
        std::fs::write(&pem, emfor_tls::testing::pki("ca.test").ca_pem).unwrap();
        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, "not a certificate\n").unwrap();
        let (pem, empty) = (pem.display(), empty.display());

        let ok = ca_options(&format!("scheme = \"https\"\nca_file = \"{pem}\"")).unwrap();
        assert!(ok.tls_verify);
        assert_eq!(
            ok.ca_file.as_deref(),
            Some(std::path::Path::new(&pem.to_string()))
        );
        let off = ca_options(&format!(
            "scheme = \"https\"\nca_file = \"{pem}\"\ntls_verify = false"
        ))
        .unwrap();
        assert!(!off.tls_verify && off.ca_file.is_some());

        let fails = |text: String| -> (String, String) {
            let e = ca_options(&text).unwrap_err();
            assert_eq!(e.len(), 1, "{e:?}");
            (e[0].key.clone(), e[0].message.clone())
        };
        let (k, m) = fails(format!("scheme = \"http\"\nca_file = \"{pem}\""));
        assert_eq!(
            (k.as_str(), m.as_str()),
            ("ca_file", "requires scheme = \"https\"")
        );
        let (k, m) = fails("scheme = \"https\"\nca_file = \"/no/such/file\"".into());
        assert_eq!((k.as_str(), m.as_str()), ("ca_file", "file not found"));
        let (k, m) = fails(format!("scheme = \"https\"\nca_file = \"{empty}\""));
        assert_eq!(
            (k.as_str(), m.as_str()),
            ("ca_file", "no certificate found")
        );
        let (k, m) = fails("scheme = \"https\"\nca_file = 5".into());
        assert_eq!((k.as_str(), m.as_str()), ("ca_file", "must be a string"));
        let (k, m) = fails("scheme = \"https\"\nca_file = \"ca.pem\"".into());
        assert_eq!(
            (k.as_str(), m.as_str()),
            ("ca_file", "not an absolute path")
        );
    }

    fn tls_checker(ca_file: &std::path::Path) -> HttpChecker {
        let options = format!(
            "scheme = \"https\"\nhost_header = \"ca.test\"\nca_file = \"{}\"",
            ca_file.display()
        );
        HttpChecker {
            opts: super::parse_options(&options.parse::<toml::Table>().unwrap()).unwrap(),
            clients: Mutex::new(HashMap::new()),
        }
    }

    /// The system CA certificates cannot be what makes these checks pass or
    /// fail: `ca_file` replaces them.
    // spectra-scenario: health-checking / Private CA through ca_file
    // spectra-scenario: health-checking / ca_file replaces the system CA certificates
    #[tokio::test]
    async fn ca_file_is_the_only_trust_source() {
        let pki = emfor_tls::testing::pki("ca.test");
        let other = emfor_tls::testing::pki("other.test");
        let addr = emfor_tls::testing::https_server(pki.server_config.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let (good, bad) = (dir.path().join("good.pem"), dir.path().join("bad.pem"));
        std::fs::write(&good, &pki.ca_pem).unwrap();
        std::fs::write(&bad, &other.ca_pem).unwrap();

        assert_eq!(tls_checker(&good).check(req(addr)).await, CheckResult::Up);
        let wrong = tls_checker(&bad).check(req(addr)).await;
        assert!(matches!(wrong, CheckResult::Down(_)), "{wrong:?}");

        // The file went bad after validation: an error naming `ca_file`.
        let gone = tls_checker(&good);
        std::fs::remove_file(&good).unwrap();
        match gone.check(req(addr)).await {
            CheckResult::Error(m) => assert!(m.contains("ca_file"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    // spectra-scenario: health-checking / Replaced ca_file takes effect without a reload
    #[tokio::test]
    async fn a_replaced_ca_file_takes_effect_at_the_next_check() {
        let pki = emfor_tls::testing::pki("ca.test");
        let other = emfor_tls::testing::pki("other.test");
        let addr = emfor_tls::testing::https_server(pki.server_config.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ca.pem");
        std::fs::write(&file, &pki.ca_pem).unwrap();
        let checker = tls_checker(&file);
        assert_eq!(checker.check(req(addr)).await, CheckResult::Up);
        assert_eq!(checker.check(req(addr)).await, CheckResult::Up, "reused");

        // Replaced by another file (new inode), as a deploy tool does.
        let next = dir.path().join("next.pem");
        std::fs::write(&next, &other.ca_pem).unwrap();
        std::fs::rename(&next, &file).unwrap();
        let after = checker.check(req(addr)).await;
        assert!(matches!(after, CheckResult::Down(_)), "{after:?}");
        assert_eq!(checker.clients.lock().unwrap().len(), 1, "one client kept");
    }

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest::new("a", addr, std::time::Duration::from_secs(1))
    }

    /// Serve one fixed status per connection; send each raw request to `tx`.
    async fn server(status: u16) -> (SocketAddr, mpsc::UnboundedReceiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
                    let extra = if (300..400).contains(&status) {
                        "Location: /elsewhere\r\n"
                    } else {
                        ""
                    };
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        (addr, rx)
    }

    fn checker(opts: &str) -> Arc<dyn Checker> {
        HttpFactory
            .validate(&opts.parse::<toml::Table>().unwrap())
            .unwrap()
    }

    // spectra-scenario: health-checking / Status inside expected range
    // spectra-scenario: health-checking / Status inside expected range / status matching
    #[tokio::test]
    async fn status_matching_table() {
        // Rows of the "status matching" example in the health-checking spec.
        let cases: &[(&str, u16, Option<&str>)] = &[
            ("", 200, None),
            ("", 302, None),
            ("", 503, Some("503")),
            (r#"expect_status = ["200", "204"]"#, 204, None),
            (r#"expect_status = ["200-299"]"#, 301, Some("301")),
            ("", 204, None),
        ];
        for (opts, status, down_contains) in cases {
            let (addr, _rx) = server(*status).await;
            let r = checker(opts).check(req(addr)).await;
            match down_contains {
                None => assert_eq!(r, CheckResult::Up, "opts {opts:?} status {status}"),
                Some(code) => match r {
                    CheckResult::Down(reason) => assert!(reason.contains(code), "reason {reason}"),
                    other => panic!("opts {opts:?} status {status}: expected down, got {other:?}"),
                },
            }
        }
    }

    // spectra-scenario: health-checking / Host header
    #[tokio::test]
    async fn sends_host_header_and_path() {
        let (addr, mut rx) = server(200).await;
        let c = checker(
            r#"host_header = "api.internal"
path = "/healthz""#,
        );
        assert_eq!(c.check(req(addr)).await, CheckResult::Up);
        let req = rx.recv().await.unwrap().to_lowercase();
        assert!(req.starts_with("get /healthz "), "request: {req}");
        assert!(req.contains("host: api.internal\r\n"), "request: {req}");
    }

    // spectra-scenario: health-checking / Host header
    #[tokio::test]
    async fn ip_literal_host_header_still_connects_to_backend() {
        for host in [
            "10.255.255.1",
            "10.255.255.1:8080",
            "[2001:db8::1]:8080",
            "[2001:db8::1]",
        ] {
            let (addr, mut rx) = server(200).await;
            let c = checker(&format!("host_header = \"{host}\""));
            assert_eq!(
                c.check(req(addr)).await,
                CheckResult::Up,
                "host_header {host}"
            );
            let req = rx.recv().await.unwrap().to_lowercase();
            assert!(req.contains(&format!("host: {host}\r\n")), "request: {req}");
        }
    }

    // spectra-scenario: health-checking / Timeout is a backend failure
    #[tokio::test]
    async fn silent_backend_times_out() {
        // Accepts the connection, never answers.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = l.accept().await {
                held.push(s);
            }
        });
        let r = crate::checker::run_check(
            checker("").as_ref(),
            CheckRequest::new("a", addr, std::time::Duration::from_millis(200)),
        )
        .await;
        assert_eq!(r, CheckResult::Down("timeout".into()));
    }

    fn http_checker(options: &str) -> HttpChecker {
        HttpChecker {
            opts: parse_options(&toml::from_str(options).unwrap()).unwrap(),
            clients: Mutex::new(HashMap::new()),
        }
    }

    #[tokio::test]
    async fn backends_share_one_client_without_a_host_name() {
        let (a, _ra) = server(200).await;
        let (b, _rb) = server(200).await;
        let c = http_checker("host_header = \"10.0.0.1\"");
        c.prepare(a).await;
        c.prepare(b).await;
        assert_eq!(c.clients.lock().unwrap().len(), 1, "one shared client");
        assert_eq!(c.check(req(a)).await, CheckResult::Up);
        assert_eq!(c.check(req(b)).await, CheckResult::Up);
        assert_eq!(c.clients.lock().unwrap().len(), 1, "checks reuse it");
        c.forget(a);
        assert_eq!(c.clients.lock().unwrap().len(), 1, "still used by b");
    }

    #[tokio::test]
    async fn forget_releases_backend_client() {
        let (a, _ra) = server(200).await;
        let (b, _rb) = server(200).await;
        let c = http_checker("host_header = \"api.example\"");
        c.prepare(a).await;
        c.prepare(b).await;
        assert_eq!(c.clients.lock().unwrap().len(), 2, "one per backend");
        assert_eq!(c.check(req(a)).await, CheckResult::Up);
        assert_eq!(c.clients.lock().unwrap().len(), 2, "check reuses it");
        c.forget(a);
        assert_eq!(c.clients.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn refused_is_down() {
        let addr = emfor_fixture::refused_addr();
        assert!(matches!(
            checker("").check(req(addr)).await,
            CheckResult::Down(_)
        ));
    }

    // spectra-scenario: health-checking / Invalid expect_status
    #[test]
    fn validation_errors() {
        let bad = |opts: &str| {
            HttpFactory
                .validate(&opts.parse::<toml::Table>().unwrap())
                .unwrap_err()
        };
        let e = bad(r#"expect_status = ["299-200"]"#);
        assert_eq!(e[0].key, "expect_status[0]");
        assert!(e[0].message.contains("lower bound exceeds upper bound"));
        assert_eq!(bad(r#"scheme = "ftp""#)[0].key, "scheme");
        assert_eq!(bad(r#"path = "healthz""#)[0].key, "path");
        assert_eq!(
            bad(r#"methd = "GET""#)[0],
            OptionError::new("methd", "unknown key")
        );
        assert_eq!(bad(r#"expect_status = [700]"#)[0].key, "expect_status[0]");
        // All problems reported together.
        assert_eq!(
            bad(r#"scheme = "ftp"
tls_verify = "yes""#)
            .len(),
            2
        );
    }

    /// A path the request URL would change: cut at `#`, with a tab or line
    /// break dropped, or with `\` read as `/`. Each was accepted before and
    /// then requested another path.
    #[test]
    fn path_must_reach_the_backend_as_written() {
        for path in [
            "/health#ready",
            "/a\\tb",
            "/he\\nalth",
            "/he\\ralth",
            "/a\\\\..\\\\b",
            "/x\\u0001",
        ] {
            let errs = HttpFactory
                .validate(&format!("path = \"{path}\"").parse::<toml::Table>().unwrap())
                .unwrap_err();
            assert_eq!(errs, vec![OptionError::new("path", PATH_RULE)], "{path}");
        }
        for path in ["/", "/healthz?full=1", "/a b", "/%41/caf\u{e9}"] {
            assert!(valid_path(path), "{path} is sent percent-encoded at worst");
        }
    }

    /// A server answering every request with `status` and a body written in
    /// `parts`, 20 ms apart, ended by closing the connection.
    async fn body_server(status: u16, parts: Vec<Vec<u8>>) -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                let parts = parts.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    let head = format!("HTTP/1.1 {status} X\r\nConnection: close\r\n\r\n");
                    let _ = s.write_all(head.as_bytes()).await;
                    for part in parts {
                        let _ = s.write_all(&part).await;
                        let _ = s.flush().await;
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                });
            }
        });
        addr
    }

    // spectra-scenario: health-checking / Body must contain expect_body
    // spectra-scenario: health-checking / Body must contain expect_body / body matching
    #[tokio::test]
    async fn body_matching_table() {
        let up_json = br#"{"status":"UP"}"#.to_vec();
        let mut long = vec![b'a'; 70000];
        long.extend_from_slice(b"ok");
        let a64 = "a".repeat(64);
        let rows: Vec<(&str, u16, Vec<Vec<u8>>, CheckResult)> = vec![
            (
                "",
                200,
                vec![br#"{"status":"DOWN"}"#.to_vec()],
                CheckResult::Up,
            ),
            (
                r#"expect_body = '"status":"UP"'"#,
                200,
                vec![up_json.clone()],
                CheckResult::Up,
            ),
            (
                r#"expect_body = '"status":"UP"'"#,
                200,
                vec![br#"{"status":"up"}"#.to_vec()],
                CheckResult::Down(r#"expect_body: got "{\"status\":\"up\"}""#.into()),
            ),
            (
                r#"expect_body = '"status":"UP"'"#,
                503,
                vec![up_json],
                CheckResult::Down("unexpected status 503".into()),
            ),
            (
                r#"expect_body = "ok""#,
                200,
                vec![b"o".to_vec(), b"k".to_vec()],
                CheckResult::Up,
            ),
            (
                r#"expect_body = "needle""#,
                200,
                vec![vec![b'a'; 5000], b"xxnee".to_vec(), b"dlexx".to_vec()],
                CheckResult::Up,
            ),
            (
                r#"expect_body = "ok""#,
                200,
                vec![long],
                CheckResult::Down(format!("expect_body: got \"{a64}\"")),
            ),
            (
                r#"expect_body = "ok""#,
                200,
                vec![],
                CheckResult::Down(r#"expect_body: got """#.into()),
            ),
        ];
        for (opts, status, parts, want) in rows {
            let addr = body_server(status, parts).await;
            assert_eq!(checker(opts).check(req(addr)).await, want, "{opts}");
        }
    }

    // spectra-scenario: health-checking / Invalid expect_body
    // spectra-scenario: health-checking / Invalid expect_body / expect_body validation
    #[test]
    fn expect_body_validation() {
        let bad = |opts: &str| {
            HttpFactory
                .validate(&opts.parse::<toml::Table>().unwrap())
                .unwrap_err()
        };
        assert_eq!(
            bad(r#"expect_body = """#),
            vec![OptionError::new("expect_body", "must not be empty")]
        );
        assert_eq!(
            bad(&format!("expect_body = \"{}\"", "x".repeat(16385))),
            vec![OptionError::new(
                "expect_body",
                "must be at most 16384 bytes"
            )]
        );
        assert_eq!(
            bad("expect_body = 200"),
            vec![OptionError::new("expect_body", "must be a string")]
        );
    }

    /// `host_header` is a host with an optional port; anything else would
    /// either fail every check or reach a host other than the backend.
    // spectra-scenario: health-checking / Host header / host_header validation
    #[test]
    fn host_header_must_be_host_and_port() {
        let validate = |h: &str| {
            let mut t = toml::Table::new();
            t.insert("host_header".into(), toml::Value::String(h.into()));
            HttpFactory.validate(&t).map(|_| ())
        };
        for good in [
            "api.internal",
            "API.Internal",
            "api.internal:8443",
            "api.internal.",
            "localhost",
            "10.0.0.1",
            "10.0.0.1:80",
            "[2001:db8::1]",
            "[2001:db8::1]:8080",
        ] {
            assert_eq!(validate(good), Ok(()), "{good}");
        }
        for bad in [
            "",
            "api internal",
            "api.internal/",
            "api.internal/x",
            "http://api.internal",
            "user@api.internal",
            "api.internal:",
            "api.internal:x",
            "api.internal:70000",
            "api.internal:+80",
            "api.internal?x",
            "api.internal#x",
            "2001:db8::1",
            "[2001:db8::1",
            "[2001:db8::1]x",
            "[api.internal]",
            "1.2",
            ":80",
        ] {
            let e = validate(bad).unwrap_err();
            assert_eq!(e.len(), 1, "{bad}: {e:?}");
            assert_eq!(e[0].key, "host_header", "{bad}");
            assert!(
                e[0].message.contains("host with an optional port"),
                "{bad}: {}",
                e[0].message
            );
        }
    }

    /// Only the new bytes and the tail before them are searched.
    #[test]
    fn body_search_covers_a_match_split_across_chunks() {
        let want = b"needle";
        let mut got = b"xxxxxxxxxxnee".to_vec();
        assert!(!found_in_new(&got, 0, want));
        let before = got.len();
        got.extend_from_slice(b"dle");
        assert!(found_in_new(&got, before, want));
        // A match wholly in bytes searched before is not looked for again.
        let got = b"needle....".to_vec();
        assert!(!found_in_new(&got, got.len(), want));
        assert!(found_in_new(&got, 0, want));
    }

    #[test]
    fn host_only_strips_port() {
        assert_eq!(host_only("api.internal"), "api.internal");
        assert_eq!(host_only("api.internal:8443"), "api.internal");
        assert_eq!(host_only("[2001:db8::1]:80"), "2001:db8::1");
        assert_eq!(host_only("[2001:db8::1]"), "2001:db8::1");
    }

    fn reason_of(r: CheckResult) -> String {
        match r {
            CheckResult::Down(s) | CheckResult::Error(s) => s,
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// Spec health-checking "Reason without the request URL": a token in
    /// the query of `path` is not in the reason of a failed check.
    fn assert_no_url(reason: &str) {
        for leaked in ["s3cret", "token=", "/health"] {
            assert!(
                !reason.contains(leaked),
                "{leaked:?} in the reason {reason:?}"
            );
        }
    }

    const TOKEN_PATH: &str = r#"path = "/health?token=s3cret""#;

    // spectra-scenario: health-checking / Reason without the request URL
    #[tokio::test]
    async fn refused_connection_reason_has_no_url() {
        let addr = emfor_fixture::refused_addr();
        let reason = reason_of(checker(TOKEN_PATH).check(req(addr)).await);
        assert_no_url(&reason);
        assert!(reason.to_lowercase().contains("connect"), "{reason}");
    }

    /// Matches the spec scenario for a body failure, but cannot catch a
    /// regression: the client puts no URL in this error, with or without
    /// `client_reason`.
    // spectra-scenario: health-checking / Reason without the request URL
    #[tokio::test]
    async fn body_failure_reason_has_no_url() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    // 100 bytes announced, 3 sent, then the connection ends.
                    let _ = s
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nabc")
                        .await;
                });
            }
        });
        let opts = format!("{TOKEN_PATH}\nexpect_body = \"zzz\"");
        let reason = reason_of(checker(&opts).check(req(addr)).await);
        assert_no_url(&reason);
        assert!(reason.starts_with("body: "), "{reason}");
    }

    // spectra-scenario: health-checking / Reason without the request URL
    #[tokio::test]
    async fn tls_failure_reason_has_no_url() {
        // A server that answers in clear text to a TLS handshake.
        let (addr, _rx) = server(200).await;
        let opts = format!("{TOKEN_PATH}\nscheme = \"https\"");
        let reason = reason_of(checker(&opts).check(req(addr)).await);
        assert_no_url(&reason);
        // "error sending request: client error (Connect): received corrupt
        // message of type InvalidContentType"
        assert!(
            reason.to_lowercase().contains("corrupt message"),
            "{reason}"
        );
    }
}
