//! The `quic` plugin: a QUIC version 1 handshake with the backend, and with
//! `http3 = true` one HTTP/3 `GET` over the new connection.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use bytes::{Buf, Bytes};
use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options};
use emfor_tls::TlsSettings;
use http::Request;
use http::uri::{Authority, PathAndQuery};
use quinn::crypto::rustls::{HandshakeData, QuicClientConfig};
use quinn::{ConnectionError, Endpoint, VarInt};
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::net;
use crate::options::{boolean, string, tls_settings};

const KEYS: &[&str] = &[
    "alpn",
    "tls_verify",
    "server_name",
    "ca_file",
    "http3",
    "path",
    "host_header",
    "expect_status",
    "expect_body",
];

/// The options that only mean something with `http3 = true`.
const HTTP3_KEYS: &[&str] = &["path", "host_header", "expect_status", "expect_body"];

/// The longest `host_header`.
const MAX_HOST_HEADER: usize = 256;
/// The longest `expect_body`.
const MAX_EXPECT_BODY: usize = 16384;
/// The longest ALPN protocol name (RFC 7301).
const MAX_ALPN: usize = 255;

const PATH_ERROR: &str = "must be a URL path without spaces, control characters or #";
const HOST_HEADER_ERROR: &str = "must be a host with an optional port, 1 to 256 bytes";
const STATUS_ERROR: &str = "must be a status code from 100 to 599 or a range lo-hi";

/// The most body bytes read for `expect_body`.
const MAX_BODY_READ: usize = 65536;
/// The largest response header section accepted
/// (SETTINGS_MAX_FIELD_SECTION_SIZE, RFC 9114 section 4.2.2): as much as
/// the body read, and far more than a health endpoint sends. h3's default
/// is unlimited.
const MAX_FIELD_SECTION: u64 = 65536;
/// How much of the body a failed `expect_body` reason shows.
const EXCERPT: usize = 64;
/// The TLS alert `no_application_protocol` (RFC 7301).
const ALERT_NO_APPLICATION_PROTOCOL: u64 = 120;
/// QUIC transport error codes 0x100 to 0x1ff carry a TLS alert.
const CRYPTO_ERRORS: std::ops::Range<u64> = 0x100..0x200;
/// `H3_NO_ERROR` (RFC 9114).
const H3_NO_ERROR: u32 = 0x100;

/// Validated `check_options` of the `quic` plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// ALPN values offered, in order.
    pub alpn: Vec<Vec<u8>>,
    /// Certificate checks; QUIC is always TLS 1.3.
    pub tls: TlsSettings,
    /// The HTTP/3 request, with `http3 = true`.
    pub http3: Option<Http3>,
}

/// What `http3 = true` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Http3 {
    pub path: String,
    pub host_header: Option<String>,
    /// Inclusive status ranges; a single code is a range of one.
    pub expect_status: Vec<(u16, u16)>,
    pub expect_body: Option<String>,
}

impl Default for Http3 {
    fn default() -> Self {
        Http3 {
            path: "/".into(),
            host_header: None,
            expect_status: vec![(200, 399)],
            expect_body: None,
        }
    }
}

/// Parse and validate the `quic` plugin's options; `read_files` also checks
/// that `ca_file` is usable.
pub fn parse(options: &Options, read_files: bool) -> Result<Settings, Vec<OptionError>> {
    let mut errors = Vec::new();
    for key in options.keys() {
        if !KEYS.contains(&key.as_str()) {
            errors.push(OptionError::new(key, "unknown key"));
        }
    }
    let alpn = alpn(options, &mut errors);
    // QUIC is always encrypted, so the TLS options never need `tls = true`.
    let tls = tls_settings(options, true, "", read_files, &mut errors);
    let http3_on = boolean(options, "http3", false, &mut errors);
    if http3_on
        && let Some(alpn) = &alpn
        && !alpn.iter().any(|a| a == b"h3")
    {
        errors.push(OptionError::new("http3", "requires \"h3\" in alpn"));
    }
    let http3 = if http3_on {
        Some(http3(options, &mut errors))
    } else {
        for key in HTTP3_KEYS {
            if options.contains_key(*key) {
                errors.push(OptionError::new(*key, "requires http3 = true"));
            }
        }
        None
    };
    match (alpn, tls) {
        (Some(alpn), Some(tls)) if errors.is_empty() => Ok(Settings { alpn, tls, http3 }),
        _ => Err(errors),
    }
}

fn alpn(options: &Options, errors: &mut Vec<OptionError>) -> Option<Vec<Vec<u8>>> {
    let Some(value) = options.get("alpn") else {
        return Some(vec![b"h3".to_vec()]);
    };
    let Some(items) = value.as_array().filter(|a| !a.is_empty()) else {
        errors.push(OptionError::new("alpn", "must be a non-empty array"));
        return None;
    };
    let alpn: Option<Vec<Vec<u8>>> = items
        .iter()
        .map(|item| match item {
            Value::String(s) if (1..=MAX_ALPN).contains(&s.len()) => Some(s.as_bytes().to_vec()),
            _ => None,
        })
        .collect();
    if alpn.is_none() {
        errors.push(OptionError::new(
            "alpn",
            "must be strings of 1 to 255 bytes",
        ));
    }
    alpn
}

