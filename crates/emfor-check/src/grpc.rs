//! The `grpc` plugin: one `grpc.health.v1.Health/Check` call over HTTP/2,
//! plaintext (prior knowledge) or TLS with ALPN `h2`.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options};
use http::{HeaderMap, Request};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::net::{self, io_failure};
use crate::options::{TlsSettings, boolean, string, tls_settings};

const KEYS: &[&str] = &["service", "tls", "tls_verify", "server_name", "ca_file"];

/// The longest `service`.
const MAX_SERVICE: usize = 256;
/// The longest response message read.
const MAX_MESSAGE: usize = 65536;
const PATH: &str = "/grpc.health.v1.Health/Check";

/// gRPC status code names, by code.
const CODE_NAMES: [&str; 17] = [
    "OK",
    "CANCELLED",
    "UNKNOWN",
    "INVALID_ARGUMENT",
    "DEADLINE_EXCEEDED",
    "NOT_FOUND",
    "ALREADY_EXISTS",
    "PERMISSION_DENIED",
    "RESOURCE_EXHAUSTED",
    "FAILED_PRECONDITION",
    "ABORTED",
    "OUT_OF_RANGE",
    "UNIMPLEMENTED",
    "INTERNAL",
    "UNAVAILABLE",
    "DATA_LOSS",
    "UNAUTHENTICATED",
];

/// Validated `check_options` of the `grpc` plugin.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    /// The service to ask about; empty for the whole server.
    pub service: String,
    pub tls: Option<TlsSettings>,
}

/// Parse and validate the `grpc` plugin's options; `read_files` also
/// checks that `ca_file` is usable.
pub fn parse(options: &Options, read_files: bool) -> Result<Settings, Vec<OptionError>> {
    let mut errors = Vec::new();
    for key in options.keys() {
        if !KEYS.contains(&key.as_str()) {
            errors.push(OptionError::new(key, "unknown key"));
        }
    }
    let service = match string(options, "service", &mut errors) {
        Some(s) if s.len() <= MAX_SERVICE => s.to_owned(),
        Some(_) => {
            errors.push(OptionError::new("service", "must be at most 256 bytes"));
            String::new()
        }
        None => String::new(),
    };
    let tls_on = boolean(options, "tls", false, &mut errors);
    let tls = tls_settings(
        options,
        tls_on,
        "requires tls = true",
        read_files,
        &mut errors,
    );
    if errors.is_empty() {
        Ok(Settings { service, tls })
    } else {
        Err(errors)
    }
}

/// Run one check.
pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let start = Instant::now();
    let tcp = match net::connect(req.ip, req.port).await {
        Ok(s) => s,
        Err(outcome) => return outcome,
    };
    let authority = authority(settings, req.ip, req.port);
    let Some(tls) = &settings.tls else {
        return call(tcp, "http", &authority, &settings.service).await;
    };
    let config = match net::client_config(tls) {
        Ok(c) => c,
        Err(outcome) => return outcome,
    };
    let mut config = (*config).clone();
    config.alpn_protocols = vec![b"h2".to_vec()];
    let remaining = req.timeout.saturating_sub(start.elapsed());
    let stream =
        match emfor_tls::handshake_with_config(tcp, Arc::new(config), tls, req.ip, remaining).await
        {
            Ok(s) => s,
            Err(f) => return net::tls_outcome(f),
        };
    if stream.get_ref().1.alpn_protocol() != Some(b"h2") {
        return CheckOutcome::Down("tls: server did not select h2 (ALPN)".into());
    }
    call(stream, "https", &authority, &settings.service).await
}

/// `:authority`: `server_name`, or the backend address.
fn authority(settings: &Settings, ip: IpAddr, port: u16) -> String {
    match settings.tls.as_ref().and_then(|t| t.server_name.as_ref()) {
        Some(name) => name.clone(),
        None => match ip {
            IpAddr::V4(v4) => format!("{v4}:{port}"),
            IpAddr::V6(v6) => format!("[{v6}]:{port}"),
        },
    }
}

