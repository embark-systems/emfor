//! The `amqp` plugin: an AMQP 0-9-1 connection handshake (RabbitMQ and
//! other 0-9-1 brokers) up to Connection.Open-Ok, with a PLAIN login.

use std::path::PathBuf;
use std::time::Duration;

use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::net::{self, Stream, io_failure};
use crate::options::{
    TlsSettings, boolean, check_password_file, file, read_password, string, tls_settings,
};

const KEYS: &[&str] = &[
    "user",
    "password_file",
    "vhost",
    "tls",
    "tls_verify",
    "server_name",
    "ca_file",
];

/// The longest `user` and `vhost` (a short string on the wire).
const MAX_NAME: usize = 255;

/// The largest frame payload read, and the frame-max sent when the broker
/// proposes none.
const MAX_FRAME: u32 = 131_072;

/// How long to wait for Connection.Close-Ok at most.
const CLOSE_WAIT: Duration = Duration::from_millis(200);

const PROTOCOL_HEADER: &[u8; 8] = b"AMQP\x00\x00\x09\x01";
const FRAME_METHOD: u8 = 1;
const FRAME_HEARTBEAT: u8 = 8;
const FRAME_END: u8 = 0xCE;

/// Class `connection` and its methods.
const CONNECTION: u16 = 10;
const START: u16 = 10;
const START_OK: u16 = 11;
const TUNE: u16 = 30;
const TUNE_OK: u16 = 31;
const OPEN: u16 = 40;
const OPEN_OK: u16 = 41;
const CLOSE: u16 = 50;
const CLOSE_OK: u16 = 51;

/// Reply code of a normal Connection.Close.
const REPLY_SUCCESS: u16 = 200;

/// Validated `check_options` of the `amqp` plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub user: String,
    pub password_file: PathBuf,
    pub vhost: String,
    pub tls: Option<TlsSettings>,
}

/// Parse and validate the `amqp` plugin's options; `read_files` also
/// checks that `password_file` and `ca_file` are usable.
pub fn parse(options: &Options, read_files: bool) -> Result<Settings, Vec<OptionError>> {
    let mut errors = Vec::new();
    for key in options.keys() {
        if !KEYS.contains(&key.as_str()) {
            errors.push(OptionError::new(key, "unknown key"));
        }
    }
    let user = name(options, "user", &mut errors);
    if !options.contains_key("user") {
        errors.push(OptionError::new("user", "missing required key"));
    }
    let password_file = file(
        options,
        "password_file",
        read_files,
        check_password_file,
        &mut errors,
    );
    if !options.contains_key("password_file") {
        errors.push(OptionError::new("password_file", "missing required key"));
    }
    let vhost = name(options, "vhost", &mut errors).unwrap_or_else(|| "/".to_owned());
    let tls_on = boolean(options, "tls", false, &mut errors);
    let tls = tls_settings(
        options,
        tls_on,
        "requires tls = true",
        read_files,
        &mut errors,
    );
    match (user, password_file) {
        (Some(user), Some(password_file)) if errors.is_empty() => Ok(Settings {
            user,
            password_file,
            vhost,
            tls,
        }),
        _ => Err(errors),
    }
}

/// A string option of 1 to 255 bytes.
fn name(options: &Options, key: &str, errors: &mut Vec<OptionError>) -> Option<String> {
    match string(options, key, errors) {
        Some(s) if (1..=MAX_NAME).contains(&s.len()) => Some(s.to_owned()),
        Some(_) => {
            errors.push(OptionError::new(key, "must be 1 to 255 bytes"));
            None
        }
        None => None,
    }
}

/// Run one check.
pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let password = match read_password(&settings.password_file) {
        Ok(p) => p,
        Err(reason) => return CheckOutcome::Error(reason),
    };
    let stream = match net::open(req.ip, req.port, settings.tls.as_ref(), req.timeout).await {
        Ok(s) => s,
        Err(outcome) => return outcome,
    };
    let mut conn = Connection { stream };
    match handshake(&mut conn, settings, &password).await {
        Ok(()) => {
            // The result does not wait for the broker to confirm the close.
            tokio::spawn(conn.close());
            CheckOutcome::Up
        }
        Err(outcome) => outcome,
    }
}

/// Where the handshake is, for the reason when the broker closes the
/// connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Before the login: a Close is `query: `.
    Start,
    /// After Start-Ok, before Tune: the broker judges the login.
    Login,
    /// After Open: the broker judges the vhost.
    Open,
}

