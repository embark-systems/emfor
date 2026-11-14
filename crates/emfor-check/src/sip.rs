//! The `sip` plugin: one OPTIONS request per check (RFC 3261) over UDP, TCP
//! or TLS, judged by the status code of the final response.

use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options};
use emfor_tls::TlsSettings;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::net::{self, Stream, io_failure};
use crate::options::{boolean, string, tls_settings};

const KEYS: &[&str] = &[
    "transport",
    "tls",
    "tls_verify",
    "server_name",
    "ca_file",
    "uri",
    "from",
    "expect_status",
];

const URI_MESSAGE: &str = "must be a sip: or sips: URI";
const STATUS_MESSAGE: &str = "must be a status code from 200 to 699 or a range lo-hi";
/// The longest `uri` or `from`.
const MAX_URI: usize = 256;
/// The longest response read.
const MAX_RESPONSE: usize = 65536;
const DEFAULT_FROM: &str = "sip:emfor@emfor.invalid";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Udp,
    Tcp,
}

/// Validated `check_options` of the sip plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub transport: Transport,
    /// Present only with `tls = true` (and then `transport` is TCP).
    pub tls: Option<TlsSettings>,
    /// `uri`; `None` for the default, `sip:<backend>`.
    pub uri: Option<String>,
    pub from: String,
    /// Inclusive ranges of accepted final status codes.
    pub expect_status: Vec<(u16, u16)>,
}

/// Parse and validate the sip plugin's `check_options`; with `read_files`
/// a `ca_file` is read to check its content.
pub fn parse(options: &Options, read_files: bool) -> Result<Settings, Vec<OptionError>> {
    let mut errors = Vec::new();
    for key in options.keys() {
        if !KEYS.contains(&key.as_str()) {
            errors.push(OptionError::new(key, "unknown key"));
        }
    }

    let transport = match string(options, "transport", &mut errors) {
        None if options.contains_key("transport") => None,
        None | Some("udp") => Some(Transport::Udp),
        Some("tcp") => Some(Transport::Tcp),
        Some(_) => {
            errors.push(OptionError::new("transport", "must be udp or tcp"));
            None
        }
    };
    let tls_on = boolean(options, "tls", false, &mut errors);
    if tls_on && transport != Some(Transport::Tcp) {
        errors.push(OptionError::new("transport", "must be tcp when tls = true"));
    }
    let tls = tls_settings(
        options,
        tls_on,
        "requires tls = true",
        read_files,
        &mut errors,
    );

    let uri = sip_uri(options, "uri", &mut errors);
    let from = sip_uri(options, "from", &mut errors).unwrap_or_else(|| DEFAULT_FROM.to_owned());

    let expect_status = match options.get("expect_status") {
        None => vec![(200, 499)],
        Some(Value::Array(list)) if !list.is_empty() => {
            let ranges: Option<Vec<(u16, u16)>> = list
                .iter()
                .map(|v| v.as_str().and_then(status_range))
                .collect();
            ranges.unwrap_or_else(|| {
                errors.push(OptionError::new("expect_status", STATUS_MESSAGE));
                Vec::new()
            })
        }
        Some(_) => {
            errors.push(OptionError::new(
                "expect_status",
                "must be a non-empty array",
            ));
            Vec::new()
        }
    };

    match transport {
        Some(transport) if errors.is_empty() => Ok(Settings {
            transport,
            tls,
            uri,
            from,
            expect_status,
        }),
        _ => Err(errors),
    }
}

/// A `sip:` or `sips:` URI option of at most 256 bytes without whitespace
/// or control characters.
fn sip_uri(options: &Options, key: &str, errors: &mut Vec<OptionError>) -> Option<String> {
    let s = string(options, key, errors)?;
    let valid = (s.starts_with("sip:") || s.starts_with("sips:"))
        && s.len() <= MAX_URI
        && !s.chars().any(|c| c.is_whitespace() || c.is_control());
    if valid {
        Some(s.to_owned())
    } else {
        errors.push(OptionError::new(key, URI_MESSAGE));
        None
    }
}

/// `"404"` or `"200-299"` as an inclusive range of codes 200 to 699.
fn status_range(s: &str) -> Option<(u16, u16)> {
    let code = |t: &str| -> Option<u16> {
        if t.len() != 3 || !t.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let c: u16 = t.parse().ok()?;
        (200..=699).contains(&c).then_some(c)
    };
    let (lo, hi) = match s.split_once('-') {
        Some((lo, hi)) => (code(lo)?, code(hi)?),
        None => {
            let c = code(s)?;
            (c, c)
        }
    };
    (lo <= hi).then_some((lo, hi))
}