fn http3(options: &Options, errors: &mut Vec<OptionError>) -> Http3 {
    let mut settings = Http3::default();
    // Both become part of the request URI, so they must parse as its parts.
    match string(options, "path", errors) {
        Some(p) if !p.starts_with('/') => {
            errors.push(OptionError::new("path", "must start with /"))
        }
        Some(p) if PathAndQuery::try_from(p).is_ok_and(|q| q.as_str() == p) => {
            settings.path = p.to_owned();
        }
        Some(_) => errors.push(OptionError::new("path", PATH_ERROR)),
        None => {}
    }
    match string(options, "host_header", errors) {
        Some(h) if (1..=MAX_HOST_HEADER).contains(&h.len()) && is_authority(h) => {
            settings.host_header = Some(h.to_owned());
        }
        Some(_) => errors.push(OptionError::new("host_header", HOST_HEADER_ERROR)),
        None => {}
    }
    if let Some(value) = options.get("expect_status") {
        match value.as_array().filter(|a| !a.is_empty()) {
            None => errors.push(OptionError::new(
                "expect_status",
                "must be a non-empty array",
            )),
            Some(items) => match items.iter().map(status_range).collect::<Option<Vec<_>>>() {
                Some(ranges) => settings.expect_status = ranges,
                None => errors.push(OptionError::new("expect_status", STATUS_ERROR)),
            },
        }
    }
    match string(options, "expect_body", errors) {
        Some(b) if (1..=MAX_EXPECT_BODY).contains(&b.len()) => {
            settings.expect_body = Some(b.to_owned());
        }
        Some(_) => errors.push(OptionError::new("expect_body", "must be 1 to 16384 bytes")),
        None => {}
    }
    settings
}

/// A host with an optional port, as `:authority` allows it: no userinfo
/// (RFC 9114 section 4.3.1), no path, no spaces or control characters.
fn is_authority(h: &str) -> bool {
    !h.contains('@') && Authority::try_from(h).is_ok_and(|a| a.as_str() == h)
}

/// One `expect_status` entry: a code (integer or string) or `"lo-hi"`.
fn status_range(item: &Value) -> Option<(u16, u16)> {
    let code = |s: &str| s.parse::<u16>().ok().filter(|c| (100..=599).contains(c));
    let (lo, hi) = match item {
        Value::Number(n) => {
            let c = n.as_u64().and_then(|c| u16::try_from(c).ok())?;
            (c, c)
        }
        Value::String(s) => match s.split_once('-') {
            Some((lo, hi)) => (code(lo)?, code(hi)?),
            None => {
                let c = code(s)?;
                (c, c)
            }
        },
        _ => return None,
    };
    ((100..=599).contains(&lo) && (100..=599).contains(&hi) && lo <= hi).then_some((lo, hi))
}

/// The local UDP endpoints, one per address family, opened by the first
/// check that needs one and shared by all later checks. A failed bind is
/// tried again by the next check.
static ENDPOINT_V4: OnceCell<Endpoint> = OnceCell::const_new();
static ENDPOINT_V6: OnceCell<Endpoint> = OnceCell::const_new();

async fn endpoint(ip: IpAddr) -> Result<&'static Endpoint, CheckOutcome> {
    let (cell, local) = match ip {
        IpAddr::V4(_) => (&ENDPOINT_V4, SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))),
        IpAddr::V6(_) => (&ENDPOINT_V6, SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))),
    };
    cell.get_or_try_init(|| async { Endpoint::client(local) })
        .await
        .map_err(|e| CheckOutcome::Error(format!("quic: cannot open a UDP socket: {e}")))
}

/// Run one check; the plugin's own timeout applies as well, this one makes
/// the result the same when the check is called directly.
pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    match tokio::time::timeout(req.timeout, run(req, settings)).await {
        Ok(outcome) => outcome,
        Err(_) => CheckOutcome::Down("timeout".into()),
    }
}