impl Phase {
    fn prefix(self) -> &'static str {
        match self {
            Phase::Login => "auth",
            Phase::Open => "vhost",
            Phase::Start => "query",
        }
    }

    fn closed(self) -> CheckOutcome {
        match self {
            Phase::Login => CheckOutcome::Down("auth: connection closed after login".into()),
            _ => CheckOutcome::Down("query: connection closed".into()),
        }
    }
}

async fn handshake(
    conn: &mut Connection,
    settings: &Settings,
    password: &str,
) -> Result<(), CheckOutcome> {
    conn.write(PROTOCOL_HEADER).await?;
    let start = conn.expect(Phase::Start, START).await?;
    let mechanisms = start_mechanisms(&start).ok_or_else(malformed)?;
    if !mechanisms.split(' ').any(|m| m == "PLAIN") {
        return Err(CheckOutcome::Down(
            "auth: server does not offer PLAIN".into(),
        ));
    }
    conn.send_method(START_OK, &start_ok(&settings.user, password))
        .await?;
    let tune = conn.expect(Phase::Login, TUNE).await?;
    let (channel_max, frame_max) = tune_values(&tune).ok_or_else(malformed)?;
    let frame_max = if frame_max == 0 { MAX_FRAME } else { frame_max };
    conn.send_method(TUNE_OK, &tune_ok(channel_max, frame_max))
        .await?;
    conn.send_method(OPEN, &open(&settings.vhost)).await?;
    conn.expect(Phase::Open, OPEN_OK).await?;
    Ok(())
}

fn malformed() -> CheckOutcome {
    CheckOutcome::Down("query: malformed frame".into())
}

/// An AMQP connection on channel 0.
struct Connection {
    stream: Box<dyn Stream>,
}

impl Connection {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), CheckOutcome> {
        self.stream
            .write_all(bytes)
            .await
            .map_err(|e| io_failure("query: ", &e))?;
        self.stream
            .flush()
            .await
            .map_err(|e| io_failure("query: ", &e))
    }

    async fn send_method(&mut self, method: u16, args: &[u8]) -> Result<(), CheckOutcome> {
        self.write(&method_frame(method, args)).await
    }

    /// Read the next method of class `connection`, skipping heartbeats,
    /// and return its arguments when it is `method`. A Close from the
    /// broker is answered with Close-Ok and becomes the reason.
    async fn expect(&mut self, phase: Phase, method: u16) -> Result<Vec<u8>, CheckOutcome> {
        loop {
            let (kind, payload) = self.frame(phase).await?;
            if kind == FRAME_HEARTBEAT {
                continue;
            }
            if kind != FRAME_METHOD || payload.len() < 4 {
                return Err(malformed());
            }
            let class = u16::from_be_bytes([payload[0], payload[1]]);
            let id = u16::from_be_bytes([payload[2], payload[3]]);
            let args = payload[4..].to_vec();
            if (class, id) == (CONNECTION, method) {
                return Ok(args);
            }
            if (class, id) == (CONNECTION, CLOSE) {
                let (code, text) = close_reason(&args).ok_or_else(malformed)?;
                let _ = self.send_method(CLOSE_OK, &[]).await;
                let _ = self.stream.shutdown().await;
                return Err(CheckOutcome::Down(format!(
                    "{}: {code} {text}",
                    phase.prefix()
                )));
            }
            return Err(CheckOutcome::Down(format!(
                "query: unexpected method {class}.{id}"
            )));
        }
    }

    /// One frame: its type and payload.
    async fn frame(&mut self, phase: Phase) -> Result<(u8, Vec<u8>), CheckOutcome> {
        let mut header = [0u8; 7];
        self.read_exact(&mut header, phase).await?;
        if header.starts_with(b"AMQP") {
            // The broker does not speak 0-9-1 and answered with its own
            // protocol header.
            let mut last = [0u8; 1];
            self.read_exact(&mut last, phase).await?;
            return Err(CheckOutcome::Down(format!(
                "query: server speaks AMQP {}-{}-{}",
                header[5], header[6], last[0]
            )));
        }
        let size = u32::from_be_bytes([header[3], header[4], header[5], header[6]]);
        if size > MAX_FRAME {
            return Err(CheckOutcome::Down("query: frame too large".into()));
        }
        let len = usize::try_from(size).map_err(|_| malformed())?;
        let mut payload = vec![0u8; len + 1];
        self.read_exact(&mut payload, phase).await?;
        if payload.pop() != Some(FRAME_END) {
            return Err(malformed());
        }
        Ok((header[0], payload))
    }

    async fn read_exact(&mut self, buf: &mut [u8], phase: Phase) -> Result<(), CheckOutcome> {
        match self.stream.read_exact(buf).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(phase.closed()),
            Err(e) => Err(io_failure("query: ", &e)),
        }
    }

    /// Close the connection normally: Close, then Close-Ok within
    /// [`CLOSE_WAIT`].
    async fn close(mut self) {
        let mut args = Vec::new();
        args.extend_from_slice(&REPLY_SUCCESS.to_be_bytes());
        short_string(&mut args, b"");
        args.extend_from_slice(&[0, 0, 0, 0]);
        if self.send_method(CLOSE, &args).await.is_ok() {
            let _ = tokio::time::timeout(CLOSE_WAIT, self.expect(Phase::Start, CLOSE_OK)).await;
        }
        let _ = self.stream.shutdown().await;
    }
}