/// The random values that tie a response to its request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ids {
    branch: String,
    tag: String,
    call_id: String,
}

impl Ids {
    fn random() -> Ids {
        let hex = |n: usize| -> String {
            (0..n)
                .map(|_| char::from(b"0123456789abcdef"[usize::from(net::random::<u8>() % 16)]))
                .collect()
        };
        Ids {
            branch: format!("z9hG4bK{}", hex(16)),
            tag: hex(10),
            call_id: format!("{}@emfor", hex(20)),
        }
    }
}

/// The OPTIONS request. `via` is `UDP`, `TCP` or `TLS`; `local` the
/// address the request is sent from.
fn request(uri: &str, from: &str, via: &str, local: SocketAddr, ids: &Ids) -> Vec<u8> {
    format!(
        "OPTIONS {uri} SIP/2.0\r\n\
         Via: SIP/2.0/{via} {local};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n\
         From: <{from}>;tag={tag}\r\n\
         To: <{uri}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 OPTIONS\r\n\
         User-Agent: emfor-check/{version}\r\n\
         Accept: application/sdp\r\n\
         Content-Length: 0\r\n\
         \r\n",
        branch = ids.branch,
        tag = ids.tag,
        call_id = ids.call_id,
        version = env!("CARGO_PKG_VERSION"),
    )
    .into_bytes()
}

/// The default Request-URI: `sip:` (or `sips:` with TLS) and the backend.
fn default_uri(ip: IpAddr, port: u16, tls: bool) -> String {
    let scheme = if tls { "sips" } else { "sip" };
    format!("{scheme}:{}", SocketAddr::new(ip, port))
}

/// What the check needs from a response.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Response {
    code: u16,
    reason: String,
    /// The `branch` parameter of the first Via.
    branch: Option<String>,
    call_id: String,
    /// CSeq number and method.
    cseq: (String, String),
}

impl Response {
    fn answers(&self, ids: &Ids) -> bool {
        self.branch.as_deref() == Some(ids.branch.as_str())
            && self.call_id == ids.call_id
            && self.cseq.0 == "1"
            && self.cseq.1 == "OPTIONS"
    }
}

/// The end of the header section (after the empty line) in `bytes`, when
/// it is complete.
fn header_end(bytes: &[u8]) -> Option<usize> {
    let crlf = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4);
    let lf = bytes.windows(2).position(|w| w == b"\n\n").map(|i| i + 2);
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The headers of a message: the status line and the header section,
/// without the empty line. Returns the response and its Content-Length.
fn parse_head(head: &[u8]) -> Result<(Response, Option<usize>), String> {
    let text = std::str::from_utf8(head).map_err(|_| "query: response is not UTF-8".to_owned())?;
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            // A folded header line continues the one before.
            // Not the status line: folding needs a header before it.
            if lines.len() < 2 {
                return Err("query: malformed response headers".into());
            }
            if let Some(last) = lines.last_mut() {
                last.push(' ');
                last.push_str(line.trim_start());
            }
        } else {
            lines.push(line.to_owned());
        }
    }
    let status = lines
        .first()
        .ok_or_else(|| "query: empty response".to_owned())?;
    let (code, reason) = status_line(status)
        .ok_or_else(|| format!("query: not a SIP/2.0 status line: {}", printable(status)))?;

    let mut via = None;
    let mut call_id = None;
    let mut cseq = None;
    let mut content_length = None;
    for line in &lines[1..] {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| "query: malformed response headers".to_owned())?;
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "via" | "v" if via.is_none() => via = Some(value.to_owned()),
            "call-id" | "i" => call_id = Some(value.to_owned()),
            "cseq" => cseq = Some(value.to_owned()),
            "content-length" | "l" => {
                let n = value
                    .parse::<usize>()
                    .map_err(|_| "query: malformed Content-Length".to_owned())?;
                content_length = Some(n);
            }
            _ => {}
        }
    }
    let via = via.ok_or_else(|| "query: response has no Via header".to_owned())?;
    let call_id = call_id.ok_or_else(|| "query: response has no Call-ID header".to_owned())?;
    let cseq = cseq.ok_or_else(|| "query: response has no CSeq header".to_owned())?;
    let mut parts = cseq.split_whitespace();
    let cseq = (
        parts.next().unwrap_or_default().to_owned(),
        parts.next().unwrap_or_default().to_owned(),
    );
    Ok((
        Response {
            code,
            reason,
            branch: via_branch(&via),
            call_id,
            cseq,
        },
        content_length,
    ))
}

