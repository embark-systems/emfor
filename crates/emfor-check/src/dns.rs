//! The `dns` plugin: one DNS query per check, over UDP or TCP, judged by
//! the response code and, optionally, by the records in the answer.

use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options};
use hickory_proto::op::{Message, MessageType, Query};
use hickory_proto::rr::{DNSClass, Name, RecordType};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::net;
use crate::options::{boolean, string};

const KEYS: &[&str] = &[
    "name",
    "type",
    "transport",
    "recursion_desired",
    "expect_rcode",
    "expect_answer",
];

const TYPES: &[(&str, RecordType)] = &[
    ("A", RecordType::A),
    ("AAAA", RecordType::AAAA),
    ("CNAME", RecordType::CNAME),
    ("MX", RecordType::MX),
    ("NS", RecordType::NS),
    ("PTR", RecordType::PTR),
    ("SOA", RecordType::SOA),
    ("SRV", RecordType::SRV),
    ("TXT", RecordType::TXT),
];
const TYPES_MESSAGE: &str = "must be one of A, AAAA, CNAME, MX, NS, PTR, SOA, SRV, TXT";

/// The response codes `expect_rcode` accepts; each one's value is its index.
const RCODES: &[&str] = &[
    "NOERROR", "FORMERR", "SERVFAIL", "NXDOMAIN", "NOTIMP", "REFUSED",
];
const RCODES_MESSAGE: &str = "must be one of NOERROR, FORMERR, SERVFAIL, NXDOMAIN, NOTIMP, REFUSED";

/// Mnemonics of the response codes a header without EDNS can carry
/// (RFC 1035, RFC 2136, RFC 8490), for reasons; the value is the index.
const RCODE_NAMES: &[&str] = &[
    "NOERROR",
    "FORMERR",
    "SERVFAIL",
    "NXDOMAIN",
    "NOTIMP",
    "REFUSED",
    "YXDOMAIN",
    "YXRRSET",
    "NXRRSET",
    "NOTAUTH",
    "NOTZONE",
    "DSOTYPENI",
];

/// The longest response read over TCP: the 2-byte length prefix allows no more.
const MAX_TCP_RESPONSE: usize = 65535;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Udp,
    Tcp,
}

/// Validated `check_options` of the dns plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub name: Name,
    pub record_type: RecordType,
    pub transport: Transport,
    pub recursion_desired: bool,
    /// Accepted response code values.
    pub expect_rcode: Vec<u16>,
    pub expect_answer: bool,
}

/// Parse and validate the dns plugin's `check_options`.
pub fn parse(options: &Options) -> Result<Settings, Vec<OptionError>> {
    let mut errors = Vec::new();
    for key in options.keys() {
        if !KEYS.contains(&key.as_str()) {
            errors.push(OptionError::new(key, "unknown key"));
        }
    }

    let name = match string(options, "name", &mut errors) {
        None if !options.contains_key("name") => {
            errors.push(OptionError::new("name", "missing required key"));
            None
        }
        None => None,
        Some(s) => match domain_name(s) {
            Some(name) => Some(name),
            None => {
                errors.push(OptionError::new("name", "not a valid domain name"));
                None
            }
        },
    };

    let record_type = match string(options, "type", &mut errors) {
        None => RecordType::A,
        Some(s) => match TYPES.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)) {
            Some((_, t)) => *t,
            None => {
                errors.push(OptionError::new("type", TYPES_MESSAGE));
                RecordType::A
            }
        },
    };

    let transport = match string(options, "transport", &mut errors) {
        None | Some("udp") => Transport::Udp,
        Some("tcp") => Transport::Tcp,
        Some(_) => {
            errors.push(OptionError::new("transport", "must be udp or tcp"));
            Transport::Udp
        }
    };

    let recursion_desired = boolean(options, "recursion_desired", true, &mut errors);
    let expect_answer = boolean(options, "expect_answer", false, &mut errors);

    let expect_rcode = match options.get("expect_rcode") {
        None => vec![0],
        Some(Value::Array(list)) if !list.is_empty() => {
            let codes: Option<Vec<u16>> = list
                .iter()
                .map(|v| {
                    let s = v.as_str()?;
                    let i = RCODES.iter().position(|n| n.eq_ignore_ascii_case(s))?;
                    u16::try_from(i).ok()
                })
                .collect();
            codes.unwrap_or_else(|| {
                errors.push(OptionError::new("expect_rcode", RCODES_MESSAGE));
                Vec::new()
            })
        }
        Some(_) => {
            errors.push(OptionError::new(
                "expect_rcode",
                "must be a non-empty array",
            ));
            Vec::new()
        }
    };

    match name {
        Some(name) if errors.is_empty() => Ok(Settings {
            name,
            record_type,
            transport,
            recursion_desired,
            expect_rcode,
            expect_answer,
        }),
        _ => Err(errors),
    }
}

