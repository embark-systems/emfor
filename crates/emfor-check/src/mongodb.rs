//! The `mongodb` plugin: `hello` over OP_MSG, an optional SCRAM-SHA-256
//! login (RFC 7677), and the role from the `hello` reply.

use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::net::{self, Stream, io_failure};
use crate::options::{
    TlsSettings, boolean, check_password_file, file, read_password, string, tls_settings,
};

const KEYS: &[&str] = &[
    "user",
    "password_file",
    "auth_source",
    "role",
    "tls",
    "tls_verify",
    "server_name",
    "ca_file",
];

/// The longest reply read, header included.
const MAX_REPLY: usize = 65536;
/// OP_MSG.
const OP_MSG: i32 = 2013;
/// The OP_MSG flag bit that says a CRC-32C follows the sections.
const CHECKSUM_PRESENT: u32 = 1;
/// The fewest PBKDF2 iterations accepted from a server (RFC 7677).
const MIN_ITERATIONS: u32 = 4096;
/// The most PBKDF2 iterations computed for a server. MongoDB uses 15000 by
/// default; a server asking for billions would hold a thread for minutes.
const MAX_ITERATIONS: u32 = 1_000_000;

/// The role a backend has to have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Writable: a replica set primary, a standalone server or a mongos.
    Primary,
    /// A replica set secondary.
    Secondary,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Role::Primary => "primary",
            Role::Secondary => "secondary",
        }
    }

    fn accepts(self, actual: &str) -> bool {
        match self {
            Role::Primary => matches!(actual, "primary" | "standalone" | "mongos"),
            Role::Secondary => actual == "secondary",
        }
    }
}

/// Validated `check_options` of the `mongodb` plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The user and the password file, set together.
    pub login: Option<(String, PathBuf)>,
    pub auth_source: String,
    pub role: Option<Role>,
    pub tls: Option<TlsSettings>,
}

/// Parse and validate the `mongodb` plugin's options; `read_files` also
/// checks that `password_file` and `ca_file` are usable.
pub fn parse(options: &Options, read_files: bool) -> Result<Settings, Vec<OptionError>> {
    let mut errors = Vec::new();
    for key in options.keys() {
        if !KEYS.contains(&key.as_str()) {
            errors.push(OptionError::new(key, "unknown key"));
        }
    }
    let user = match string(options, "user", &mut errors) {
        Some(s) if (1..=255).contains(&s.len()) => Some(s.to_owned()),
        Some(_) => {
            errors.push(OptionError::new("user", "must be 1 to 255 bytes"));
            None
        }
        None => None,
    };
    let password_file = file(
        options,
        "password_file",
        read_files,
        check_password_file,
        &mut errors,
    );
    let has_user = options.contains_key("user");
    if has_user && !options.contains_key("password_file") {
        errors.push(OptionError::new("user", "requires password_file"));
    }
    if options.contains_key("password_file") && !has_user {
        errors.push(OptionError::new("password_file", "requires user"));
    }
    let auth_source = match string(options, "auth_source", &mut errors) {
        Some(_) if !has_user => {
            errors.push(OptionError::new("auth_source", "requires user"));
            None
        }
        Some(s) if is_database_name(s) => Some(s.to_owned()),
        Some(_) => {
            errors.push(OptionError::new("auth_source", "must be a database name"));
            None
        }
        None => None,
    };
    let role = match string(options, "role", &mut errors) {
        Some("primary") => Some(Role::Primary),
        Some("secondary") => Some(Role::Secondary),
        Some(_) => {
            errors.push(OptionError::new(
                "role",
                "must be one of primary, secondary",
            ));
            None
        }
        None => None,
    };
    let tls_on = boolean(options, "tls", false, &mut errors);
    let tls = tls_settings(
        options,
        tls_on,
        "requires tls = true",
        read_files,
        &mut errors,
    );
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Settings {
        login: user.zip(password_file),
        auth_source: auth_source.unwrap_or_else(|| "admin".to_owned()),
        role,
        tls,
    })
}

fn is_database_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Run one check.
pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let login = match &settings.login {
        None => None,
        Some((user, path)) => match read_password(path) {
            Ok(password) if password.bytes().all(|b| (0x20..=0x7e).contains(&b)) => {
                Some((user.as_str(), password))
            }
            Ok(_) => {
                return CheckOutcome::Error(
                    "password_file: only printable ASCII passwords are supported".into(),
                );
            }
            Err(reason) => return CheckOutcome::Error(reason),
        },
    };
    let stream = match net::open(req.ip, req.port, settings.tls.as_ref(), req.timeout).await {
        Ok(s) => s,
        Err(outcome) => return outcome,
    };
    let mut conn = Connection {
        stream,
        next_id: net::random::<u16>().into(),
    };
    match converse(&mut conn, settings, login).await {
        Ok(()) => CheckOutcome::Up,
        Err(outcome) => outcome,
    }
}