/// A method frame of class `connection` on channel 0.
fn method_frame(method: u16, args: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(4 + args.len());
    payload.extend_from_slice(&CONNECTION.to_be_bytes());
    payload.extend_from_slice(&method.to_be_bytes());
    payload.extend_from_slice(args);
    let mut frame = vec![FRAME_METHOD, 0, 0];
    // Every frame this plugin sends is far below 4 GiB.
    let size = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    frame.extend_from_slice(&size.to_be_bytes());
    frame.extend_from_slice(&payload);
    frame.push(FRAME_END);
    frame
}

/// Start-Ok: client properties, mechanism PLAIN, the PLAIN response and
/// the locale.
fn start_ok(user: &str, password: &str) -> Vec<u8> {
    let mut capabilities = Vec::new();
    field(
        &mut capabilities,
        b"authentication_failure_close",
        b't',
        &[1],
    );
    let mut properties = Vec::new();
    field(&mut properties, b"product", b'S', &long_bytes(b"emfor"));
    field(
        &mut properties,
        b"version",
        b'S',
        &long_bytes(env!("CARGO_PKG_VERSION").as_bytes()),
    );
    field(
        &mut properties,
        b"capabilities",
        b'F',
        &long_bytes(&capabilities),
    );

    let mut response = vec![0];
    response.extend_from_slice(user.as_bytes());
    response.push(0);
    response.extend_from_slice(password.as_bytes());

    let mut args = long_bytes(&properties);
    short_string(&mut args, b"PLAIN");
    args.extend_from_slice(&long_bytes(&response));
    short_string(&mut args, b"en_US");
    args
}

fn tune_ok(channel_max: u16, frame_max: u32) -> Vec<u8> {
    let mut args = Vec::new();
    args.extend_from_slice(&channel_max.to_be_bytes());
    args.extend_from_slice(&frame_max.to_be_bytes());
    args.extend_from_slice(&0u16.to_be_bytes());
    args
}

/// Open: the vhost, the reserved capabilities string and the reserved
/// insist bit.
fn open(vhost: &str) -> Vec<u8> {
    let mut args = Vec::new();
    short_string(&mut args, vhost.as_bytes());
    short_string(&mut args, b"");
    args.push(0);
    args
}

/// A table field: name, type and the encoded value.
fn field(table: &mut Vec<u8>, name: &[u8], kind: u8, value: &[u8]) {
    short_string(table, name);
    table.push(kind);
    table.extend_from_slice(value);
}

/// A short string; callers keep it within 255 bytes.
fn short_string(out: &mut Vec<u8>, s: &[u8]) {
    let len = s.len().min(MAX_NAME);
    out.push(u8::try_from(len).unwrap_or(u8::MAX));
    out.extend_from_slice(&s[..len]);
}

/// A long string (or a table): a 32-bit length, then the bytes.
fn long_bytes(s: &[u8]) -> Vec<u8> {
    let len = u32::try_from(s.len()).unwrap_or(u32::MAX);
    let mut out = len.to_be_bytes().to_vec();
    out.extend_from_slice(s);
    out
}

/// Reads method arguments.
struct Args<'a> {
    rest: &'a [u8],
}

impl<'a> Args<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.rest.len() < n {
            return None;
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Some(head)
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn short(&mut self) -> Option<&'a [u8]> {
        let len = *self.take(1)?.first()?;
        self.take(usize::from(len))
    }

    fn long(&mut self) -> Option<&'a [u8]> {
        let len = usize::try_from(self.u32()?).ok()?;
        self.take(len)
    }
}