/// The request body: one uncompressed message holding
/// `HealthCheckRequest { service }`.
fn request_body(service: &str) -> Vec<u8> {
    let mut message = Vec::new();
    if !service.is_empty() {
        message.push(0x0a);
        varint(&mut message, service.len() as u64);
        message.extend_from_slice(service.as_bytes());
    }
    let mut body = vec![0];
    // `service` is at most 256 bytes.
    body.extend_from_slice(
        &u32::try_from(message.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    body.extend_from_slice(&message);
    body
}

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn http2_error(e: h2::Error) -> CheckOutcome {
    match e.get_io() {
        Some(io) => io_failure("query: HTTP/2: ", io),
        None => CheckOutcome::Down(format!("query: HTTP/2: {e}")),
    }
}

/// What came back: headers, the body (as much as is kept) and trailers.
struct Response {
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
    trailers: Option<HeaderMap>,
}

/// Send the health check on a new HTTP/2 connection and judge the answer.
async fn call<S>(io: S, scheme: &str, authority: &str, service: &str) -> CheckOutcome
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (client, connection) = match h2::client::handshake(io).await {
        Ok(pair) => pair,
        Err(e) => return http2_error(e),
    };
    // The connection runs beside the request; it is dropped (and the TCP
    // connection closed) when this check ends.
    let driver = tokio::spawn(connection);
    let result = exchange(client, scheme, authority, service).await;
    driver.abort();
    match result {
        Ok(response) => judge(&response),
        Err(outcome) => outcome,
    }
}

async fn exchange(
    client: h2::client::SendRequest<Bytes>,
    scheme: &str,
    authority: &str,
    service: &str,
) -> Result<Response, CheckOutcome> {
    let mut client = client.ready().await.map_err(http2_error)?;
    let request = Request::builder()
        .method("POST")
        .uri(format!("{scheme}://{authority}{PATH}"))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .header(
            "user-agent",
            concat!("emfor-check/", env!("CARGO_PKG_VERSION")),
        )
        .body(())
        .map_err(|e| CheckOutcome::Error(format!("internal: gRPC request: {e}")))?;
    let (response, mut send) = client.send_request(request, false).map_err(http2_error)?;
    send.send_data(Bytes::from(request_body(service)), true)
        .map_err(http2_error)?;
    let response = response.await.map_err(http2_error)?;
    let status = response.status().as_u16();
    let (parts, mut body) = response.into_parts();
    let mut kept = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.map_err(http2_error)?;
        let _ = body.flow_control().release_capacity(chunk.len());
        // Keep the 5-byte prefix and up to MAX_MESSAGE bytes after it;
        // the rest is read only to reach the trailers.
        let room = (5 + MAX_MESSAGE + 1).saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..chunk.len().min(room)]);
    }
    let trailers = body.trailers().await.map_err(http2_error)?;
    Ok(Response {
        status,
        headers: parts.headers,
        body: kept,
        trailers,
    })
}

fn judge(response: &Response) -> CheckOutcome {
    let down = |r: String| CheckOutcome::Down(r);
    if response.status != 200 {
        return down(format!("query: HTTP status {}", response.status));
    }
    let content_type = header(&response.headers, "content-type").unwrap_or_default();
    if !content_type.starts_with("application/grpc") {
        return down(format!(
            "query: not a gRPC response (content-type {content_type})"
        ));
    }
    // Trailers-Only responses carry grpc-status in the headers.
    let fields = response
        .trailers
        .as_ref()
        .filter(|t| t.contains_key("grpc-status"));
    let fields = fields.unwrap_or(&response.headers);
    let Some(code) = header(fields, "grpc-status") else {
        return down("query: no grpc-status".into());
    };
    if code != "0" {
        let name = code
            .parse::<usize>()
            .ok()
            .and_then(|c| CODE_NAMES.get(c))
            .copied()
            .unwrap_or("UNKNOWN_CODE");
        let message = header(fields, "grpc-message")
            .map(|m| percent_decode(&m))
            .filter(|m| !m.is_empty());
        return down(match message {
            Some(m) => format!("grpc: {code} {name}: {m}"),
            None => format!("grpc: {code} {name}"),
        });
    }
    let body = &response.body;
    let Some((&flag, rest)) = body.split_first() else {
        return down("query: no message".into());
    };
    if flag != 0 {
        return down("query: compressed message".into());
    }
    let Some(len) = rest
        .get(..4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    else {
        return down("query: malformed message".into());
    };
    let len = len as usize;
    if len > MAX_MESSAGE {
        return down("query: reply too large".into());
    }
    let Some(message) = rest.get(4..4 + len) else {
        return down("query: malformed message".into());
    };
    match health_status(message) {
        Some(1) => CheckOutcome::Up,
        Some(0) => down("status: UNKNOWN".into()),
        Some(2) => down("status: NOT_SERVING".into()),
        Some(3) => down("status: SERVICE_UNKNOWN".into()),
        Some(n) => down(format!("status: {n}")),
        None => down("query: malformed message".into()),
    }
}

fn header(map: &HeaderMap, name: &str) -> Option<String> {
    map.get(name)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
}

/// Decode `%XX` escapes (grpc-message is percent-encoded).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Field 1 of a `HealthCheckResponse`; 0 when it is missing, `None` when
/// the message cannot be decoded.
fn health_status(mut message: &[u8]) -> Option<u64> {
    let mut status = 0;
    while !message.is_empty() {
        let key = read_varint(&mut message)?;
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                let v = read_varint(&mut message)?;
                if field == 1 {
                    status = v;
                }
            }
            1 => message = message.get(8..)?,
            2 => {
                let len = usize::try_from(read_varint(&mut message)?).ok()?;
                message = message.get(len..)?;
            }
            5 => message = message.get(4..)?,
            _ => return None,
        }
    }
    Some(status)
}