async fn run(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let config = match client_config(settings) {
        Ok(c) => c,
        Err(outcome) => return outcome,
    };
    let endpoint = match endpoint(req.ip).await {
        Ok(e) => e,
        Err(outcome) => return outcome,
    };
    let server_name = settings
        .tls
        .server_name
        .clone()
        .unwrap_or_else(|| req.ip.to_string());
    let connecting =
        match endpoint.connect_with(config, SocketAddr::new(req.ip, req.port), &server_name) {
            Ok(c) => c,
            Err(e) => return CheckOutcome::Error(format!("quic: {e}")),
        };
    let connection = match connecting.await {
        Ok(c) => c,
        Err(e) => return connection_error(&e),
    };
    let protocol = connection
        .handshake_data()
        .and_then(|d| d.downcast::<HandshakeData>().ok())
        .and_then(|d| d.protocol);
    if !protocol.as_ref().is_some_and(|p| settings.alpn.contains(p)) {
        connection.close(VarInt::from_u32(0), b"");
        let value = protocol.map_or_else(|| "none".to_owned(), |p| p.escape_ascii().to_string());
        return CheckOutcome::Down(format!("tls: server selected ALPN {value} not in alpn"));
    }
    let Some(http3) = &settings.http3 else {
        connection.close(VarInt::from_u32(0), b"");
        return CheckOutcome::Up;
    };
    let authority = authority(http3, settings, req.ip, req.port);
    request(connection, http3, &authority).await
}

/// The QUIC client configuration: `emfor-tls` trust and verification,
/// TLS 1.3 only, the `alpn` values, no 0-RTT.
fn client_config(settings: &Settings) -> Result<quinn::ClientConfig, CheckOutcome> {
    let tls = net::client_config(&settings.tls)?;
    let mut tls = (*tls).clone();
    tls.alpn_protocols.clone_from(&settings.alpn);
    tls.enable_early_data = false;
    let quic =
        QuicClientConfig::try_from(tls).map_err(|e| CheckOutcome::Error(format!("tls: {e}")))?;
    Ok(quinn::ClientConfig::new(Arc::new(quic)))
}

/// The outcome for a connection that failed or was closed.
fn connection_error(e: &ConnectionError) -> CheckOutcome {
    let down = |r: String| CheckOutcome::Down(r);
    match e {
        ConnectionError::VersionMismatch => down("quic: version negotiation failed".into()),
        ConnectionError::TransportError(t) if CRYPTO_ERRORS.contains(&u64::from(t.code)) => {
            down(tls_reason(u64::from(t.code), &t.reason))
        }
        ConnectionError::ConnectionClosed(c)
            if CRYPTO_ERRORS.contains(&u64::from(c.error_code)) =>
        {
            down(tls_reason(
                u64::from(c.error_code),
                &String::from_utf8_lossy(&c.reason),
            ))
        }
        ConnectionError::ConnectionClosed(c) => {
            down(peer_closed(u64::from(c.error_code), &c.reason))
        }
        ConnectionError::ApplicationClosed(c) => {
            down(peer_closed(u64::from(c.error_code), &c.reason))
        }
        ConnectionError::Reset => down("quic: stateless reset".into()),
        other => down(format!("quic: {other}")),
    }
}

/// `tls: …` for a QUIC error code that carries a TLS alert.
fn tls_reason(code: u64, reason: &str) -> String {
    let alert = code - CRYPTO_ERRORS.start;
    if alert == ALERT_NO_APPLICATION_PROTOCOL {
        "tls: no application protocol (ALPN)".into()
    } else if reason.is_empty() {
        format!("tls: alert {alert}")
    } else {
        format!("tls: {reason}")
    }
}

fn peer_closed(code: u64, reason: &[u8]) -> String {
    if reason.is_empty() {
        format!("quic: closed by peer: {code}")
    } else {
        format!(
            "quic: closed by peer: {code}: {}",
            String::from_utf8_lossy(reason)
        )
    }
}

/// `:authority`: `host_header`, else `server_name` (an IPv6 address in
/// brackets), else the backend address.
fn authority(http3: &Http3, settings: &Settings, ip: IpAddr, port: u16) -> String {
    if let Some(h) = &http3.host_header {
        return h.clone();
    }
    if let Some(name) = &settings.tls.server_name {
        return net::authority_host(name);
    }
    match ip {
        IpAddr::V4(v4) => format!("{v4}:{port}"),
        IpAddr::V6(v6) => format!("[{v6}]:{port}"),
    }
}

fn http3_error(e: impl std::fmt::Display) -> CheckOutcome {
    CheckOutcome::Down(format!("query: HTTP/3: {e}"))
}

/// The HTTP/3 `GET` on `connection`, judged by `expect_status` and
/// `expect_body`; then the connection is closed with `H3_NO_ERROR`.
async fn request(connection: quinn::Connection, http3: &Http3, authority: &str) -> CheckOutcome {
    let (outcome, control, _send) = match h3::client::builder()
        .max_field_section_size(MAX_FIELD_SECTION)
        .build(h3_quinn::Connection::new(connection.clone()))
        .await
    {
        Ok((mut driver, mut send)) => {
            // The connection's control streams run beside the request.
            let control = tokio::spawn(async move { driver.wait_idle().await });
            let outcome = exchange(&mut send, http3, authority).await;
            (outcome, Some(control), Some(send))
        }
        Err(e) => (http3_error(e), None, None),
    };
    // An HTTP/3 or body failure caused by the connection ending (the peer
    // closed it, a reset, an idle timeout) reads as that connection error,
    // which comes first in the order of reasons. A close by h3 itself is
    // local and keeps the HTTP/3 error. This comes before our own close,
    // which replaces the close reason.
    let outcome = match (&outcome, connection.close_reason()) {
        (CheckOutcome::Down(r), Some(e))
            if (r.starts_with("query: ") || r.starts_with("body: "))
                && !matches!(e, ConnectionError::LocallyClosed) =>
        {
            connection_error(&e)
        }
        _ => outcome,
    };
    // Close before stopping the control task: a server that sees the
    // control stream end first closes with H3_CLOSED_CRITICAL_STREAM
    // instead of receiving H3_NO_ERROR.
    connection.close(VarInt::from_u32(H3_NO_ERROR), b"");
    if let Some(control) = control {
        control.abort();
    }
    outcome
}