/// The mechanisms of a Start: after the version and the server
/// properties table.
fn start_mechanisms(args: &[u8]) -> Option<String> {
    let mut a = Args { rest: args };
    a.take(2)?;
    a.long()?;
    Some(String::from_utf8_lossy(a.long()?).into_owned())
}

/// The channel-max and frame-max a Tune proposes.
fn tune_values(args: &[u8]) -> Option<(u16, u32)> {
    let mut a = Args { rest: args };
    let channel_max = a.u16()?;
    let frame_max = a.u32()?;
    a.u16()?;
    Some((channel_max, frame_max))
}

/// The reply code and text of a Close.
fn close_reason(args: &[u8]) -> Option<(u16, String)> {
    let mut a = Args { rest: args };
    let code = a.u16()?;
    let text = String::from_utf8_lossy(a.short()?).into_owned();
    Some((code, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
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

    fn err(key: &str, message: &str) -> (String, String) {
        (key.to_owned(), message.to_owned())
    }

    #[test]
    fn amqp_option_validation() {
        let pass = "/etc/emfor/amqp.pass";
        let rows: Vec<(Value, Vec<(String, String)>)> = vec![
            (
                json!({}),
                vec![
                    err("user", "missing required key"),
                    err("password_file", "missing required key"),
                ],
            ),
            (
                json!({"user": "emfor"}),
                vec![err("password_file", "missing required key")],
            ),
            (
                json!({"user": "emfor", "password_file": pass, "vhost": "orders"}),
                vec![],
            ),
            (
                json!({"user": "emfor", "password_file": pass, "vhost": ""}),
                vec![err("vhost", "must be 1 to 255 bytes")],
            ),
            (
                json!({"user": "", "password_file": pass}),
                vec![err("user", "must be 1 to 255 bytes")],
            ),
            (
                json!({"user": "emfor", "password_file": pass, "server_name": "mq.internal"}),
                vec![err("server_name", "requires tls = true")],
            ),
            (
                json!({"user": "emfor", "password_file": pass, "password": "x"}),
                vec![err("password", "unknown key")],
            ),
            (
                json!({"user": 1, "password_file": pass, "tls": "yes"}),
                vec![
                    err("user", "must be a string"),
                    err("tls", "must be a boolean"),
                ],
            ),
        ];
        for (input, want) in rows {
            let mut got = errors(input.clone());
            let mut want = want;
            got.sort();
            want.sort();
            assert_eq!(got, want, "{input}");
        }
        let settings = parse(
            &options(json!({"user": "emfor", "password_file": pass})),
            false,
        )
        .unwrap();
        assert_eq!(settings.vhost, "/");
        assert_eq!(settings.tls, None);
    }

    /// What the fake broker does, in order.
    #[derive(Clone)]
    enum Step {
        /// Write these bytes.
        Send(Vec<u8>),
        /// Read one frame and record it.
        Read,
    }
    use Step::{Read, Send};

    /// A recorded frame: class, method and arguments.
    type Seen = Arc<Mutex<Vec<(u16, u16, Vec<u8>)>>>;

    /// A fake broker: reads the protocol header, then plays `script`,
    /// then closes the connection.
    async fn fake_broker(
        script: Vec<Step>,
        acceptor: Option<tokio_rustls::TlsAcceptor>,
    ) -> (SocketAddr, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let mut stream: Box<dyn Stream> = match acceptor {
                None => Box::new(tcp),
                Some(a) => match a.accept(tcp).await {
                    Ok(tls) => Box::new(tls),
                    Err(_) => return,
                },
            };
            let mut header = [0u8; 8];
            if stream.read_exact(&mut header).await.is_err() || &header != PROTOCOL_HEADER {
                return;
            }
            for step in script {
                match step {
                    Send(bytes) => {
                        if stream.write_all(&bytes).await.is_err() {
                            return;
                        }
                    }
                    Read => {
                        let mut h = [0u8; 7];
                        if stream.read_exact(&mut h).await.is_err() {
                            return;
                        }
                        let size = u32::from_be_bytes([h[3], h[4], h[5], h[6]]) as usize;
                        let mut payload = vec![0u8; size + 1];
                        if stream.read_exact(&mut payload).await.is_err() {
                            return;
                        }
                        let class = u16::from_be_bytes([payload[0], payload[1]]);
                        let method = u16::from_be_bytes([payload[2], payload[3]]);
                        log.lock()
                            .unwrap()
                            .push((class, method, payload[4..size].to_vec()));
                    }
                }
            }
        });
        (addr, seen)
    }

    fn start(mechanisms: &str) -> Vec<u8> {
        let mut args = vec![0, 9];
        let mut props = Vec::new();
        field(&mut props, b"product", b'S', &long_bytes(b"FakeMQ"));
        args.extend_from_slice(&long_bytes(&props));
        args.extend_from_slice(&long_bytes(mechanisms.as_bytes()));
        args.extend_from_slice(&long_bytes(b"en_US"));
        method_frame(START, &args)
    }

    fn tune(channel_max: u16, frame_max: u32, heartbeat: u16) -> Vec<u8> {
        let mut args = Vec::new();
        args.extend_from_slice(&channel_max.to_be_bytes());
        args.extend_from_slice(&frame_max.to_be_bytes());
        args.extend_from_slice(&heartbeat.to_be_bytes());
        method_frame(TUNE, &args)
    }

    fn open_ok() -> Vec<u8> {
        method_frame(OPEN_OK, &[0])
    }

    fn close(code: u16, text: &str) -> Vec<u8> {
        let mut args = code.to_be_bytes().to_vec();
        short_string(&mut args, text.as_bytes());
        args.extend_from_slice(&[0, 10, 0, 11]);
        method_frame(CLOSE, &args)
    }

    fn heartbeat() -> Vec<u8> {
        vec![FRAME_HEARTBEAT, 0, 0, 0, 0, 0, 0, FRAME_END]
    }

    /// The whole successful handshake, up to and including reading Close.
    fn happy() -> Vec<Step> {
        vec![
            Send(start("PLAIN AMQPLAIN")),
            Read,
            Send(tune(2047, 131_072, 60)),
            Read,
            Read,
            Send(open_ok()),
            Read,
        ]
    }

    fn settings(dir: &std::path::Path) -> Settings {
        let password_file = dir.join("amqp.pass");
        std::fs::write(&password_file, "s3cret\n").unwrap();
        Settings {
            user: "emfor".into(),
            password_file,
            vhost: "/".into(),
            tls: None,
        }
    }

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest {
            service: "mq".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: Default::default(),
            timeout: Duration::from_secs(2),
        }
    }

    #[tokio::test]
    async fn amqp_broker_answers() {
        let dir = tempfile::tempdir().unwrap();
        let settings = settings(dir.path());
        let down = |r: &str| CheckOutcome::Down(r.into());
        let mut bad_end = method_frame(START, &[0, 9]);
        *bad_end.last_mut().unwrap() = 0;
        let rows: Vec<(&str, Vec<Step>, CheckOutcome)> = vec![
            ("Start, Tune, Open-Ok", happy(), CheckOutcome::Up),
            (
                "only AMQPLAIN",
                vec![Send(start("AMQPLAIN")), Read],
                down("auth: server does not offer PLAIN"),
            ),
            (
                "Close 403 after Start-Ok",
                vec![
                    Send(start("PLAIN")),
                    Read,
                    Send(close(403, "ACCESS_REFUSED - Login was refused")),
                    Read,
                ],
                down("auth: 403 ACCESS_REFUSED - Login was refused"),
            ),
            (
                "closed after Start-Ok",
                vec![Send(start("PLAIN")), Read],
                down("auth: connection closed after login"),
            ),
            (
                "Close 530 after Open",
                vec![
                    Send(start("PLAIN")),
                    Read,
                    Send(tune(0, 0, 0)),
                    Read,
                    Read,
                    Send(close(530, "NOT_ALLOWED - vhost orders not found")),
                    Read,
                ],
                down("vhost: 530 NOT_ALLOWED - vhost orders not found"),
            ),
            (
                "heartbeat between Tune and Open-Ok",
                vec![
                    Send(start("PLAIN")),
                    Read,
                    Send(tune(2047, 131_072, 60)),
                    Read,
                    Read,
                    Send(heartbeat()),
                    Send(open_ok()),
                    Read,
                ],
                CheckOutcome::Up,
            ),
            (
                "protocol header",
                vec![Send(b"AMQP\x00\x01\x00\x00".to_vec())],
                down("query: server speaks AMQP 1-0-0"),
            ),
            (
                "size 200000",
                vec![Send(vec![1, 0, 0, 0, 3, 13, 64])],
                down("query: frame too large"),
            ),
            (
                "ends in 0x00",
                vec![Send(bad_end)],
                down("query: malformed frame"),
            ),
            (
                "Secure instead of Tune",
                vec![
                    Send(start("PLAIN")),
                    Read,
                    Send(method_frame(20, &long_bytes(b"challenge"))),
                ],
                down("query: unexpected method 10.20"),
            ),
            (
                "closed before Start",
                vec![],
                down("query: connection closed"),
            ),
            (
                "no Close-Ok",
                vec![
                    Send(start("PLAIN")),
                    Read,
                    Send(tune(2047, 131_072, 60)),
                    Read,
                    Read,
                    Send(open_ok()),
                ],
                CheckOutcome::Up,
            ),
        ];
        for (name, script, want) in rows {
            let (addr, _) = fake_broker(script, None).await;
            let got = check(&req(addr), &settings).await;
            assert_eq!(got, want, "{name}");
            if let CheckOutcome::Down(reason) = &got {
                assert!(!reason.contains("s3cret"), "{name}: {reason}");
            }
        }
    }

    /// Wait until the fake broker has recorded `n` frames.
    async fn frames(seen: &Seen, n: usize) -> Vec<(u16, u16, Vec<u8>)> {
        for _ in 0..100 {
            let frames = seen.lock().unwrap().clone();
            if frames.len() >= n {
                return frames;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        seen.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn sent_methods() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = settings(dir.path());
        settings.vhost = "orders".into();
        let (addr, seen) = fake_broker(happy(), None).await;
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
        let frames = frames(&seen, 4).await;
        let methods: Vec<(u16, u16)> = frames.iter().map(|f| (f.0, f.1)).collect();
        assert_eq!(
            methods,
            [(10, START_OK), (10, TUNE_OK), (10, OPEN), (10, CLOSE)]
        );

        let start_ok = &frames[0].2;
        let mut a = Args { rest: start_ok };
        let properties = a.long().unwrap();
        assert_eq!(a.short().unwrap(), b"PLAIN");
        assert_eq!(a.long().unwrap(), b"\0emfor\0s3cret");
        assert_eq!(a.short().unwrap(), b"en_US");
        let contains = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).any(|w| w == needle);
        assert!(contains(properties, b"\x07productS\x00\x00\x00\x05emfor"));
        assert!(contains(
            properties,
            b"\x1cauthentication_failure_closet\x01"
        ));

        assert_eq!(frames[1].2, tune_ok(2047, 131_072));
        assert_eq!(frames[1].2, [7, 255, 0, 2, 0, 0, 0, 0]);
        assert_eq!(frames[2].2, b"\x06orders\x00\x00");
        assert_eq!(&frames[3].2[..2], &REPLY_SUCCESS.to_be_bytes());
    }

    #[tokio::test]
    async fn frame_max_zero_becomes_the_limit_and_close_is_answered() {
        let dir = tempfile::tempdir().unwrap();
        let settings = settings(dir.path());
        let (addr, seen) = fake_broker(
            vec![
                Send(start("PLAIN")),
                Read,
                Send(tune(0, 0, 0)),
                Read,
                Read,
                Send(close(530, "NOT_ALLOWED")),
                Read,
            ],
            None,
        )
        .await;
        assert_eq!(
            check(&req(addr), &settings).await,
            CheckOutcome::Down("vhost: 530 NOT_ALLOWED".into())
        );
        let frames = frames(&seen, 4).await;
        assert_eq!(frames[1].2, tune_ok(0, MAX_FRAME));
        assert_eq!((frames[3].0, frames[3].1), (10, CLOSE_OK));
    }

    #[tokio::test]
    async fn unreadable_password_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = settings(dir.path());
        settings.password_file = dir.path().join("missing");
        let (addr, _) = fake_broker(happy(), None).await;
        match check(&req(addr), &settings).await {
            CheckOutcome::Error(r) => assert!(r.starts_with("password_file: "), "{r}"),
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tls_is_up() {
        use emfor_tls::testing::pki;
        let pki = pki("mq.test");
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, &pki.ca_pem).unwrap();
        let mut settings = settings(dir.path());
        settings.tls = Some(TlsSettings {
            verify: true,
            server_name: Some("mq.test".into()),
            ca_file: Some(ca),
        });
        let acceptor = tokio_rustls::TlsAcceptor::from(pki.server_config);
        let (addr, _) = fake_broker(happy(), Some(acceptor)).await;
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
    }
}