async fn converse(
    conn: &mut Connection,
    settings: &Settings,
    login: Option<(&str, String)>,
) -> Result<(), CheckOutcome> {
    let mut hello = Document::new();
    hello.i32("hello", 1);
    hello.string("$db", "admin");
    let reply = conn.command("query", &hello.finish()).await?;
    if let Some((user, password)) = login {
        authenticate(conn, &settings.auth_source, user, &password).await?;
    }
    if let Some(want) = settings.role {
        let actual = actual_role(&reply);
        if !want.accepts(actual) {
            return Err(CheckOutcome::Down(format!(
                "role is {actual}, want {}",
                want.name()
            )));
        }
    }
    Ok(())
}

/// The role a `hello` reply describes.
fn actual_role(hello: &Doc) -> &'static str {
    let flag = |key: &str| matches!(hello.get(key), Some(Bson::Bool(true)));
    if flag("arbiterOnly") {
        "arbiter"
    } else if matches!(hello.get("msg"), Some(Bson::String(m)) if m == "isdbgrid") {
        "mongos"
    } else if flag("isWritablePrimary") {
        if hello.get("setName").is_some() {
            "primary"
        } else {
            "standalone"
        }
    } else if flag("secondary") {
        "secondary"
    } else {
        "unknown"
    }
}

async fn authenticate(
    conn: &mut Connection,
    db: &str,
    user: &str,
    password: &str,
) -> Result<(), CheckOutcome> {
    let mut nonce = [0u8; 24];
    net::fill(&mut nonce);
    let mut scram = Scram::new(user, password, &BASE64.encode(nonce));

    let mut options = Document::new();
    options.boolean("skipEmptyExchange", true);
    let mut start = Document::new();
    start.i32("saslStart", 1);
    start.string("mechanism", "SCRAM-SHA-256");
    start.binary("payload", scram.client_first().as_bytes());
    start.i32("autoAuthorize", 1);
    start.document("options", &options.finish());
    start.string("$db", db);
    let reply = conn.command("auth", &start.finish()).await?;
    let conversation = reply.get("conversationId").cloned().ok_or_else(bad_scram)?;
    let server_first = payload(&reply)?;
    // PBKDF2 is CPU work that does not yield: run it off the async
    // workers, so the check's timeout still ends the check.
    let (scram_back, client_final) = tokio::task::spawn_blocking(move || {
        let client_final = scram.client_final(&server_first);
        (scram, client_final)
    })
    .await
    .map_err(|e| CheckOutcome::Error(format!("auth: SCRAM computation failed: {e}")))?;
    scram = scram_back;
    let client_final = client_final?;

    let reply = conn
        .command(
            "auth",
            &sasl_continue(&conversation, client_final.as_bytes(), db),
        )
        .await?;
    scram.verify(&payload(&reply)?)?;
    if !matches!(reply.get("done"), Some(Bson::Bool(true))) {
        // A server that ignores skipEmptyExchange wants one more, empty,
        // message.
        let reply = conn
            .command("auth", &sasl_continue(&conversation, b"", db))
            .await?;
        if !matches!(reply.get("done"), Some(Bson::Bool(true))) {
            return Err(bad_scram());
        }
    }
    Ok(())
}

fn sasl_continue(conversation: &Bson, payload: &[u8], db: &str) -> Vec<u8> {
    let mut doc = Document::new();
    doc.i32("saslContinue", 1);
    doc.value("conversationId", conversation);
    doc.binary("payload", payload);
    doc.string("$db", db);
    doc.finish()
}

/// The SASL payload of a reply, as text.
fn payload(reply: &Doc) -> Result<String, CheckOutcome> {
    match reply.get("payload") {
        Some(Bson::Binary(b)) => String::from_utf8(b.clone()).map_err(|_| bad_scram()),
        _ => Err(bad_scram()),
    }
}

fn bad_scram() -> CheckOutcome {
    CheckOutcome::Down("auth: malformed SCRAM message".into())
}

fn malformed() -> CheckOutcome {
    CheckOutcome::Down("query: malformed reply".into())
}

/// The client side of one SCRAM-SHA-256 exchange (RFC 5802, RFC 7677).
struct Scram {
    password: String,
    nonce: String,
    client_first_bare: String,
    /// The server signature the server must send, once known.
    expected: Option<Vec<u8>>,
}

impl Scram {
    fn new(user: &str, password: &str, nonce: &str) -> Scram {
        let user = user.replace('=', "=3D").replace(',', "=2C");
        Scram {
            password: password.to_owned(),
            nonce: nonce.to_owned(),
            client_first_bare: format!("n={user},r={nonce}"),
            expected: None,
        }
    }

    fn client_first(&self) -> String {
        format!("n,,{}", self.client_first_bare)
    }