/// The fully qualified name for `s`, or `None` when it is not a valid
/// domain name: at most 253 characters without the trailing dot, labels of
/// 1 to 63 ASCII letters, digits, `-` and `_`; `.` is the root.
fn domain_name(s: &str) -> Option<Name> {
    if s == "." {
        return Some(Name::root());
    }
    let bare = s.strip_suffix('.').unwrap_or(s);
    let valid = !bare.is_empty()
        && bare.len() <= 253
        && bare.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        });
    if !valid {
        return None;
    }
    Name::from_ascii(format!("{bare}.")).ok()
}

/// Run one check.
pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let mut query = Message::query();
    query.metadata.recursion_desired = settings.recursion_desired;
    query.add_query(Query::query(settings.name.clone(), settings.record_type));
    let id = query.metadata.id;
    let bytes = match query.to_vec() {
        Ok(b) => b,
        Err(e) => return CheckOutcome::Error(format!("internal: cannot encode the query: {e}")),
    };
    let response = match settings.transport {
        Transport::Udp => {
            net::udp_exchange(req.ip, req.port, &bytes, req.timeout, |d| {
                match_datagram(d, id, settings)
            })
            .await
        }
        Transport::Tcp => {
            match tokio::time::timeout(req.timeout, tcp_exchange(req, &bytes, id, settings)).await {
                Ok(r) => r,
                Err(_) => Err(CheckOutcome::Down("timeout".into())),
            }
        }
    };
    let response = match response {
        Ok(Ok(message)) => message,
        Ok(Err(outcome)) | Err(outcome) => return outcome,
    };
    judge(&response, settings)
}

/// The UDP matching rule: `None` for a datagram that is not the response
/// to query `id`; a response with the right ID that cannot be parsed is
/// `down`.
fn match_datagram(
    datagram: &[u8],
    id: u16,
    settings: &Settings,
) -> Option<Result<Message, CheckOutcome>> {
    if datagram.len() < 2 || u16::from_be_bytes([datagram[0], datagram[1]]) != id {
        return None;
    }
    let message = match Message::from_vec(datagram) {
        Ok(m) => m,
        Err(e) => return Some(Err(CheckOutcome::Down(format!("query: {e}")))),
    };
    is_response_to(&message, id, settings).then_some(Ok(message))
}

/// Whether `message` is a response with ID `id` to the one question asked.
fn is_response_to(message: &Message, id: u16, settings: &Settings) -> bool {
    message.metadata.message_type == MessageType::Response
        && message.metadata.id == id
        && message.queries.len() == 1
        && same_name(message.queries[0].name(), &settings.name)
        && message.queries[0].query_type() == settings.record_type
        && message.queries[0].query_class() == DNSClass::IN
}

/// Names compared without regard to ASCII case or a trailing dot.
fn same_name(a: &Name, b: &Name) -> bool {
    let plain = |n: &Name| n.to_lowercase().to_ascii().trim_end_matches('.').to_owned();
    plain(a) == plain(b)
}

/// One query over TCP: the outer `Result` is the transport, the inner one
/// what the response says.
async fn tcp_exchange(
    req: &CheckRequest,
    query: &[u8],
    id: u16,
    settings: &Settings,
) -> Result<Result<Message, CheckOutcome>, CheckOutcome> {
    let mut stream = net::connect(req.ip, req.port).await?;
    let Ok(len) = u16::try_from(query.len()) else {
        return Err(CheckOutcome::Error(
            "internal: the query is too long".into(),
        ));
    };
    let mut framed = Vec::with_capacity(query.len() + 2);
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(query);
    stream
        .write_all(&framed)
        .await
        .map_err(|e| net::io_failure("query: ", &e))?;
    let mut prefix = [0u8; 2];
    stream
        .read_exact(&mut prefix)
        .await
        .map_err(|e| net::io_failure("query: ", &e))?;
    let len = usize::from(u16::from_be_bytes(prefix));
    let mut body = vec![0u8; len.min(MAX_TCP_RESPONSE)];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| net::io_failure("query: ", &e))?;
    let message = match Message::from_vec(&body) {
        Ok(m) => m,
        Err(e) => return Ok(Err(CheckOutcome::Down(format!("query: {e}")))),
    };
    if !is_response_to(&message, id, settings) {
        return Ok(Err(CheckOutcome::Down(
            "query: response does not match the query".into(),
        )));
    }
    Ok(Ok(message))
}