fn read_varint(bytes: &mut &[u8]) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let (&b, rest) = bytes.split_first()?;
        *bytes = rest;
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::net::SocketAddr;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

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

    #[test]
    fn grpc_option_validation() {
        let rows = [
            (json!({}), vec![]),
            (json!({"service": "orders.v1.Orders"}), vec![]),
            (json!({"service": ""}), vec![]),
            (json!({"service": "x".repeat(256)}), vec![]),
            (
                json!({"service": "x".repeat(300)}),
                one("service", "must be at most 256 bytes"),
            ),
            (
                json!({"tls": true, "server_name": "orders.internal"}),
                vec![],
            ),
            (
                json!({"server_name": "orders.internal"}),
                one("server_name", "requires tls = true"),
            ),
            (json!({"service": 1}), one("service", "must be a string")),
            (json!({"tls": 1}), one("tls", "must be a boolean")),
            (
                json!({"authority": "orders"}),
                one("authority", "unknown key"),
            ),
        ];
        for (input, want) in rows {
            assert_eq!(errors(input.clone()), want, "{input}");
        }
        assert_eq!(parse(&options(json!({})), false), Ok(Settings::default()));
    }

    #[test]
    fn request_bytes() {
        assert_eq!(request_body(""), [0, 0, 0, 0, 0]);
        assert_eq!(request_body("ab"), [0, 0, 0, 0, 4, 0x0a, 2, b'a', b'b']);
        let long = "x".repeat(200);
        let body = request_body(&long);
        assert_eq!(&body[..8], &[0, 0, 0, 0, 203, 0x0a, 0xc8, 0x01]);
    }

    #[test]
    fn authority_is_server_name_or_address() {
        let ip4: IpAddr = "10.0.0.1".parse().unwrap();
        let ip6: IpAddr = "2001:db8::1".parse().unwrap();
        let plain = Settings::default();
        assert_eq!(authority(&plain, ip4, 50051), "10.0.0.1:50051");
        assert_eq!(authority(&plain, ip6, 50051), "[2001:db8::1]:50051");
        let named = Settings {
            tls: Some(TlsSettings {
                verify: true,
                server_name: Some("orders.internal".into()),
                ca_file: None,
            }),
            ..Settings::default()
        };
        assert_eq!(authority(&named, ip4, 50051), "orders.internal");
    }

    /// What the fake server answers.
    #[derive(Clone)]
    struct Answer {
        status: u16,
        content_type: &'static str,
        /// Extra response headers (a Trailers-Only response when `body`
        /// and `trailers` are empty).
        headers: Vec<(&'static str, &'static str)>,
        body: Vec<u8>,
        trailers: Vec<(&'static str, &'static str)>,
    }

    fn ok(message: &[u8]) -> Answer {
        let mut body = vec![0];
        body.extend_from_slice(&(message.len() as u32).to_be_bytes());
        body.extend_from_slice(message);
        Answer {
            status: 200,
            content_type: "application/grpc",
            headers: vec![],
            body,
            trailers: vec![("grpc-status", "0")],
        }
    }

    fn headers_only(fields: Vec<(&'static str, &'static str)>) -> Answer {
        Answer {
            status: 200,
            content_type: "application/grpc",
            headers: fields,
            body: vec![],
            trailers: vec![],
        }
    }

    /// The request the fake server saw: path, authority and headers, and
    /// the body.
    type Seen = Arc<Mutex<Option<(http::request::Parts, Vec<u8>)>>>;

    async fn serve_h2<S>(io: S, answer: Answer, seen: Seen)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let Ok(mut conn) = h2::server::handshake(io).await else {
            return;
        };
        let Some(Ok((request, mut respond))) = conn.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let (parts, mut body) = request.into_parts();
            let mut data = Vec::new();
            while let Some(Ok(chunk)) = body.data().await {
                let _ = body.flow_control().release_capacity(chunk.len());
                data.extend_from_slice(&chunk);
            }
            *seen.lock().unwrap() = Some((parts, data));
            let mut response = http::Response::builder()
                .status(answer.status)
                .header("content-type", answer.content_type);
            for (k, v) in &answer.headers {
                response = response.header(*k, *v);
            }
            let response = response.body(()).unwrap();
            let end = answer.body.is_empty() && answer.trailers.is_empty();
            let Ok(mut send) = respond.send_response(response, end) else {
                return;
            };
            if end {
                return;
            }
            if !answer.body.is_empty() {
                send.reserve_capacity(answer.body.len());
                let last = answer.trailers.is_empty();
                if send.send_data(Bytes::from(answer.body), last).is_err() || last {
                    return;
                }
            }
            let mut trailers = HeaderMap::new();
            for (k, v) in &answer.trailers {
                trailers.insert(*k, v.parse().unwrap());
            }
            let _ = send.send_trailers(trailers);
        });
        // Drive the connection until the client goes away.
        while let Some(Ok(_)) = conn.accept().await {}
    }

    async fn fake_server(
        answer: Answer,
        acceptor: Option<tokio_rustls::TlsAcceptor>,
    ) -> (SocketAddr, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Seen = Arc::new(Mutex::new(None));
        let record = seen.clone();
        tokio::spawn(async move {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            match acceptor {
                None => serve_h2(tcp, answer, record).await,
                Some(a) => {
                    if let Ok(tls) = a.accept(tcp).await {
                        serve_h2(tls, answer, record).await;
                    }
                }
            }
        });
        (addr, seen)
    }

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest {
            service: "orders".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: Default::default(),
            timeout: Duration::from_secs(2),
        }
    }

    #[tokio::test]
    async fn grpc_responses() {
        let down = |r: &str| CheckOutcome::Down(r.into());
        let mut big = vec![0];
        big.extend_from_slice(&70_000u32.to_be_bytes());
        big.extend_from_slice(&vec![0; 70_000]);
        let rows: Vec<(&str, Answer, CheckOutcome)> = vec![
            ("serving", ok(&[0x08, 0x01]), CheckOutcome::Up),
            (
                "not serving",
                ok(&[0x08, 0x02]),
                down("status: NOT_SERVING"),
            ),
            (
                "service unknown",
                ok(&[0x08, 0x03]),
                down("status: SERVICE_UNKNOWN"),
            ),
            ("empty message", ok(&[]), down("status: UNKNOWN")),
            ("status 7", ok(&[0x08, 0x07]), down("status: 7")),
            (
                "unknown field first",
                ok(&[0x10, 0x05, 0x08, 0x01]),
                CheckOutcome::Up,
            ),
            (
                "not found",
                headers_only(vec![
                    ("grpc-status", "5"),
                    ("grpc-message", "unknown%20service"),
                ]),
                down("grpc: 5 NOT_FOUND: unknown service"),
            ),
            (
                "unimplemented",
                headers_only(vec![("grpc-status", "12")]),
                down("grpc: 12 UNIMPLEMENTED"),
            ),
            (
                "code 99",
                headers_only(vec![("grpc-status", "99")]),
                down("grpc: 99 UNKNOWN_CODE"),
            ),
            (
                "no trailers",
                Answer {
                    trailers: vec![],
                    ..ok(&[0x08, 0x01])
                },
                down("query: no grpc-status"),
            ),
            (
                "compressed",
                Answer {
                    body: vec![1, 0, 0, 0, 2, 0x08, 0x01],
                    ..ok(&[])
                },
                down("query: compressed message"),
            ),
            (
                "70000 bytes",
                Answer {
                    body: big,
                    ..ok(&[])
                },
                down("query: reply too large"),
            ),
            (
                "no message",
                Answer {
                    body: vec![],
                    ..ok(&[])
                },
                down("query: no message"),
            ),
            (
                "truncated varint",
                ok(&[0x08]),
                down("query: malformed message"),
            ),
            (
                "404",
                Answer {
                    status: 404,
                    ..headers_only(vec![])
                },
                down("query: HTTP status 404"),
            ),
            (
                "text/html",
                Answer {
                    content_type: "text/html",
                    ..ok(&[0x08, 0x01])
                },
                down("query: not a gRPC response (content-type text/html)"),
            ),
        ];
        for (name, answer, want) in rows {
            let (addr, _) = fake_server(answer, None).await;
            assert_eq!(
                check(&req(addr), &Settings::default()).await,
                want,
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn sends_the_health_request() {
        let (addr, seen) = fake_server(ok(&[0x08, 0x01]), None).await;
        let settings = Settings {
            service: "orders.v1.Orders".into(),
            tls: None,
        };
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
        let (parts, body) = seen.lock().unwrap().take().unwrap();
        assert_eq!(parts.method, "POST");
        assert_eq!(parts.uri.path(), PATH);
        assert_eq!(parts.uri.scheme_str(), Some("http"));
        assert_eq!(parts.uri.authority().unwrap().as_str(), addr.to_string());
        assert_eq!(parts.headers["content-type"], "application/grpc");
        assert_eq!(parts.headers["te"], "trailers");
        assert_eq!(
            parts.headers["user-agent"],
            concat!("emfor-check/", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(body, request_body("orders.v1.Orders"));
    }

    #[tokio::test]
    async fn http1_server_is_an_http2_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut tcp, &mut buf).await;
            let _ = tcp
                .write_all(
                    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        });
        match check(&req(addr), &Settings::default()).await {
            CheckOutcome::Down(r) => assert!(r.starts_with("query: HTTP/2: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    fn tls_settings_for(ca: std::path::PathBuf) -> Option<TlsSettings> {
        Some(TlsSettings {
            verify: true,
            server_name: Some("grpc.test".into()),
            ca_file: Some(ca),
        })
    }

    /// A TLS server offering `alpn` (nothing for an empty list).
    async fn tls_server(alpn: &[&[u8]]) -> (SocketAddr, Seen, tempfile::TempDir) {
        use emfor_tls::testing::pki;
        let pki = pki("grpc.test");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ca.pem"), &pki.ca_pem).unwrap();
        let mut config = (*pki.server_config).clone();
        config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let (addr, seen) = fake_server(ok(&[0x08, 0x01]), Some(acceptor)).await;
        (addr, seen, dir)
    }

    #[tokio::test]
    async fn tls_with_alpn_h2() {
        let (addr, seen, dir) = tls_server(&[b"h2"]).await;
        let settings = Settings {
            service: String::new(),
            tls: tls_settings_for(dir.path().join("ca.pem")),
        };
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
        let (parts, _) = seen.lock().unwrap().take().unwrap();
        assert_eq!(parts.uri.scheme_str(), Some("https"));
        assert_eq!(parts.uri.authority().unwrap().as_str(), "grpc.test");
    }

    #[tokio::test]
    async fn tls_without_h2_is_refused() {
        // A server without ALPN completes the handshake and selects
        // nothing.
        let (addr, _, dir) = tls_server(&[]).await;
        let settings = Settings {
            service: String::new(),
            tls: tls_settings_for(dir.path().join("ca.pem")),
        };
        assert_eq!(
            check(&req(addr), &settings).await,
            CheckOutcome::Down("tls: server did not select h2 (ALPN)".into())
        );
        // A server that speaks only http/1.1 refuses the handshake
        // (RFC 7301 section 3.2).
        let (addr, _, dir) = tls_server(&[b"http/1.1"]).await;
        let settings = Settings {
            service: String::new(),
            tls: tls_settings_for(dir.path().join("ca.pem")),
        };
        match check(&req(addr), &settings).await {
            CheckOutcome::Down(r) => assert!(r.starts_with("tls: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("unknown%20service"), "unknown service");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%E4%B8%AD"), "中");
    }
}