    /// The client-final-message for a server-first-message.
    fn client_final(&mut self, server_first: &str) -> Result<String, CheckOutcome> {
        let mut nonce = None;
        let mut salt = None;
        let mut iterations = None;
        for attribute in server_first.split(',') {
            match attribute.split_at_checked(2) {
                Some(("r=", v)) => nonce = Some(v),
                Some(("s=", v)) => salt = BASE64.decode(v).ok(),
                Some(("i=", v)) => iterations = v.parse::<u32>().ok(),
                _ => {}
            }
        }
        let (Some(nonce), Some(salt), Some(iterations)) = (nonce, salt, iterations) else {
            return Err(bad_scram());
        };
        if iterations > MAX_ITERATIONS {
            return Err(CheckOutcome::Down(format!(
                "auth: server asks for {iterations} SCRAM iterations, more than {MAX_ITERATIONS}"
            )));
        }
        if iterations < MIN_ITERATIONS
            || !nonce.starts_with(&self.nonce)
            || nonce.len() == self.nonce.len()
        {
            return Err(bad_scram());
        }
        let salted = pbkdf2(self.password.as_bytes(), &salt, iterations);
        let client_key = hmac(&salted, b"Client Key");
        let stored_key = Sha256::digest(&client_key);
        let without_proof = format!("c=biws,r={nonce}");
        let auth_message = format!("{},{server_first},{without_proof}", self.client_first_bare);
        let signature = hmac(&stored_key, auth_message.as_bytes());
        let proof: Vec<u8> = client_key
            .iter()
            .zip(&signature)
            .map(|(k, s)| k ^ s)
            .collect();
        let server_key = hmac(&salted, b"Server Key");
        self.expected = Some(hmac(&server_key, auth_message.as_bytes()));
        Ok(format!("{without_proof},p={}", BASE64.encode(proof)))
    }

    /// Check the server-final-message.
    fn verify(&self, server_final: &str) -> Result<(), CheckOutcome> {
        let signature = server_final
            .split(',')
            .find_map(|a| a.strip_prefix("v="))
            .and_then(|v| BASE64.decode(v).ok())
            .ok_or_else(bad_scram)?;
        if Some(signature) == self.expected {
            Ok(())
        } else {
            Err(CheckOutcome::Down(
                "auth: server signature does not match".into(),
            ))
        }
    }
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .expect("HMAC accepts keys of any length (RFC 2104), so new_from_slice never fails");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// PBKDF2-HMAC-SHA-256 with one block: the 32 bytes SCRAM-SHA-256 needs.
fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32) -> Vec<u8> {
    let mut block = salt.to_vec();
    block.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac(password, &block);
    let mut out = u.clone();
    for _ in 1..iterations {
        u = hmac(password, &u);
        for (o, b) in out.iter_mut().zip(&u) {
            *o ^= b;
        }
    }
    out
}

/// A connection that sends OP_MSG commands.
struct Connection {
    stream: Box<dyn Stream>,
    next_id: i32,
}

impl Connection {
    /// Send a command and read its reply; `ok: 0` is `down` with
    /// `<prefix>: <codeName>: <errmsg>`.
    async fn command(&mut self, prefix: &str, body: &[u8]) -> Result<Doc, CheckOutcome> {
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        let message = op_msg(id, body);
        self.stream
            .write_all(&message)
            .await
            .map_err(|e| io_failure("query: ", &e))?;
        self.stream
            .flush()
            .await
            .map_err(|e| io_failure("query: ", &e))?;
        let reply = self.reply(id).await?;
        let ok = match reply.get("ok") {
            Some(Bson::Double(v)) => *v != 0.0,
            Some(Bson::Int32(v)) => *v != 0,
            Some(Bson::Int64(v)) => *v != 0,
            Some(Bson::Bool(v)) => *v,
            _ => false,
        };
        if ok {
            return Ok(reply);
        }
        let text = |key: &str| match reply.get(key) {
            Some(Bson::String(s)) => s.clone(),
            _ => String::new(),
        };
        let code_name = match text("codeName") {
            name if name.is_empty() => "Error".to_owned(),
            name => name,
        };
        Err(CheckOutcome::Down(format!(
            "{prefix}: {code_name}: {}",
            text("errmsg")
        )))
    }

    /// Read the reply to request `id`.
    async fn reply(&mut self, id: i32) -> Result<Doc, CheckOutcome> {
        let mut len = [0u8; 4];
        self.read_exact(&mut len).await?;
        let len = usize::try_from(i32::from_le_bytes(len)).map_err(|_| malformed())?;
        if len > MAX_REPLY {
            return Err(CheckOutcome::Down("query: reply too large".into()));
        }
        // Header (16), flagBits (4) and a section kind byte.
        if len < 21 {
            return Err(malformed());
        }
        let mut rest = vec![0u8; len - 4];
        self.read_exact(&mut rest).await?;
        let word =
            |at: usize| i32::from_le_bytes([rest[at], rest[at + 1], rest[at + 2], rest[at + 3]]);
        let (response_to, op_code) = (word(4), word(8));
        let flags = u32::from_le_bytes([rest[12], rest[13], rest[14], rest[15]]);
        if op_code != OP_MSG || response_to != id {
            return Err(malformed());
        }
        let mut sections = &rest[16..];
        if flags & CHECKSUM_PRESENT != 0 {
            sections = sections
                .get(..sections.len().saturating_sub(4))
                .ok_or_else(malformed)?;
        }
        match sections.split_first() {
            Some((0, body)) => {
                let (doc, used) = Doc::decode(body).ok_or_else(malformed)?;
                if used != body.len() {
                    return Err(malformed());
                }
                Ok(doc)
            }
            _ => Err(malformed()),
        }
    }

    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), CheckOutcome> {
        match self.stream.read_exact(buf).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                Err(CheckOutcome::Down("query: connection closed".into()))
            }
            Err(e) => Err(io_failure("query: ", &e)),
        }
    }
}