/// The result for a response to the query.
fn judge(response: &Message, settings: &Settings) -> CheckOutcome {
    let rcode = u16::from(response.metadata.response_code);
    if !settings.expect_rcode.contains(&rcode) {
        let name = RCODE_NAMES
            .get(usize::from(rcode))
            .map_or_else(|| rcode.to_string(), |n| (*n).to_owned());
        return CheckOutcome::Down(format!("rcode: {name}"));
    }
    if settings.expect_answer
        && !response
            .answers
            .iter()
            .any(|r| r.record_type() == settings.record_type)
    {
        let truncated = if response.metadata.truncation {
            " (truncated; set transport = \"tcp\")"
        } else {
            ""
        };
        return CheckOutcome::Down(format!(
            "answer: no {} records{truncated}",
            settings.record_type
        ));
    }
    CheckOutcome::Up
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn options(value: Value) -> Options {
        let Value::Object(map) = value else {
            panic!("options must be an object")
        };
        map
    }

    fn errors(value: Value) -> Vec<(String, String)> {
        match parse(&options(value)) {
            Ok(_) => Vec::new(),
            Err(errors) => errors.into_iter().map(|e| (e.key, e.message)).collect(),
        }
    }

    fn one(key: &str, message: &str) -> Vec<(String, String)> {
        vec![(key.to_owned(), message.to_owned())]
    }

    #[test]
    fn minimal_options_use_the_defaults() {
        let s = parse(&options(json!({ "name": "ns-check.example.org" }))).unwrap();
        assert_eq!(s.name, Name::from_ascii("ns-check.example.org.").unwrap());
        assert_eq!(s.record_type, RecordType::A);
        assert_eq!(s.transport, Transport::Udp);
        assert!(s.recursion_desired);
        assert_eq!(s.expect_rcode, vec![0]);
        assert!(!s.expect_answer);
    }

    #[test]
    fn dns_option_validation() {
        assert_eq!(errors(json!({})), one("name", "missing required key"));
        assert_eq!(errors(json!({ "name": "example.org." })), Vec::new());
        assert_eq!(errors(json!({ "name": ".", "type": "soa" })), Vec::new());
        assert_eq!(
            errors(json!({ "name": "bad name.example" })),
            one("name", "not a valid domain name")
        );
        assert_eq!(
            errors(json!({ "name": "example.org", "type": "ANY" })),
            one("type", TYPES_MESSAGE)
        );
        assert_eq!(
            errors(json!({ "name": "example.org", "transport": "tls" })),
            one("transport", "must be udp or tcp")
        );
        assert_eq!(
            errors(json!({ "name": "example.org", "expect_rcode": [] })),
            one("expect_rcode", "must be a non-empty array")
        );
        assert_eq!(
            errors(json!({ "name": "example.org", "expect_rcode": ["NOERROR", "BADVERS"] })),
            one("expect_rcode", RCODES_MESSAGE)
        );
        assert_eq!(
            errors(json!({ "name": "example.org", "expect_answer": "yes" })),
            one("expect_answer", "must be a boolean")
        );
        assert_eq!(
            errors(json!({ "name": "example.org", "tls": true })),
            one("tls", "unknown key")
        );
    }

    #[test]
    fn options_parse_as_written() {
        let s = parse(&options(json!({
            "name": "Example.ORG",
            "type": "mx",
            "transport": "tcp",
            "recursion_desired": false,
            "expect_rcode": ["noerror", "NXDOMAIN"],
            "expect_answer": true,
        })))
        .unwrap();
        assert_eq!(s.record_type, RecordType::MX);
        assert_eq!(s.transport, Transport::Tcp);
        assert!(!s.recursion_desired);
        assert_eq!(s.expect_rcode, vec![0, 3]);
        assert!(s.expect_answer);
        assert!(same_name(
            &s.name,
            &Name::from_ascii("example.org").unwrap()
        ));
    }

    #[test]
    fn wrong_value_kinds_are_reported() {
        assert_eq!(
            errors(json!({ "name": 1 })),
            one("name", "must be a string")
        );
        assert_eq!(
            errors(json!({ "name": "a.example", "type": 28 })),
            one("type", "must be a string")
        );
        assert_eq!(
            errors(json!({ "name": "a.example", "expect_rcode": "NOERROR" })),
            one("expect_rcode", "must be a non-empty array")
        );
        let long_label = "a".repeat(64);
        assert_eq!(
            errors(json!({ "name": format!("{long_label}.example") })),
            one("name", "not a valid domain name")
        );
        assert_eq!(
            errors(json!({ "name": "a..example" })),
            one("name", "not a valid domain name")
        );
    }

    // ---- check ----

    use hickory_proto::op::{OpCode, ResponseCode};
    use hickory_proto::rr::rdata::{A, AAAA};
    use hickory_proto::rr::{RData, Record};
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::Duration;
    use tokio::net::{TcpListener, UdpSocket};

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest {
            service: "dns".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: Default::default(),
            timeout: Duration::from_millis(900),
        }
    }

    /// A response to `query` with `rcode`, the TC flag and `answers`.
    fn reply(query: &Message, rcode: ResponseCode, tc: bool, answers: Vec<RData>) -> Vec<u8> {
        let mut m = Message::response(query.metadata.id, OpCode::Query);
        m.metadata.response_code = rcode;
        m.metadata.truncation = tc;
        m.metadata.recursion_desired = query.metadata.recursion_desired;
        for q in &query.queries {
            m.add_query(q.clone());
        }
        let name = query.queries[0].name().clone();
        for rdata in answers {
            m.add_answer(Record::from_rdata(name.clone(), 60, rdata));
        }
        m.to_vec().unwrap()
    }

    fn a_record() -> RData {
        RData::A(A(Ipv4Addr::new(192, 0, 2, 1)))
    }

    /// A UDP server answering the `n`-th query with `answer(query, n)`.
    async fn udp_dns(
        answer: impl Fn(&Message, usize) -> Vec<Vec<u8>> + Send + 'static,
    ) -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let mut n = 0;
            while let Ok((len, from)) = socket.recv_from(&mut buf).await {
                let query = Message::from_vec(&buf[..len]).unwrap();
                for d in answer(&query, n) {
                    let _ = socket.send_to(&d, from).await;
                }
                n += 1;
            }
        });
        addr
    }

    async fn outcome(
        opts: Value,
        answer: impl Fn(&Message) -> Vec<u8> + Send + 'static,
    ) -> CheckOutcome {
        let settings = parse(&options(opts)).unwrap();
        let addr = udp_dns(move |q, _| vec![answer(q)]).await;
        check(&req(addr), &settings).await
    }

    fn down(reason: &str) -> CheckOutcome {
        CheckOutcome::Down(reason.to_owned())
    }

    /// How the fake server answers a query.
    type Answer = fn(&Message) -> Vec<u8>;

    #[tokio::test]
    async fn dns_outcomes() {
        let name = json!({ "name": "ns-check.example.org" });
        let with = |extra: Value| {
            let mut o = name.clone();
            for (k, v) in extra.as_object().unwrap() {
                o[k] = v.clone();
            }
            o
        };
        let cases: Vec<(Value, Answer, CheckOutcome)> = vec![
            (
                name.clone(),
                |q| reply(q, ResponseCode::NoError, false, vec![a_record()]),
                CheckOutcome::Up,
            ),
            (
                name.clone(),
                |q| reply(q, ResponseCode::NoError, false, vec![]),
                CheckOutcome::Up,
            ),
            (
                with(json!({ "expect_answer": true })),
                |q| reply(q, ResponseCode::NoError, false, vec![]),
                down("answer: no A records"),
            ),
            (
                with(json!({ "expect_answer": true })),
                |q| reply(q, ResponseCode::NoError, true, vec![]),
                down("answer: no A records (truncated; set transport = \"tcp\")"),
            ),
            (
                with(json!({ "expect_answer": true, "type": "AAAA" })),
                |q| reply(q, ResponseCode::NoError, false, vec![a_record()]),
                down("answer: no AAAA records"),
            ),
            (
                name.clone(),
                |q| reply(q, ResponseCode::NXDomain, false, vec![]),
                down("rcode: NXDOMAIN"),
            ),
            (
                with(json!({ "expect_rcode": ["NOERROR", "NXDOMAIN"] })),
                |q| reply(q, ResponseCode::NXDomain, false, vec![]),
                CheckOutcome::Up,
            ),
            (
                name.clone(),
                |q| reply(q, ResponseCode::Refused, false, vec![]),
                down("rcode: REFUSED"),
            ),
            (
                name.clone(),
                |q| reply(q, ResponseCode::ServFail, false, vec![]),
                down("rcode: SERVFAIL"),
            ),
        ];
        for (i, (opts, answer, expected)) in cases.into_iter().enumerate() {
            assert_eq!(outcome(opts, answer).await, expected, "row {i}");
        }
    }

    #[tokio::test]
    async fn aaaa_answer_is_up() {
        let r = outcome(
            json!({ "name": "a.example", "type": "AAAA", "expect_answer": true }),
            |q| {
                reply(
                    q,
                    ResponseCode::NoError,
                    false,
                    vec![RData::AAAA(AAAA("2001:db8::1".parse().unwrap()))],
                )
            },
        )
        .await;
        assert_eq!(r, CheckOutcome::Up);
    }

    #[tokio::test]
    async fn unknown_rcode_is_its_decimal_value() {
        let r = outcome(json!({ "name": "a.example" }), |q| {
            reply(q, ResponseCode::Unknown(13), false, vec![])
        })
        .await;
        assert_eq!(r, down("rcode: 13"));
    }

    #[tokio::test]
    async fn garbage_with_the_query_id_is_down_query() {
        let r = outcome(json!({ "name": "a.example" }), |q| {
            let [hi, lo] = q.metadata.id.to_be_bytes();
            vec![hi, lo, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0, 0]
        })
        .await;
        match r {
            CheckOutcome::Down(reason) => assert!(reason.starts_with("query: "), "{reason}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn query_carries_the_options() {
        let settings = parse(&options(json!({
            "name": "Mail.Example.org", "type": "MX", "recursion_desired": false
        })))
        .unwrap();
        let addr = udp_dns(|q, _| {
            let ok = q.metadata.message_type == MessageType::Query
                && q.metadata.op_code == OpCode::Query
                && !q.metadata.recursion_desired
                && q.queries.len() == 1
                && q.queries[0].query_type() == RecordType::MX
                && q.queries[0].query_class() == DNSClass::IN
                && q.additionals.is_empty();
            let rcode = if ok {
                ResponseCode::NoError
            } else {
                ResponseCode::FormErr
            };
            vec![reply(q, rcode, false, vec![])]
        })
        .await;
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
    }

    #[tokio::test]
    async fn response_for_another_question_is_ignored() {
        let settings = parse(&options(json!({ "name": "a.example" }))).unwrap();
        let addr = udp_dns(|q, _| {
            let mut other = q.clone();
            other.queries[0] = Query::query(Name::from_ascii("b.example.").unwrap(), RecordType::A);
            vec![
                reply(&other, ResponseCode::Refused, false, vec![]),
                reply(q, ResponseCode::NoError, false, vec![]),
            ]
        })
        .await;
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
    }

    #[tokio::test]
    async fn response_name_in_other_case_matches() {
        let settings = parse(&options(json!({ "name": "a.example" }))).unwrap();
        let addr = udp_dns(|q, _| {
            let mut other = q.clone();
            other.queries[0] = Query::query(Name::from_ascii("A.EXAMPLE.").unwrap(), RecordType::A);
            vec![reply(&other, ResponseCode::NoError, false, vec![])]
        })
        .await;
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
    }

    /// A TCP server answering one length-prefixed query with `answer`.
    async fn tcp_dns(answer: impl Fn(&Message) -> Vec<u8> + Send + 'static) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut prefix = [0u8; 2];
                stream.read_exact(&mut prefix).await.unwrap();
                let mut body = vec![0u8; usize::from(u16::from_be_bytes(prefix))];
                stream.read_exact(&mut body).await.unwrap();
                let query = Message::from_vec(&body).unwrap();
                let d = answer(&query);
                let mut out = u16::try_from(d.len()).unwrap().to_be_bytes().to_vec();
                out.extend_from_slice(&d);
                stream.write_all(&out).await.unwrap();
            }
        });
        addr
    }

    #[tokio::test]
    async fn dns_over_tcp() {
        let settings = parse(&options(json!({ "name": "a.example", "transport": "tcp" }))).unwrap();
        let addr = tcp_dns(|q| reply(q, ResponseCode::NoError, false, vec![a_record()])).await;
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
    }

    #[tokio::test]
    async fn tcp_response_for_another_query_is_down() {
        let settings = parse(&options(json!({ "name": "a.example", "transport": "tcp" }))).unwrap();
        let addr = tcp_dns(|q| {
            let mut other = q.clone();
            other.metadata.id = q.metadata.id.wrapping_add(1);
            reply(&other, ResponseCode::NoError, false, vec![])
        })
        .await;
        assert_eq!(
            check(&req(addr), &settings).await,
            down("query: response does not match the query")
        );
    }

    #[tokio::test]
    async fn tcp_port_closed_is_down_connect() {
        let settings = parse(&options(json!({ "name": "a.example", "transport": "tcp" }))).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        match check(&req(addr), &settings).await {
            CheckOutcome::Down(reason) => assert!(reason.starts_with("connect: "), "{reason}"),
            other => panic!("expected down, got {other:?}"),
        }
    }
}