async fn exchange<T>(
    send: &mut h3::client::SendRequest<T, Bytes>,
    http3: &Http3,
    authority: &str,
) -> CheckOutcome
where
    T: h3::quic::OpenStreams<Bytes>,
{
    let request = match Request::get(format!("https://{authority}{}", http3.path))
        .header(
            "user-agent",
            concat!("emfor-check/", env!("CARGO_PKG_VERSION")),
        )
        .body(())
    {
        Ok(r) => r,
        Err(e) => return CheckOutcome::Error(format!("internal: HTTP/3 request: {e}")),
    };
    let mut stream = match send.send_request(request).await {
        Ok(s) => s,
        Err(e) => return http3_error(e),
    };
    if let Err(e) = stream.finish().await {
        return http3_error(e);
    }
    let response = match stream.recv_response().await {
        Ok(r) => r,
        Err(e) => return http3_error(e),
    };
    let status = response.status().as_u16();
    if !http3
        .expect_status
        .iter()
        .any(|(lo, hi)| (*lo..=*hi).contains(&status))
    {
        return CheckOutcome::Down(format!("unexpected status {status}"));
    }
    let Some(want) = &http3.expect_body else {
        return CheckOutcome::Up;
    };
    let want = want.as_bytes();
    let mut got: Vec<u8> = Vec::new();
    loop {
        match stream.recv_data().await {
            Ok(Some(mut chunk)) => {
                while chunk.has_remaining() && got.len() < MAX_BODY_READ {
                    let part = chunk.chunk();
                    let n = part.len().min(MAX_BODY_READ - got.len());
                    got.extend_from_slice(&part[..n]);
                    chunk.advance(n);
                }
                if contains(&got, want) {
                    return CheckOutcome::Up;
                }
                if got.len() == MAX_BODY_READ {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => return CheckOutcome::Down(format!("body: HTTP/3: {e}")),
        }
    }
    let excerpt = &got[..got.len().min(EXCERPT)];
    CheckOutcome::Down(format!("expect_body: got \"{}\"", excerpt.escape_ascii()))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn options(v: Value) -> Options {
        let Value::Object(map) = v else {
            panic!("options must be an object")
        };
        map.into_iter().collect()
    }

    fn errors(v: Value) -> Vec<(String, String)> {
        match parse(&options(v), false) {
            Ok(_) => vec![],
            Err(e) => e.into_iter().map(|e| (e.key, e.message)).collect(),
        }
    }

    fn one(key: &str, message: &str) -> Vec<(String, String)> {
        vec![(key.to_owned(), message.to_owned())]
    }

    /// Spec network-checks Example "QUIC option validation".
    #[test]
    fn quic_option_validation() {
        let rows = [
            (json!({}), vec![]),
            (json!({"alpn": ["h3", "hq-interop"]}), vec![]),
            (
                json!({"alpn": []}),
                one("alpn", "must be a non-empty array"),
            ),
            (
                json!({"alpn": [""]}),
                one("alpn", "must be strings of 1 to 255 bytes"),
            ),
            (
                json!({"alpn": "h3"}),
                one("alpn", "must be a non-empty array"),
            ),
            (
                json!({"http3": true, "path": "/healthz", "expect_status": [200, "204-299"], "expect_body": "ok"}),
                vec![],
            ),
            (
                json!({"http3": true, "alpn": ["doq"]}),
                one("http3", "requires \"h3\" in alpn"),
            ),
            (
                json!({"http3": true, "path": "healthz"}),
                one("path", "must start with /"),
            ),
            (
                json!({"http3": true, "expect_status": ["299-200"]}),
                one("expect_status", STATUS_ERROR),
            ),
            (
                json!({"http3": true, "expect_status": []}),
                one("expect_status", "must be a non-empty array"),
            ),
            (
                json!({"http3": true, "host_header": "a\nb"}),
                one("host_header", HOST_HEADER_ERROR),
            ),
            (
                json!({"expect_body": "ok"}),
                one("expect_body", "requires http3 = true"),
            ),
            (
                json!({"server_name": "web.internal", "tls_verify": false}),
                vec![],
            ),
            (json!({"tls": true}), one("tls", "unknown key")),
            (json!({"http3": "yes"}), one("http3", "must be a boolean")),
            // Beyond the spec's table.
            (
                json!({"path": "/healthz"}),
                one("path", "requires http3 = true"),
            ),
            (
                json!({"alpn": ["x".repeat(256)]}),
                one("alpn", "must be strings of 1 to 255 bytes"),
            ),
            (
                json!({"http3": true, "expect_status": [99]}),
                one("expect_status", STATUS_ERROR),
            ),
            (
                json!({"http3": true, "expect_status": ["600"]}),
                one("expect_status", STATUS_ERROR),
            ),
            (
                json!({"http3": true, "expect_status": [true]}),
                one("expect_status", STATUS_ERROR),
            ),
            (
                json!({"http3": true, "host_header": "h".repeat(257)}),
                one("host_header", HOST_HEADER_ERROR),
            ),
            (
                json!({"http3": true, "expect_body": ""}),
                one("expect_body", "must be 1 to 16384 bytes"),
            ),
            (
                json!({"http3": true, "path": 1}),
                one("path", "must be a string"),
            ),
            (
                json!({"http3": true, "host_header": "api.internal/x"}),
                one("host_header", HOST_HEADER_ERROR),
            ),
            (
                json!({"http3": true, "host_header": "a b"}),
                one("host_header", HOST_HEADER_ERROR),
            ),
            (
                json!({"http3": true, "host_header": "user@api.internal"}),
                one("host_header", HOST_HEADER_ERROR),
            ),
            (
                json!({"http3": true, "host_header": "api.internal:8443"}),
                vec![],
            ),
            (
                json!({"http3": true, "host_header": "[2001:db8::1]:443"}),
                vec![],
            ),
            (
                json!({"http3": true, "path": "/a b"}),
                one("path", PATH_ERROR),
            ),
            (
                json!({"http3": true, "path": "/a#b"}),
                one("path", PATH_ERROR),
            ),
            (json!({"http3": true, "path": "/healthz?full=1"}), vec![]),
        ];
        for (input, want) in rows {
            assert_eq!(errors(input.clone()), want, "{input}");
        }
    }

    #[test]
    fn defaults() {
        let s = parse(&options(json!({})), false).unwrap();
        assert_eq!(s.alpn, vec![b"h3".to_vec()]);
        assert!(s.tls.verify);
        assert_eq!(s.http3, None);
        let s = parse(&options(json!({"http3": true})), false).unwrap();
        assert_eq!(s.http3, Some(Http3::default()));
        let s = parse(
            &options(json!({"http3": true, "expect_status": [204, "200", "300-302"]})),
            false,
        )
        .unwrap();
        assert_eq!(
            s.http3.map(|h| h.expect_status),
            Some(vec![(204, 204), (200, 200), (300, 302)])
        );
    }

    use std::sync::LazyLock;
    use std::time::{Duration, Instant};

    use emfor_tls::testing::pki;
    use quinn::crypto::rustls::QuicServerConfig;
    use tokio::sync::mpsc;

    /// One runtime for every QUIC test: the shared client endpoints belong
    /// to the runtime that opened them.
    static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a test runtime can be built")
    });

    const NAME: &str = "web.internal";

    /// What the test server does with a connection.
    #[derive(Clone)]
    enum Serve {
        /// Complete the handshake and report how the client closed.
        Handshake,
        /// Refuse the connection before the handshake.
        Refuse,
        /// After the handshake, close with application code 0x10, `busy`.
        CloseBusy,
        /// Answer HTTP/3 requests with this status, extra header and body.
        Http(u16, Option<(&'static str, &'static str)>, &'static str),
        /// Reset every HTTP/3 request stream.
        Reset,
        /// Answer 200 and part of a body, then close the connection with
        /// application code 0x10, `busy`.
        CloseInBody,
        /// Answer 200, then reset the stream instead of sending the body.
        ResetInBody,
    }

    /// What the server saw: the client's close, or a request's
    /// `:authority` and `user-agent`.
    #[derive(Debug, PartialEq)]
    enum Seen {
        Closed(ConnectionError),
        Request(String, String),
    }

    struct Server {
        addr: SocketAddr,
        ca: tempfile::NamedTempFile,
        seen: mpsc::UnboundedReceiver<Seen>,
        _endpoint: Endpoint,
    }

    fn server(serve: Serve) -> Server {
        let pki = pki(NAME);
        let mut tls = (*pki.server_config).clone();
        tls.alpn_protocols = vec![b"h3".to_vec()];
        let quic = QuicServerConfig::try_from(tls).expect("TLS 1.3 server config");
        let config = quinn::ServerConfig::with_crypto(Arc::new(quic));
        let endpoint = RUNTIME
            .block_on(async { Endpoint::server(config, "127.0.0.1:0".parse().unwrap()) })
            .expect("the test server can bind");
        let addr = endpoint.local_addr().unwrap();
        let ca = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(ca.path(), &pki.ca_pem).unwrap();
        let (tx, seen) = mpsc::unbounded_channel();
        let accepting = endpoint.clone();
        RUNTIME.spawn(async move {
            while let Some(incoming) = accepting.accept().await {
                let (serve, tx) = (serve.clone(), tx.clone());
                tokio::spawn(async move {
                    if let Serve::Refuse = serve {
                        incoming.refuse();
                        return;
                    }
                    let Ok(conn) = incoming.await else { return };
                    match serve {
                        Serve::Handshake => {
                            let _ = tx.send(Seen::Closed(conn.closed().await));
                        }
                        Serve::CloseBusy => conn.close(VarInt::from_u32(0x10), b"busy"),
                        Serve::Http(..)
                        | Serve::Reset
                        | Serve::CloseInBody
                        | Serve::ResetInBody => http(conn, serve, tx).await,
                        Serve::Refuse => {}
                    }
                });
            }
        });
        Server {
            addr,
            ca,
            seen,
            _endpoint: endpoint,
        }
    }

    async fn http(conn: quinn::Connection, serve: Serve, tx: mpsc::UnboundedSender<Seen>) {
        let watch = conn.clone();
        // Kept until the client's close is read: dropping the h3
        // connection closes it with H3_NO_ERROR, which replaces the
        // client's close reason.
        let h3 = h3::server::Connection::<_, Bytes>::new(h3_quinn::Connection::new(conn)).await;
        if let Ok(mut h3) = h3 {
            serve_requests(&mut h3, &watch, &serve, &tx).await;
            let _ = tx.send(Seen::Closed(watch.closed().await));
        } else {
            let _ = tx.send(Seen::Closed(watch.closed().await));
        }
    }

    async fn serve_requests(
        h3: &mut h3::server::Connection<h3_quinn::Connection, Bytes>,
        conn: &quinn::Connection,
        serve: &Serve,
        tx: &mpsc::UnboundedSender<Seen>,
    ) {
        while let Ok(Some(resolver)) = h3.accept().await {
            let Ok((request, mut stream)) = resolver.resolve_request().await else {
                return;
            };
            let authority = request.uri().authority().map(|a| a.to_string());
            let agent = request
                .headers()
                .get("user-agent")
                .map(|v| v.to_str().unwrap().to_owned());
            let _ = tx.send(Seen::Request(
                authority.unwrap_or_default(),
                agent.unwrap_or_default(),
            ));
            // Read the request to its end first, as a real server does: a
            // request stream dropped before the client's FIN arrives is
            // stopped with code 0, which the client sees as a reset.
            while let Ok(Some(_)) = stream.recv_data().await {}
            match serve {
                Serve::Http(status, header, body) => {
                    let mut response = http::Response::builder().status(*status);
                    if let Some((k, v)) = header {
                        response = response.header(*k, *v);
                    }
                    let _ = stream.send_response(response.body(()).unwrap()).await;
                    if !body.is_empty() {
                        let _ = stream.send_data(Bytes::from_static(body.as_bytes())).await;
                    }
                    let _ = stream.finish().await;
                }
                Serve::CloseInBody | Serve::ResetInBody => {
                    let ok = http::Response::builder().status(200).body(()).unwrap();
                    let _ = stream.send_response(ok).await;
                    if let Serve::CloseInBody = serve {
                        let _ = stream.send_data(Bytes::from_static(b"partial")).await;
                    }
                    // Let the client get past the headers first.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if let Serve::CloseInBody = serve {
                        conn.close(VarInt::from_u32(0x10), b"busy");
                    } else {
                        stream.stop_stream(h3::error::Code::H3_INTERNAL_ERROR);
                    }
                }
                _ => stream.stop_stream(h3::error::Code::H3_REQUEST_REJECTED),
            }
        }
    }

    impl Server {
        /// A request to this server with `options` plus `server_name` and
        /// `ca_file` for its certificate, unless `options` set them.
        fn req(&self, extra: Value) -> CheckRequest {
            let mut o = json!({"server_name": NAME, "ca_file": self.ca.path()});
            for (k, v) in extra.as_object().unwrap() {
                o[k] = v.clone();
            }
            o.as_object_mut().unwrap().retain(|_, v| !v.is_null());
            CheckRequest {
                service: "web".into(),
                ip: self.addr.ip(),
                port: self.addr.port(),
                options: options(o),
                timeout: Duration::from_secs(5),
            }
        }

        fn check(&self, extra: Value) -> CheckOutcome {
            let req = self.req(extra);
            let settings = parse(&req.options, true).expect("valid options");
            RUNTIME.block_on(check(&req, &settings))
        }

        /// The next request the server saw, skipping closes.
        fn next_request(&mut self) -> Seen {
            loop {
                let seen = self.next_seen();
                if let Seen::Request(..) = seen {
                    return seen;
                }
            }
        }

        fn next_seen(&mut self) -> Seen {
            RUNTIME.block_on(async {
                tokio::time::timeout(Duration::from_secs(5), self.seen.recv())
                    .await
                    .expect("the server reports within 5 s")
                    .expect("the server is running")
            })
        }
    }

    fn down(reason: &str) -> CheckOutcome {
        CheckOutcome::Down(reason.into())
    }

    fn down_starting(outcome: &CheckOutcome, prefix: &str) -> bool {
        matches!(outcome, CheckOutcome::Down(r) if r.starts_with(prefix))
    }

    /// Spec Scenario "Handshake only": up, then closed with code 0.
    #[test]
    fn handshake_only_is_up_and_closes_with_code_0() {
        let mut s = server(Serve::Handshake);
        assert_eq!(s.check(json!({})), CheckOutcome::Up);
        match s.next_seen() {
            Seen::Closed(ConnectionError::ApplicationClosed(c)) => {
                assert_eq!(u64::from(c.error_code), 0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn alpn_without_overlap_is_a_tls_failure() {
        let s = server(Serve::Handshake);
        assert_eq!(
            s.check(json!({"alpn": ["hq-interop"]})),
            down("tls: no application protocol (ALPN)")
        );
    }

    #[test]
    fn untrusted_certificate() {
        let s = server(Serve::Handshake);
        let other = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(other.path(), pki(NAME).ca_pem).unwrap();
        let outcome = s.check(json!({"ca_file": other.path()}));
        assert!(down_starting(&outcome, "tls: "), "{outcome:?}");
        assert_eq!(
            s.check(json!({"ca_file": other.path(), "tls_verify": false})),
            CheckOutcome::Up
        );
    }

    #[test]
    fn server_name_must_match_the_certificate() {
        let s = server(Serve::Handshake);
        let outcome = s.check(json!({"server_name": "other.internal"}));
        assert!(down_starting(&outcome, "tls: "), "{outcome:?}");
    }

    #[test]
    fn refused_connection_is_closed_by_peer() {
        let s = server(Serve::Refuse);
        assert_eq!(s.check(json!({})), down("quic: closed by peer: 2"));
    }

    #[test]
    fn peer_close_with_a_reason() {
        let s = server(Serve::CloseBusy);
        assert_eq!(
            s.check(json!({"http3": true})),
            down("quic: closed by peer: 16: busy")
        );
    }

    /// Spec Scenario "Nothing answers on the UDP port".
    #[test]
    fn nothing_answers_on_the_udp_port() {
        let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = silent.local_addr().unwrap();
        // A ca_file, so loading the system's CA certificates (slow the
        // first time on macOS) is not part of the time measured.
        let ca = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(ca.path(), pki(NAME).ca_pem).unwrap();
        let req = CheckRequest {
            service: "web".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: options(json!({"ca_file": ca.path()})),
            timeout: Duration::from_millis(900),
        };
        let settings = parse(&req.options, false).unwrap();
        let start = Instant::now();
        assert_eq!(RUNTIME.block_on(check(&req, &settings)), down("timeout"));
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(850) && took < Duration::from_millis(1500),
            "{took:?}"
        );
    }

    #[test]
    fn missing_ca_file_is_an_error() {
        let s = server(Serve::Handshake);
        let req = s.req(json!({"ca_file": "/nonexistent/ca.pem"}));
        let settings = parse(&req.options, false).unwrap();
        let outcome = RUNTIME.block_on(check(&req, &settings));
        assert!(
            matches!(&outcome, CheckOutcome::Error(r) if r.starts_with("ca_file: ")),
            "{outcome:?}"
        );
    }

    #[test]
    fn reasons_for_connection_errors() {
        use quinn::{ApplicationClose, ConnectionClose, TransportErrorCode};
        let closed = |code: TransportErrorCode, reason: &'static [u8]| {
            ConnectionError::ConnectionClosed(ConnectionClose {
                error_code: code,
                frame_type: None,
                reason: Bytes::from_static(reason),
            })
        };
        assert_eq!(
            connection_error(&ConnectionError::VersionMismatch),
            down("quic: version negotiation failed")
        );
        assert_eq!(
            connection_error(&ConnectionError::Reset),
            down("quic: stateless reset")
        );
        assert_eq!(
            connection_error(&closed(TransportErrorCode::crypto(0x78), b"")),
            down("tls: no application protocol (ALPN)")
        );
        assert_eq!(
            connection_error(&closed(TransportErrorCode::crypto(0x2a), b"")),
            down("tls: alert 42")
        );
        assert_eq!(
            connection_error(&closed(TransportErrorCode::CONNECTION_REFUSED, b"busy")),
            down("quic: closed by peer: 2: busy")
        );
        assert_eq!(
            connection_error(&ConnectionError::ApplicationClosed(ApplicationClose {
                error_code: VarInt::from_u32(16),
                reason: Bytes::from_static(b"busy"),
            })),
            down("quic: closed by peer: 16: busy")
        );
        assert_eq!(
            connection_error(&ConnectionError::TimedOut),
            down("quic: timed out")
        );
    }

    /// Spec Example "QUIC and HTTP/3 outcomes", the `http3 = true` rows.
    #[test]
    fn http3_outcomes() {
        let rows = [
            (Serve::Http(200, None, ""), json!({}), CheckOutcome::Up),
            (
                Serve::Http(302, Some(("location", "/login")), ""),
                json!({}),
                CheckOutcome::Up,
            ),
            (
                Serve::Http(503, None, ""),
                json!({}),
                down("unexpected status 503"),
            ),
            (
                Serve::Http(200, None, "all ok"),
                json!({"expect_body": "ok"}),
                CheckOutcome::Up,
            ),
            (
                Serve::Http(200, None, "fail"),
                json!({"expect_body": "ok"}),
                down("expect_body: got \"fail\""),
            ),
            (
                Serve::Http(503, None, "ok"),
                json!({"expect_body": "ok"}),
                down("unexpected status 503"),
            ),
            (
                Serve::Http(204, None, ""),
                json!({"expect_status": [200]}),
                down("unexpected status 204"),
            ),
        ];
        for (serve, extra, want) in rows {
            let s = server(serve);
            let mut o = extra.clone();
            o["http3"] = json!(true);
            assert_eq!(s.check(o), want, "{extra}");
        }
    }

    #[test]
    fn http3_response_headers_are_limited() {
        let big: &'static str = Box::leak("x".repeat(MAX_FIELD_SECTION as usize).into_boxed_str());
        let s = server(Serve::Http(200, Some(("x-big", big)), ""));
        let got = s.check(json!({"http3": true}));
        assert!(
            matches!(&got, CheckOutcome::Down(r) if r.starts_with("query: HTTP/3: ")),
            "{got:?}"
        );
    }

    #[test]
    fn http3_authority_and_user_agent() {
        let agent = concat!("emfor-check/", env!("CARGO_PKG_VERSION")).to_owned();
        let mut s = server(Serve::Http(200, None, ""));
        assert_eq!(
            s.check(json!({"http3": true, "host_header": "api.internal"})),
            CheckOutcome::Up
        );
        assert_eq!(
            s.next_request(),
            Seen::Request("api.internal".into(), agent.clone())
        );
        assert_eq!(s.check(json!({"http3": true})), CheckOutcome::Up);
        assert_eq!(s.next_request(), Seen::Request(NAME.into(), agent.clone()));
        let port = s.addr.port();
        assert_eq!(
            s.check(json!({"http3": true, "server_name": null, "tls_verify": false})),
            CheckOutcome::Up
        );
        assert_eq!(
            s.next_request(),
            Seen::Request(format!("127.0.0.1:{port}"), agent)
        );
    }

    /// With `http3 = true` the connection ends with `H3_NO_ERROR`.
    #[test]
    fn http3_closes_with_h3_no_error() {
        let mut s = server(Serve::Http(200, None, ""));
        assert_eq!(s.check(json!({"http3": true})), CheckOutcome::Up);
        loop {
            match s.next_seen() {
                Seen::Request(..) => continue,
                Seen::Closed(ConnectionError::ApplicationClosed(c)) => {
                    assert_eq!(u64::from(c.error_code), 0x100);
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn http3_reset_stream() {
        let s = server(Serve::Reset);
        let outcome = s.check(json!({"http3": true}));
        assert!(down_starting(&outcome, "query: HTTP/3: "), "{outcome:?}");
    }

    /// Rule 10: a failure while reading the body.
    #[test]
    fn body_read_failure() {
        let s = server(Serve::ResetInBody);
        let outcome = s.check(json!({"http3": true, "expect_body": "ok"}));
        assert!(down_starting(&outcome, "body: HTTP/3: "), "{outcome:?}");
    }

    /// Rule 5 comes before rule 10: the peer closing the connection while
    /// the body is read is reported as the close.
    #[test]
    fn peer_close_while_reading_the_body() {
        let s = server(Serve::CloseInBody);
        assert_eq!(
            s.check(json!({"http3": true, "expect_body": "ok"})),
            down("quic: closed by peer: 16: busy")
        );
    }

    #[test]
    fn authority_for_ipv6() {
        let settings = parse(&options(json!({"http3": true})), false).unwrap();
        let h = settings.http3.clone().unwrap();
        assert_eq!(
            authority(&h, &settings, "::1".parse().unwrap(), 443),
            "[::1]:443"
        );
        // An IPv6 `server_name` is bracketed, so the request URI parses.
        let settings = parse(
            &options(json!({"http3": true, "server_name": "2001:db8::1"})),
            false,
        )
        .unwrap();
        let a = authority(&h, &settings, "::1".parse().unwrap(), 443);
        assert_eq!(a, "[2001:db8::1]");
        assert!(Request::get(format!("https://{a}/")).body(()).is_ok());
    }
}