/// An OP_MSG message with flagBits 0 and one kind 0 section.
fn op_msg(id: i32, body: &[u8]) -> Vec<u8> {
    let len = 16 + 4 + 1 + body.len();
    let mut out = Vec::with_capacity(len);
    // Commands are a few hundred bytes, far below i32::MAX.
    out.extend_from_slice(&i32::try_from(len).unwrap_or(i32::MAX).to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&OP_MSG.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(0);
    out.extend_from_slice(body);
    out
}

/// Builds a BSON document with the element types commands need.
struct Document {
    elements: Vec<u8>,
}

impl Document {
    fn new() -> Document {
        Document {
            elements: Vec::new(),
        }
    }

    fn key(&mut self, kind: u8, key: &str) {
        self.elements.push(kind);
        self.elements.extend_from_slice(key.as_bytes());
        self.elements.push(0);
    }

    fn i32(&mut self, key: &str, v: i32) {
        self.key(0x10, key);
        self.elements.extend_from_slice(&v.to_le_bytes());
    }

    fn i64(&mut self, key: &str, v: i64) {
        self.key(0x12, key);
        self.elements.extend_from_slice(&v.to_le_bytes());
    }

    fn boolean(&mut self, key: &str, v: bool) {
        self.key(0x08, key);
        self.elements.push(u8::from(v));
    }

    fn string(&mut self, key: &str, v: &str) {
        self.key(0x02, key);
        self.elements
            .extend_from_slice(&length(v.len() + 1).to_le_bytes());
        self.elements.extend_from_slice(v.as_bytes());
        self.elements.push(0);
    }

    fn binary(&mut self, key: &str, v: &[u8]) {
        self.key(0x05, key);
        self.elements
            .extend_from_slice(&length(v.len()).to_le_bytes());
        self.elements.push(0);
        self.elements.extend_from_slice(v);
    }

    fn document(&mut self, key: &str, encoded: &[u8]) {
        self.key(0x03, key);
        self.elements.extend_from_slice(encoded);
    }

    /// A value read from a reply, sent back as it came (conversationId).
    fn value(&mut self, key: &str, v: &Bson) {
        match v {
            Bson::Int32(n) => self.i32(key, *n),
            Bson::Int64(n) => self.i64(key, *n),
            Bson::String(s) => self.string(key, s),
            Bson::Bool(b) => self.boolean(key, *b),
            Bson::Binary(b) => self.binary(key, b),
            // conversationId is an int32 on every server version.
            _ => self.i32(key, 0),
        }
    }

    fn finish(self) -> Vec<u8> {
        let mut out = length(self.elements.len() + 5).to_le_bytes().to_vec();
        out.extend_from_slice(&self.elements);
        out.push(0);
        out
    }
}

/// A BSON length; documents built here are small.
fn length(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// A decoded BSON value; types the checks never look at are `Other`.
#[derive(Debug, Clone, PartialEq)]
enum Bson {
    Double(f64),
    String(String),
    Document(Doc),
    Binary(Vec<u8>),
    Bool(bool),
    Int32(i32),
    Int64(i64),
    Other,
}

/// A decoded BSON document.
#[derive(Debug, Clone, PartialEq, Default)]
struct Doc {
    fields: Vec<(String, Bson)>,
}

impl Doc {
    fn get(&self, key: &str) -> Option<&Bson> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Decode the document at the start of `bytes`; returns it and its
    /// length.
    fn decode(bytes: &[u8]) -> Option<(Doc, usize)> {
        let len = usize::try_from(i32::from_le_bytes(bytes.get(..4)?.try_into().ok()?)).ok()?;
        if len < 5 || len > bytes.len() || bytes[len - 1] != 0 {
            return None;
        }
        let mut rest = &bytes[4..len - 1];
        let mut doc = Doc::default();
        while let Some((&kind, tail)) = rest.split_first() {
            let end = tail.iter().position(|&b| b == 0)?;
            let key = String::from_utf8(tail[..end].to_vec()).ok()?;
            let (value, used) = decode_value(kind, &tail[end + 1..])?;
            doc.fields.push((key, value));
            rest = &tail[end + 1 + used..];
        }
        Some((doc, len))
    }
}

/// Decode one value of type `kind`; returns it and the bytes it used.
fn decode_value(kind: u8, bytes: &[u8]) -> Option<(Bson, usize)> {
    let fixed = |n: usize| (bytes.len() >= n).then_some((Bson::Other, n));
    let i32_at = |at: usize| -> Option<i32> {
        Some(i32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    };
    match kind {
        0x01 => {
            let v = f64::from_le_bytes(bytes.get(..8)?.try_into().ok()?);
            Some((Bson::Double(v), 8))
        }
        0x02 => {
            let n = usize::try_from(i32_at(0)?).ok()?;
            let s = bytes.get(4..4 + n)?;
            let (nul, text) = s.split_last()?;
            if *nul != 0 {
                return None;
            }
            Some((
                Bson::String(String::from_utf8_lossy(text).into_owned()),
                4 + n,
            ))
        }
        0x03 | 0x04 => {
            let (doc, used) = Doc::decode(bytes)?;
            let value = if kind == 0x03 {
                Bson::Document(doc)
            } else {
                Bson::Other
            };
            Some((value, used))
        }
        0x05 => {
            let n = usize::try_from(i32_at(0)?).ok()?;
            let data = bytes.get(5..5 + n)?;
            Some((Bson::Binary(data.to_vec()), 5 + n))
        }
        0x07 => fixed(12),
        0x08 => Some((Bson::Bool(*bytes.first()? != 0), 1)),
        0x09 | 0x11 => fixed(8),
        0x0A => Some((Bson::Other, 0)),
        0x10 => Some((Bson::Int32(i32_at(0)?), 4)),
        0x12 => {
            let v = i64::from_le_bytes(bytes.get(..8)?.try_into().ok()?);
            Some((Bson::Int64(v), 8))
        }
        0x13 => fixed(16),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::net::SocketAddr;
    use std::time::Duration;
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
    fn mongodb_option_validation() {
        let pass = "/etc/emfor/mongo.pass";
        let rows = [
            (json!({}), vec![]),
            (
                json!({"user": "emfor", "password_file": pass, "auth_source": "app"}),
                vec![],
            ),
            (
                json!({"user": "emfor"}),
                one("user", "requires password_file"),
            ),
            (
                json!({"password_file": pass}),
                one("password_file", "requires user"),
            ),
            (
                json!({"auth_source": "app"}),
                one("auth_source", "requires user"),
            ),
            (
                json!({"user": "emfor", "password_file": pass, "auth_source": "a.b"}),
                one("auth_source", "must be a database name"),
            ),
            (
                json!({"role": "replica"}),
                one("role", "must be one of primary, secondary"),
            ),
            (
                json!({"server_name": "db.internal"}),
                one("server_name", "requires tls = true"),
            ),
            (json!({"database": "app"}), one("database", "unknown key")),
            (
                json!({"user": "", "password_file": pass}),
                one("user", "must be 1 to 255 bytes"),
            ),
            (json!({"tls": "yes"}), one("tls", "must be a boolean")),
        ];
        for (input, want) in rows {
            assert_eq!(errors(input.clone()), want, "{input}");
        }
        let s = parse(&options(json!({"role": "primary"})), false).unwrap();
        assert_eq!(s.login, None);
        assert_eq!(s.auth_source, "admin");
        assert_eq!(s.role, Some(Role::Primary));
    }

    #[test]
    fn bson_round_trip_and_skipping() {
        let mut inner = Document::new();
        inner.boolean("skip", true);
        let mut doc = Document::new();
        doc.i32("n", -7);
        doc.i64("big", 1 << 40);
        doc.string("s", "héllo");
        doc.binary("b", b"\x00\x01");
        doc.boolean("t", true);
        doc.document("d", &inner.finish());
        let bytes = doc.finish();
        let (decoded, used) = Doc::decode(&bytes).unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(decoded.get("n"), Some(&Bson::Int32(-7)));
        assert_eq!(decoded.get("big"), Some(&Bson::Int64(1 << 40)));
        assert_eq!(decoded.get("s"), Some(&Bson::String("héllo".into())));
        assert_eq!(decoded.get("b"), Some(&Bson::Binary(vec![0, 1])));
        assert_eq!(decoded.get("t"), Some(&Bson::Bool(true)));
        let Some(Bson::Document(d)) = decoded.get("d") else {
            panic!("d is a document")
        };
        assert_eq!(d.get("skip"), Some(&Bson::Bool(true)));

        // Types the check skips: double, ObjectId, datetime, null,
        // timestamp, decimal128 and an array.
        let mut e = Vec::new();
        let mut push = |kind: u8, key: &str, value: &[u8]| {
            e.push(kind);
            e.extend_from_slice(key.as_bytes());
            e.push(0);
            e.extend_from_slice(value);
        };
        push(0x01, "ok", &1.0f64.to_le_bytes());
        push(0x07, "oid", &[7; 12]);
        push(0x09, "when", &[0; 8]);
        push(0x0A, "none", &[]);
        push(0x11, "ts", &[0; 8]);
        push(0x13, "dec", &[0; 16]);
        push(0x04, "hosts", &[5, 0, 0, 0, 0]);
        push(0x08, "last", &[1]);
        let mut bytes = length(e.len() + 5).to_le_bytes().to_vec();
        bytes.extend_from_slice(&e);
        bytes.push(0);
        let (decoded, _) = Doc::decode(&bytes).unwrap();
        assert_eq!(decoded.get("ok"), Some(&Bson::Double(1.0)));
        assert_eq!(decoded.get("last"), Some(&Bson::Bool(true)));
        assert_eq!(decoded.fields.len(), 8);

        // A type code the decoder does not know (0x0B, a regex).
        let mut bad = bytes.clone();
        bad[4] = 0x0B;
        assert_eq!(Doc::decode(&bad), None);
    }

    #[test]
    fn scram_example_from_rfc_7677() {
        let mut scram = Scram::new("user", "pencil", "rOprNGfwEbeRWgbNEkqO");
        assert_eq!(scram.client_first(), "n,,n=user,r=rOprNGfwEbeRWgbNEkqO");
        let server_first = "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
        assert_eq!(
            scram.client_final(server_first).unwrap(),
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );
        assert_eq!(
            scram.verify("v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4="),
            Ok(())
        );
        assert_eq!(
            scram.verify("v=AAAATRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4="),
            Err(CheckOutcome::Down(
                "auth: server signature does not match".into()
            ))
        );
    }

    #[test]
    fn scram_escapes_the_user_and_rejects_bad_server_messages() {
        let scram = Scram::new("a=b,c", "pw", "abc");
        assert_eq!(scram.client_first(), "n,,n=a=3Db=2Cc,r=abc");
        for server_first in [
            "r=abcdef,s=c2FsdA==,i=1000",
            "r=xyzdef,s=c2FsdA==,i=4096",
            "r=abc,s=c2FsdA==,i=4096",
            "r=abcdef,i=4096",
            "garbage",
        ] {
            let mut scram = Scram::new("u", "pw", "abc");
            assert_eq!(
                scram.client_final(server_first),
                Err(bad_scram()),
                "{server_first}"
            );
        }
    }

    #[test]
    fn scram_refuses_too_many_iterations() {
        let mut scram = Scram::new("u", "pw", "abc");
        let started = std::time::Instant::now();
        assert_eq!(
            scram.client_final("r=abcdef,s=c2FsdA==,i=4294967295"),
            Err(CheckOutcome::Down(
                "auth: server asks for 4294967295 SCRAM iterations, more than 1000000".into()
            ))
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    /// How the fake server answers one command.
    enum Answer {
        Reply(Vec<u8>),
        Raw(Vec<u8>),
        Close,
    }

    /// Called with the request id, the command name and the command.
    type Handler = Box<dyn FnMut(i32, &str, &Doc) -> Answer + Send>;

    /// A fake server: answers each OP_MSG command with `handler`, called
    /// with the command name and the command document.
    async fn fake_server(
        mut handler: Handler,
    ) -> (SocketAddr, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let Ok((mut tcp, _)) = listener.accept().await else {
                return;
            };
            loop {
                let mut len = [0u8; 4];
                if tcp.read_exact(&mut len).await.is_err() {
                    return;
                }
                let len = i32::from_le_bytes(len) as usize;
                let mut rest = vec![0u8; len - 4];
                if tcp.read_exact(&mut rest).await.is_err() {
                    return;
                }
                let id = i32::from_le_bytes(rest[0..4].try_into().unwrap());
                let (doc, _) = Doc::decode(&rest[17..]).unwrap();
                let name = doc.fields[0].0.clone();
                log.lock().unwrap().push(name.clone());
                let bytes = match handler(id, &name, &doc) {
                    Answer::Close => return,
                    Answer::Raw(bytes) => bytes,
                    Answer::Reply(body) => {
                        let mut m = op_msg(77, &body);
                        m[8..12].copy_from_slice(&id.to_le_bytes());
                        m
                    }
                };
                if tcp.write_all(&bytes).await.is_err() {
                    return;
                }
            }
        });
        (addr, seen)
    }

    /// A `hello` reply with `ok: 1` and these fields.
    fn hello(fields: &[(&str, Value)]) -> Vec<u8> {
        let mut doc = Document::new();
        for (key, value) in fields {
            match value {
                Value::Bool(b) => doc.boolean(key, *b),
                Value::String(s) => doc.string(key, s),
                other => panic!("unsupported {other}"),
            }
        }
        // ok as a double, as servers send it.
        doc.key(0x01, "ok");
        doc.elements.extend_from_slice(&1.0f64.to_le_bytes());
        doc.finish()
    }

    fn failure(code_name: &str, errmsg: &str) -> Vec<u8> {
        let mut doc = Document::new();
        doc.i32("ok", 0);
        doc.string("errmsg", errmsg);
        doc.i32("code", 1);
        doc.string("codeName", code_name);
        doc.finish()
    }

    const SALT: &[u8] = b"emfor-salt-1234";

    /// SCRAM on the server side for `password`, with `iterations` and a
    /// server-final-message from `server_final` (given the right one).
    fn scram_server(
        password: &'static str,
        iterations: u32,
        server_final: fn(String) -> String,
    ) -> impl FnMut(&str, &Doc) -> Option<Vec<u8>> + Send {
        let mut auth: Option<(String, String)> = None;
        move |name, doc| {
            let payload = match doc.get("payload") {
                Some(Bson::Binary(b)) => String::from_utf8(b.clone()).unwrap(),
                _ => String::new(),
            };
            let mut reply = Document::new();
            reply.i32("conversationId", 1);
            match name {
                "saslStart" => {
                    let bare = payload.strip_prefix("n,,").unwrap().to_owned();
                    let nonce = bare.split(",r=").nth(1).unwrap();
                    let first = format!("r={nonce}srv,s={},i={iterations}", BASE64.encode(SALT));
                    reply.binary("payload", first.as_bytes());
                    reply.boolean("done", false);
                    auth = Some((bare, first));
                }
                "saslContinue" => {
                    let (bare, first) = auth.clone().unwrap();
                    let without_proof = payload.split(",p=").next().unwrap();
                    let message = format!("{bare},{first},{without_proof}");
                    let salted = pbkdf2(password.as_bytes(), SALT, iterations);
                    let server_key = hmac(&salted, b"Server Key");
                    let v = format!("v={}", BASE64.encode(hmac(&server_key, message.as_bytes())));
                    reply.binary("payload", server_final(v).as_bytes());
                    reply.boolean("done", true);
                }
                _ => return None,
            }
            reply.i32("ok", 1);
            Some(reply.finish())
        }
    }

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest {
            service: "db".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: Default::default(),
            timeout: Duration::from_secs(2),
        }
    }

    fn settings(role: Option<Role>) -> Settings {
        Settings {
            login: None,
            auth_source: "admin".into(),
            role,
            tls: None,
        }
    }

    fn with_login(dir: &std::path::Path, password: &str) -> Settings {
        let path = dir.join("mongo.pass");
        std::fs::write(&path, format!("{password}\n")).unwrap();
        Settings {
            login: Some(("emfor".into(), path)),
            ..settings(None)
        }
    }

    fn hello_handler(reply: Vec<u8>) -> Handler {
        Box::new(move |_, _, _| Answer::Reply(reply.clone()))
    }

    #[tokio::test]
    async fn mongodb_replies() {
        let down = |r: &str| CheckOutcome::Down(r.into());
        let t = || Value::Bool(true);
        let f = || Value::Bool(false);
        let rs0 = || Value::String("rs0".into());
        let rows: Vec<(Option<Role>, Vec<u8>, CheckOutcome)> = vec![
            (None, hello(&[("isWritablePrimary", t())]), CheckOutcome::Up),
            (
                Some(Role::Primary),
                hello(&[("isWritablePrimary", t()), ("setName", rs0())]),
                CheckOutcome::Up,
            ),
            (
                Some(Role::Primary),
                hello(&[("isWritablePrimary", t())]),
                CheckOutcome::Up,
            ),
            (
                Some(Role::Primary),
                hello(&[("isWritablePrimary", t()), ("msg", "isdbgrid".into())]),
                CheckOutcome::Up,
            ),
            (
                Some(Role::Primary),
                hello(&[("secondary", t()), ("setName", rs0())]),
                down("role is secondary, want primary"),
            ),
            (
                Some(Role::Secondary),
                hello(&[("secondary", t()), ("setName", rs0())]),
                CheckOutcome::Up,
            ),
            (
                Some(Role::Secondary),
                hello(&[("isWritablePrimary", t())]),
                down("role is standalone, want secondary"),
            ),
            (
                Some(Role::Primary),
                hello(&[("arbiterOnly", t()), ("secondary", f())]),
                down("role is arbiter, want primary"),
            ),
            (
                Some(Role::Secondary),
                hello(&[
                    ("isWritablePrimary", f()),
                    ("secondary", f()),
                    ("setName", rs0()),
                ]),
                down("role is unknown, want secondary"),
            ),
            (
                None,
                failure("CommandNotFound", "no such command: 'hello'"),
                down("query: CommandNotFound: no such command: 'hello'"),
            ),
        ];
        for (role, reply, want) in rows {
            let (addr, _) = fake_server(hello_handler(reply.clone())).await;
            assert_eq!(check(&req(addr), &settings(role)).await, want, "{role:?}");
        }
    }

    #[tokio::test]
    async fn mongodb_reply_framing() {
        let down = |r: &str| CheckOutcome::Down(r.into());
        let mut big = 70_000i32.to_le_bytes().to_vec();
        big.extend_from_slice(&[0; 64]);
        let ok = hello(&[("isWritablePrimary", Value::Bool(true))]);
        let mut checksummed = op_msg(0, &ok);
        checksummed[16..20].copy_from_slice(&CHECKSUM_PRESENT.to_le_bytes());
        checksummed.extend_from_slice(&[1, 2, 3, 4]);
        let len = length(checksummed.len());
        checksummed[..4].copy_from_slice(&len.to_le_bytes());
        let mut wrong_op = op_msg(0, &ok);
        wrong_op[12..16].copy_from_slice(&1i32.to_le_bytes());
        type Raw = Box<dyn Fn(i32) -> Answer + Send>;
        let cases: Vec<(&str, Raw, CheckOutcome)> = vec![
            (
                "70000 bytes",
                Box::new(move |_| Answer::Raw(big.clone())),
                down("query: reply too large"),
            ),
            (
                "closed",
                Box::new(|_| Answer::Close),
                down("query: connection closed"),
            ),
            (
                "checksum",
                Box::new(move |id| {
                    let mut m = checksummed.clone();
                    m[8..12].copy_from_slice(&id.to_le_bytes());
                    Answer::Raw(m)
                }),
                CheckOutcome::Up,
            ),
            (
                "not OP_MSG",
                Box::new(move |id| {
                    let mut m = wrong_op.clone();
                    m[8..12].copy_from_slice(&id.to_le_bytes());
                    Answer::Raw(m)
                }),
                down("query: malformed reply"),
            ),
            (
                "answers another request",
                Box::new(move |id| {
                    let mut m = op_msg(0, &ok);
                    m[8..12].copy_from_slice(&(id + 1).to_le_bytes());
                    Answer::Raw(m)
                }),
                down("query: malformed reply"),
            ),
        ];
        for (name, answer, want) in cases {
            let handler: Handler = Box::new(move |id, _, _| answer(id));
            let (addr, _) = fake_server(handler).await;
            assert_eq!(check(&req(addr), &settings(None)).await, want, "{name}");
        }
    }

    #[tokio::test]
    async fn mongodb_logins() {
        let dir = tempfile::tempdir().unwrap();
        let down = |r: &str| CheckOutcome::Down(r.into());
        let ok_hello = hello(&[("isWritablePrimary", Value::Bool(true))]);
        let login = |password: &'static str, iterations: u32, last: fn(String) -> String| {
            let mut scram = scram_server(password, iterations, last);
            let ok_hello = ok_hello.clone();
            let handler: Handler = Box::new(move |_, name, doc| match name {
                "hello" => Answer::Reply(ok_hello.clone()),
                _ => match scram(name, doc) {
                    Some(reply) => Answer::Reply(reply),
                    None => Answer::Close,
                },
            });
            handler
        };
        let same = |v: String| v;
        let rows: Vec<(&str, Handler, CheckOutcome)> = vec![
            (
                "right password",
                login("s3cret", 4096, same),
                CheckOutcome::Up,
            ),
            (
                "rejected",
                Box::new({
                    let ok_hello = ok_hello.clone();
                    move |_, name, _| match name {
                        "hello" => Answer::Reply(ok_hello.clone()),
                        _ => {
                            Answer::Reply(failure("AuthenticationFailed", "Authentication failed."))
                        }
                    }
                }),
                down("auth: AuthenticationFailed: Authentication failed."),
            ),
            (
                "i=1000",
                login("s3cret", 1000, same),
                down("auth: malformed SCRAM message"),
            ),
            (
                "wrong v=",
                login("s3cret", 4096, |_| "v=AAAA".to_owned()),
                down("auth: server signature does not match"),
            ),
            (
                "server knows another password",
                login("other", 4096, same),
                down("auth: server signature does not match"),
            ),
        ];
        for (name, handler, want) in rows {
            let (addr, seen) = fake_server(handler).await;
            let got = check(&req(addr), &with_login(dir.path(), "s3cret")).await;
            assert_eq!(got, want, "{name}");
            if let CheckOutcome::Down(r) = &got {
                assert!(!r.contains("s3cret"), "{name}: {r}");
            }
            assert_eq!(seen.lock().unwrap()[0], "hello", "{name}");
        }

        // A non-ASCII password is refused before anything is sent.
        let (addr, seen) = fake_server(hello_handler(ok_hello.clone())).await;
        assert_eq!(
            check(&req(addr), &with_login(dir.path(), "pässword")).await,
            CheckOutcome::Error(
                "password_file: only printable ASCII passwords are supported".into()
            )
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn auth_source_is_the_database_of_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = with_login(dir.path(), "s3cret");
        settings.auth_source = "app".into();
        let dbs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = dbs.clone();
        let mut scram = scram_server("s3cret", 4096, |v| v);
        let ok_hello = hello(&[("isWritablePrimary", Value::Bool(true))]);
        let handler: Handler = Box::new(move |_, name, doc| {
            if let Some(Bson::String(db)) = doc.get("$db") {
                record.lock().unwrap().push(db.clone());
            }
            match name {
                "hello" => Answer::Reply(ok_hello.clone()),
                _ => Answer::Reply(scram(name, doc).unwrap()),
            }
        });
        let (addr, seen) = fake_server(handler).await;
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
        assert_eq!(
            *seen.lock().unwrap(),
            ["hello", "saslStart", "saslContinue"]
        );
        assert_eq!(*dbs.lock().unwrap(), ["admin", "app", "app"]);
    }

    #[tokio::test]
    async fn tls_is_up() {
        use emfor_tls::testing::pki;
        let pki = pki("db.test");
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, &pki.ca_pem).unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(pki.server_config);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut tls = acceptor.accept(tcp).await.unwrap();
            let mut len = [0u8; 4];
            tls.read_exact(&mut len).await.unwrap();
            let mut rest = vec![0u8; i32::from_le_bytes(len) as usize - 4];
            tls.read_exact(&mut rest).await.unwrap();
            let id = i32::from_le_bytes(rest[0..4].try_into().unwrap());
            let mut m = op_msg(9, &hello(&[("isWritablePrimary", Value::Bool(true))]));
            m[8..12].copy_from_slice(&id.to_le_bytes());
            tls.write_all(&m).await.unwrap();
        });
        let mut settings = settings(Some(Role::Primary));
        settings.tls = Some(TlsSettings {
            verify: true,
            server_name: Some("db.test".into()),
            ca_file: Some(ca),
        });
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
    }
}