/// `SIP/2.0 <code> <reason>`, the code from 100 to 699; the reason phrase
/// may be empty (RFC 3261 25.1).
fn status_line(line: &str) -> Option<(u16, String)> {
    let rest = line.strip_prefix("SIP/2.0 ")?;
    let (code, reason) = match rest.split_once(' ') {
        Some((code, reason)) => (code, reason),
        None => (rest, ""),
    };
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let code: u16 = code.parse().ok()?;
    (100..=699)
        .contains(&code)
        .then(|| (code, reason.trim().to_owned()))
}

/// The `branch` parameter of the first via-parm in a Via header value.
fn via_branch(via: &str) -> Option<String> {
    let first = via.split(',').next()?;
    first.split(';').skip(1).find_map(|param| {
        let (name, value) = param.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("branch")
            .then(|| value.trim().to_owned())
    })
}

/// At most 64 characters of `s` with control characters replaced, for a
/// reason.
fn printable(s: &str) -> String {
    s.chars()
        .take(64)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// A whole datagram as one response.
fn parse_datagram(datagram: &[u8]) -> Result<Response, String> {
    let head = match header_end(datagram) {
        Some(end) => &datagram[..end],
        None => datagram,
    };
    parse_head(head).map(|(response, _)| response)
}

/// Run one check.
pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let ids = Ids::random();
    let uri = settings
        .uri
        .clone()
        .unwrap_or_else(|| default_uri(req.ip, req.port, settings.tls.is_some()));
    let response = match settings.transport {
        Transport::Udp => udp(req, settings, &uri, &ids).await,
        Transport::Tcp => {
            match tokio::time::timeout(req.timeout, tcp(req, settings, &uri, &ids)).await {
                Ok(r) => r,
                Err(_) => Err(CheckOutcome::Down("timeout".into())),
            }
        }
    };
    match response {
        Ok(response) => judge(&response, settings),
        Err(outcome) => outcome,
    }
}

async fn udp(
    req: &CheckRequest,
    settings: &Settings,
    uri: &str,
    ids: &Ids,
) -> Result<Response, CheckOutcome> {
    let result = net::udp_exchange_from(
        req.ip,
        req.port,
        |local| request(uri, &settings.from, "UDP", local, ids),
        req.timeout,
        |datagram| match parse_datagram(datagram) {
            Err(reason) => Some(Err(CheckOutcome::Down(reason))),
            Ok(r) if !r.answers(ids) || r.code < 200 => None,
            Ok(r) => Some(Ok(r)),
        },
    )
    .await;
    match result {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(outcome)) | Err(outcome) => Err(outcome),
    }
}

async fn tcp(
    req: &CheckRequest,
    settings: &Settings,
    uri: &str,
    ids: &Ids,
) -> Result<Response, CheckOutcome> {
    let start = Instant::now();
    let stream = net::connect(req.ip, req.port).await?;
    let local = stream
        .local_addr()
        .map_err(|e| io_failure("connect: ", &e))?;
    let (mut conn, via): (Box<dyn Stream>, &str) = match &settings.tls {
        None => (Box::new(stream), "TCP"),
        Some(tls) => {
            let remaining = req.timeout.saturating_sub(start.elapsed());
            let stream = emfor_tls::handshake(stream, tls, req.ip, remaining)
                .await
                .map_err(net::tls_outcome)?;
            (Box::new(stream), "TLS")
        }
    };
    conn.write_all(&request(uri, &settings.from, via, local, ids))
        .await
        .map_err(|e| io_failure("query: ", &e))?;

    let mut buf: Vec<u8> = Vec::new();
    loop {
        let (response, used) = read_message(&mut conn, &mut buf).await?;
        buf.drain(..used);
        if !response.answers(ids) {
            return Err(CheckOutcome::Down(
                "query: response does not match the request".into(),
            ));
        }
        if response.code >= 200 {
            return Ok(response);
        }
    }
}

/// Read one response from `conn`, with `buf` holding bytes already read;
/// returns it and how many bytes of `buf` it took.
async fn read_message(
    conn: &mut Box<dyn Stream>,
    buf: &mut Vec<u8>,
) -> Result<(Response, usize), CheckOutcome> {
    let too_long = || CheckOutcome::Down("query: response longer than 65536 bytes".into());
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        // CRLFs before the start-line (keepalives) are ignored on a
        // stream (RFC 3261 7.5).
        let blank = buf
            .iter()
            .take_while(|b| matches!(b, b'\r' | b'\n'))
            .count();
        buf.drain(..blank);
        if let Some(end) = header_end(buf) {
            break end;
        }
        if buf.len() > MAX_RESPONSE {
            return Err(too_long());
        }
        let n = conn
            .read(&mut chunk)
            .await
            .map_err(|e| io_failure("query: ", &e))?;
        if n == 0 {
            return Err(CheckOutcome::Down(
                "query: connection closed before a response".into(),
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let (response, content_length) = parse_head(&buf[..head_end]).map_err(CheckOutcome::Down)?;
    // Checked: a Content-Length near usize::MAX would otherwise wrap total
    // below head_end, and the caller would loop over the same buffered
    // provisional response without ever reading (or timing out).
    let total = head_end
        .checked_add(content_length.unwrap_or(0))
        .filter(|&total| total <= MAX_RESPONSE)
        .ok_or_else(too_long)?;
    while buf.len() < total {
        let n = conn
            .read(&mut chunk)
            .await
            .map_err(|e| io_failure("query: ", &e))?;
        if n == 0 {
            return Err(CheckOutcome::Down(
                "query: connection closed before a response".into(),
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok((response, total))
}

/// The result for a final response.
fn judge(response: &Response, settings: &Settings) -> CheckOutcome {
    if settings
        .expect_status
        .iter()
        .any(|(lo, hi)| (*lo..=*hi).contains(&response.code))
    {
        CheckOutcome::Up
    } else {
        let reason = printable(&response.reason);
        CheckOutcome::Down(
            format!("status: {} {reason}", response.code)
                .trim_end()
                .to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use tokio::net::{TcpListener, UdpSocket};

    fn options(value: Value) -> Options {
        let Value::Object(map) = value else {
            panic!("options must be an object")
        };
        map
    }

    fn errors(value: Value) -> Vec<(String, String)> {
        match parse(&options(value), true) {
            Ok(_) => Vec::new(),
            Err(errors) => errors.into_iter().map(|e| (e.key, e.message)).collect(),
        }
    }

    fn one(key: &str, message: &str) -> Vec<(String, String)> {
        vec![(key.to_owned(), message.to_owned())]
    }

    // ---- options ----

    #[test]
    fn defaults() {
        let s = parse(&Options::new(), true).unwrap();
        assert_eq!(s.transport, Transport::Udp);
        assert_eq!(s.tls, None);
        assert_eq!(s.uri, None);
        assert_eq!(s.from, "sip:emfor@emfor.invalid");
        assert_eq!(s.expect_status, vec![(200, 499)]);
        assert_eq!(
            default_uri("10.0.9.11".parse().unwrap(), 5060, false),
            "sip:10.0.9.11:5060"
        );
    }

    #[test]
    fn sip_option_validation() {
        assert_eq!(errors(json!({})), Vec::new());
        assert_eq!(
            errors(json!({ "transport": "sctp" })),
            one("transport", "must be udp or tcp")
        );
        assert_eq!(
            errors(json!({ "tls": true })),
            one("transport", "must be tcp when tls = true")
        );
        assert_eq!(
            errors(json!({ "transport": "sctp", "tls": true })),
            vec![
                ("transport".to_owned(), "must be udp or tcp".to_owned()),
                (
                    "transport".to_owned(),
                    "must be tcp when tls = true".to_owned()
                ),
            ]
        );
        assert_eq!(
            errors(json!({ "transport": "tcp", "tls": true, "server_name": "sbc.example.org" })),
            Vec::new()
        );
        assert_eq!(
            errors(json!({ "transport": "tcp", "server_name": "sbc.example.org" })),
            one("server_name", "requires tls = true")
        );
        assert_eq!(
            errors(json!({ "uri": "tel:+1234" })),
            one("uri", URI_MESSAGE)
        );
        assert_eq!(
            errors(json!({ "from": "sip:probe@example.org" })),
            Vec::new()
        );
        assert_eq!(
            errors(json!({ "expect_status": [] })),
            one("expect_status", "must be a non-empty array")
        );
        assert_eq!(
            errors(json!({ "expect_status": ["200", "180"] })),
            one("expect_status", STATUS_MESSAGE)
        );
        assert_eq!(
            errors(json!({ "expect_status": ["499-200"] })),
            one("expect_status", STATUS_MESSAGE)
        );
        assert_eq!(
            errors(json!({ "method": "REGISTER" })),
            one("method", "unknown key")
        );
    }

    #[test]
    fn other_option_errors() {
        assert_eq!(errors(json!({ "uri": "sip:a b" })), one("uri", URI_MESSAGE));
        assert_eq!(
            errors(json!({ "uri": format!("sip:{}", "a".repeat(253)) })),
            one("uri", URI_MESSAGE)
        );
        assert_eq!(
            errors(json!({ "from": 1 })),
            one("from", "must be a string")
        );
        assert_eq!(
            errors(json!({ "tls": "yes" })),
            one("tls", "must be a boolean")
        );
        assert_eq!(
            errors(json!({ "expect_status": "200" })),
            one("expect_status", "must be a non-empty array")
        );
        assert_eq!(
            errors(json!({ "expect_status": ["700"] })),
            one("expect_status", STATUS_MESSAGE)
        );
        let s = parse(
            &options(json!({
                "transport": "tcp", "tls": true, "tls_verify": false,
                "uri": "sips:sbc.example.org", "expect_status": ["200", "401-407"],
            })),
            true,
        )
        .unwrap();
        assert_eq!(s.transport, Transport::Tcp);
        assert!(s.tls.as_ref().is_some_and(|t| !t.verify));
        assert_eq!(s.uri.as_deref(), Some("sips:sbc.example.org"));
        assert_eq!(s.expect_status, vec![(200, 200), (401, 407)]);
    }

    // ---- messages ----

    fn fixed_ids() -> Ids {
        Ids {
            branch: "z9hG4bKabc".into(),
            tag: "t1".into(),
            call_id: "c1@emfor".into(),
        }
    }

    #[test]
    fn request_format() {
        let local: SocketAddr = "10.0.0.5:40000".parse().unwrap();
        let uri = default_uri("10.0.9.11".parse().unwrap(), 5060, false);
        let r = String::from_utf8(request(&uri, DEFAULT_FROM, "UDP", local, &fixed_ids())).unwrap();
        let expected = format!(
            "OPTIONS sip:10.0.9.11:5060 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 10.0.0.5:40000;branch=z9hG4bKabc;rport\r\n\
             Max-Forwards: 70\r\n\
             From: <sip:emfor@emfor.invalid>;tag=t1\r\n\
             To: <sip:10.0.9.11:5060>\r\n\
             Call-ID: c1@emfor\r\n\
             CSeq: 1 OPTIONS\r\n\
             User-Agent: emfor-check/{}\r\n\
             Accept: application/sdp\r\n\
             Content-Length: 0\r\n\r\n",
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(r, expected);
    }

    #[test]
    fn request_format_ipv6_and_tls() {
        let local: SocketAddr = "[2001:db8::5]:40000".parse().unwrap();
        let uri = default_uri("2001:db8::11".parse().unwrap(), 5061, true);
        assert_eq!(uri, "sips:[2001:db8::11]:5061");
        let r = String::from_utf8(request(&uri, DEFAULT_FROM, "TLS", local, &fixed_ids())).unwrap();
        assert!(r.starts_with(
            "OPTIONS sips:[2001:db8::11]:5061 SIP/2.0\r\n\
             Via: SIP/2.0/TLS [2001:db8::5]:40000;branch=z9hG4bKabc;rport\r\n"
        ));
        assert!(r.contains("\r\nTo: <sips:[2001:db8::11]:5061>\r\n"));
    }

    #[test]
    fn random_ids_differ_and_branch_has_the_magic_cookie() {
        let (a, b) = (Ids::random(), Ids::random());
        assert!(a.branch.starts_with("z9hG4bK") && a.branch.len() > 7);
        assert_ne!(a, b);
    }

    #[test]
    fn response_parsing() {
        let ok = "SIP/2.0 200 OK\r\nVia: SIP/2.0/UDP 10.0.0.5:40000;branch=z9hG4bKabc;rport=40000;received=10.0.0.5\r\n\
                  Call-ID: c1@emfor\r\nCSeq: 1 OPTIONS\r\nContent-Length: 0\r\n\r\n";
        let r = parse_datagram(ok.as_bytes()).unwrap();
        assert_eq!((r.code, r.reason.as_str()), (200, "OK"));
        assert!(r.answers(&fixed_ids()));

        // No reason phrase.
        let bare = "SIP/2.0 200\r\nVia: x;branch=z9hG4bKabc\r\nCall-ID: c1@emfor\r\nCSeq: 1 OPTIONS\r\n\r\n";
        let r = parse_datagram(bare.as_bytes()).unwrap();
        assert_eq!((r.code, r.reason.as_str()), (200, ""));

        // Compact forms, any case, folding, a second Via ignored.
        let compact = "SIP/2.0 404 Not Found\r\nv: SIP/2.0/UDP x;BRANCH=z9hG4bKabc\r\n\
                       VIA: SIP/2.0/UDP y;branch=other\r\ni: c1@emfor\r\ncseq:   1\r\n  OPTIONS\r\nl: 0\r\n\r\n";
        let r = parse_datagram(compact.as_bytes()).unwrap();
        assert_eq!(r.code, 404);
        assert!(r.answers(&fixed_ids()));

        for (bytes, reason) in [
            (
                "HTTP/1.1 200 OK\r\n\r\n",
                "query: not a SIP/2.0 status line: HTTP/1.1 200 OK",
            ),
            (
                "SIP/2.0 2000 OK\r\n\r\n",
                "query: not a SIP/2.0 status line: SIP/2.0 2000 OK",
            ),
            (
                "SIP/2.0 200 OK\r\nCall-ID: c\r\nCSeq: 1 OPTIONS\r\n\r\n",
                "query: response has no Via header",
            ),
            (
                "SIP/2.0 200 OK\r\nVia: x\r\nCSeq: 1 OPTIONS\r\n\r\n",
                "query: response has no Call-ID header",
            ),
            (
                "SIP/2.0 200 OK\r\nVia: x\r\nCall-ID: c\r\n\r\n",
                "query: response has no CSeq header",
            ),
            (
                "SIP/2.0 200 OK\r\nno colon\r\n\r\n",
                "query: malformed response headers",
            ),
            (
                "SIP/2.0 200 OK\r\nVia: x\r\nCall-ID: c\r\nCSeq: 1 OPTIONS\r\nl: x\r\n\r\n",
                "query: malformed Content-Length",
            ),
        ] {
            assert_eq!(
                parse_datagram(bytes.as_bytes()),
                Err(reason.to_owned()),
                "{bytes:?}"
            );
        }
    }

    // ---- checks against fake servers ----

    /// A header of a request, by full name.
    fn header<'a>(request: &'a str, name: &str) -> &'a str {
        request
            .split("\r\n")
            .find_map(|l| l.strip_prefix(&format!("{name}: ")))
            .unwrap_or_else(|| panic!("no {name} in {request:?}"))
    }

    /// How the fake server answers a request.
    #[derive(Clone, Copy)]
    struct Reply {
        status: &'static str,
        /// Use another Call-ID.
        other_call: bool,
        compact: bool,
    }

    fn reply(status: &'static str) -> Reply {
        Reply {
            status,
            other_call: false,
            compact: false,
        }
    }

    fn respond(request: &str, r: Reply) -> String {
        if !r.status.starts_with(|c: char| c.is_ascii_digit()) {
            return format!("{}\r\n\r\n", r.status);
        }
        let call_id = if r.other_call {
            "other@x".to_owned()
        } else {
            header(request, "Call-ID").to_owned()
        };
        let (v, i, l) = if r.compact {
            ("v", "i", "l")
        } else {
            ("Via", "Call-ID", "Content-Length")
        };
        format!(
            "SIP/2.0 {}\r\n{v}: {};received=127.0.0.1\r\nFrom: {}\r\nTo: {};tag=srv\r\n\
             {i}: {call_id}\r\nCSeq: {}\r\n{l}: 0\r\n\r\n",
            r.status,
            header(request, "Via"),
            header(request, "From"),
            header(request, "To"),
            header(request, "CSeq"),
        )
    }

    /// A UDP SIP server sending `replies` to each request.
    async fn udp_sip(replies: Vec<Reply>) -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok((n, from)) = socket.recv_from(&mut buf).await {
                let request = String::from_utf8(buf[..n].to_vec()).unwrap();
                for r in &replies {
                    let _ = socket.send_to(respond(&request, *r).as_bytes(), from).await;
                }
            }
        });
        addr
    }

    /// A TCP SIP server writing `replies` to the first request on each
    /// connection.
    async fn tcp_sip(replies: Vec<Reply>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let replies = replies.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while header_end(&buf).is_none() {
                        let n = s.read(&mut chunk).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    let request = String::from_utf8(buf).unwrap();
                    assert!(request.contains("Via: SIP/2.0/TCP "), "{request}");
                    for r in &replies {
                        s.write_all(respond(&request, *r).as_bytes()).await.unwrap();
                    }
                    let _ = s.read(&mut chunk).await;
                });
            }
        });
        addr
    }

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest {
            service: "sip".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: Default::default(),
            timeout: Duration::from_millis(900),
        }
    }

    fn down(reason: &str) -> CheckOutcome {
        CheckOutcome::Down(reason.to_owned())
    }

    async fn outcome(opts: Value, replies: Vec<Reply>) -> CheckOutcome {
        let s = parse(&options(opts), true).unwrap();
        let addr = if s.transport == Transport::Tcp {
            tcp_sip(replies).await
        } else {
            udp_sip(replies).await
        };
        check(&req(addr), &s).await
    }

    #[tokio::test]
    async fn sip_outcomes() {
        let other = Reply {
            other_call: true,
            ..reply("200 OK")
        };
        let cases: Vec<(Value, Vec<Reply>, CheckOutcome)> = vec![
            (json!({}), vec![reply("200 OK")], CheckOutcome::Up),
            (json!({}), vec![reply("404 Not Found")], CheckOutcome::Up),
            (
                json!({}),
                vec![reply("407 Proxy Authentication Required")],
                CheckOutcome::Up,
            ),
            (
                json!({}),
                vec![reply("100 Trying"), reply("200 OK")],
                CheckOutcome::Up,
            ),
            (
                json!({}),
                vec![reply("503 Service Unavailable")],
                down("status: 503 Service Unavailable"),
            ),
            (
                json!({}),
                vec![reply("603 Decline")],
                down("status: 603 Decline"),
            ),
            (
                json!({ "expect_status": ["200"] }),
                vec![reply("404 Not Found")],
                down("status: 404 Not Found"),
            ),
            (json!({}), vec![other, reply("200 OK")], CheckOutcome::Up),
            (
                json!({ "transport": "tcp" }),
                vec![other],
                down("query: response does not match the request"),
            ),
            (
                json!({}),
                vec![reply("HTTP/1.1 200 OK")],
                down("query: not a SIP/2.0 status line: HTTP/1.1 200 OK"),
            ),
            (
                json!({}),
                vec![Reply {
                    compact: true,
                    ..reply("200 OK")
                }],
                CheckOutcome::Up,
            ),
        ];
        for (i, (opts, replies, expected)) in cases.into_iter().enumerate() {
            assert_eq!(outcome(opts, replies).await, expected, "row {i}");
        }
    }

    #[tokio::test]
    async fn server_answers_options() {
        assert_eq!(
            outcome(json!({}), vec![reply("200 OK")]).await,
            CheckOutcome::Up
        );
    }

    #[tokio::test]
    async fn server_overloaded() {
        assert_eq!(
            outcome(json!({}), vec![reply("503 Service Unavailable")]).await,
            down("status: 503 Service Unavailable")
        );
    }

    #[tokio::test]
    async fn tcp_provisional_then_final() {
        let tcp = json!({ "transport": "tcp" });
        assert_eq!(
            outcome(tcp.clone(), vec![reply("100 Trying"), reply("200 OK")]).await,
            CheckOutcome::Up
        );
        assert_eq!(
            outcome(tcp, vec![reply("486 Busy Here")]).await,
            CheckOutcome::Up
        );
    }

    #[tokio::test]
    async fn tcp_response_with_a_body_is_read_whole() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap();
            let request = String::from_utf8(buf[..n].to_vec()).unwrap();
            let provisional = respond(&request, reply("100 Trying"))
                .replace("Content-Length: 0", "Content-Length: 5");
            // The body "hello" would be read as the next response if
            // Content-Length were ignored.
            s.write_all(format!("{provisional}hello").as_bytes())
                .await
                .unwrap();
            s.write_all(respond(&request, reply("200 OK")).as_bytes())
                .await
                .unwrap();
            let _ = s.read(&mut buf).await;
        });
        let s = parse(&options(json!({ "transport": "tcp" })), true).unwrap();
        assert_eq!(check(&req(addr), &s).await, CheckOutcome::Up);
    }

    #[tokio::test]
    async fn tcp_keepalives_before_a_response_are_skipped() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap();
            let request = String::from_utf8(buf[..n].to_vec()).unwrap();
            s.write_all(b"\r\n\r\n").await.unwrap();
            s.write_all(respond(&request, reply("503 Service Unavailable")).as_bytes())
                .await
                .unwrap();
            let _ = s.read(&mut buf).await;
        });
        let s = parse(&options(json!({ "transport": "tcp" })), true).unwrap();
        assert_eq!(
            check(&req(addr), &s).await,
            down("status: 503 Service Unavailable")
        );
    }

    #[tokio::test]
    async fn tcp_response_too_long() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = s.read(&mut buf).await.unwrap();
            let mut big = b"SIP/2.0 200 OK\r\nX: ".to_vec();
            big.extend(std::iter::repeat_n(b'a', MAX_RESPONSE + 10));
            let _ = s.write_all(&big).await;
            let _ = s.read(&mut buf).await;
        });
        let s = parse(&options(json!({ "transport": "tcp" })), true).unwrap();
        assert_eq!(
            check(&req(addr), &s).await,
            down("query: response longer than 65536 bytes")
        );
    }

    #[tokio::test]
    async fn tcp_content_length_near_usize_max_is_too_long() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap();
            let request = String::from_utf8(buf[..n].to_vec()).unwrap();
            // head_end + Content-Length overflows usize.
            let provisional = respond(&request, reply("100 Trying")).replace(
                "Content-Length: 0",
                &format!("Content-Length: {}", usize::MAX),
            );
            s.write_all(provisional.as_bytes()).await.unwrap();
            let _ = s.read(&mut buf).await;
        });
        let s = parse(&options(json!({ "transport": "tcp" })), true).unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(5), check(&req(addr), &s))
            .await
            .expect("check returns instead of looping");
        assert_eq!(outcome, down("query: response longer than 65536 bytes"));
    }

    #[tokio::test]
    async fn tcp_port_closed_is_down_connect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let s = parse(&options(json!({ "transport": "tcp" })), true).unwrap();
        match check(&req(addr), &s).await {
            CheckOutcome::Down(r) => assert!(r.starts_with("connect: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    /// A TLS SIP server with a certificate for `name`; returns its address
    /// and the CA file.
    async fn tls_sip(name: &str) -> (SocketAddr, tempfile::TempDir) {
        use emfor_tls::testing::pki;
        use tokio_rustls::TlsAcceptor;
        let pki = pki(name);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ca.pem"), &pki.ca_pem).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(pki.server_config);
        tokio::spawn(async move {
            while let Ok((s, _)) = listener.accept().await {
                let Ok(mut tls) = acceptor.accept(s).await else {
                    continue;
                };
                let mut buf = vec![0u8; 4096];
                let n = tls.read(&mut buf).await.unwrap();
                let request = String::from_utf8(buf[..n].to_vec()).unwrap();
                assert!(request.contains("Via: SIP/2.0/TLS "), "{request}");
                assert!(request.starts_with("OPTIONS sips:"), "{request}");
                tls.write_all(respond(&request, reply("200 OK")).as_bytes())
                    .await
                    .unwrap();
                let _ = tls.read(&mut buf).await;
            }
        });
        (addr, dir)
    }

    #[tokio::test]
    async fn sip_over_tls() {
        let (addr, dir) = tls_sip("sbc.example.org").await;
        let ca = dir.path().join("ca.pem");
        let opts = |name: &str| {
            options(json!({
                "transport": "tcp", "tls": true, "server_name": name,
                "ca_file": ca.to_str().unwrap(),
            }))
        };
        let s = parse(&opts("sbc.example.org"), true).unwrap();
        assert_eq!(check(&req(addr), &s).await, CheckOutcome::Up);
        let s = parse(&opts("other.example.org"), true).unwrap();
        match check(&req(addr), &s).await {
            CheckOutcome::Down(r) => assert!(r.starts_with("tls: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn udp_request_names_the_local_address() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            let (n, from) = socket.recv_from(&mut buf).await.unwrap();
            let request = String::from_utf8(buf[..n].to_vec()).unwrap();
            let _ = socket
                .send_to(respond(&request, reply("200 OK")).as_bytes(), from)
                .await;
            (request, from)
        });
        let s = parse(&Options::new(), true).unwrap();
        assert_eq!(check(&req(addr), &s).await, CheckOutcome::Up);
        let (request, from) = server.await.unwrap();
        assert!(
            request.contains(&format!("Via: SIP/2.0/UDP {from};branch=z9hG4bK")),
            "{request}"
        );
        assert!(request.starts_with(&format!("OPTIONS sip:{addr} SIP/2.0\r\n")));
    }
}
