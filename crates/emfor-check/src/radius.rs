//! The `radius` plugin: one RADIUS request per check over UDP, Status-Server
//! (RFC 5997) by default or an Access-Request with PAP (RFC 2865), always
//! with Message-Authenticator (RFC 3579) first. Every response must carry a
//! Response Authenticator made with the shared secret.

use std::path::{Path, PathBuf};

use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options};
use hmac::{Hmac, KeyInit, Mac};
use md5::{Digest, Md5};

use crate::net;
use crate::options::{
    boolean, check_password_file, file, read_option_file, read_password, string, trim_newline,
};

const KEYS: &[&str] = &[
    "secret_file",
    "request",
    "application",
    "user",
    "password_file",
    "reject_is_up",
    "nas_identifier",
    "require_message_authenticator",
];

// Codes (RFC 2865, 2866, 5997).
const ACCESS_REQUEST: u8 = 1;
const ACCESS_ACCEPT: u8 = 2;
const ACCESS_REJECT: u8 = 3;
const ACCOUNTING_RESPONSE: u8 = 5;
const ACCESS_CHALLENGE: u8 = 11;
const STATUS_SERVER: u8 = 12;

// Attribute types (RFC 2865, 3579).
const USER_NAME: u8 = 1;
const USER_PASSWORD: u8 = 2;
const REPLY_MESSAGE: u8 = 18;
const NAS_IDENTIFIER: u8 = 32;
const MESSAGE_AUTHENTICATOR: u8 = 80;

/// Header: Code, Identifier, Length, Authenticator.
const HEADER: usize = 20;
/// The longest attribute value.
const MAX_VALUE: usize = 253;
/// The longest password User-Password can hide (RFC 2865 5.2).
const MAX_PASSWORD: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    StatusServer,
    AccessRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Application {
    Auth,
    Acct,
}

/// Validated `check_options` of the radius plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub secret_file: PathBuf,
    pub request: Request,
    pub application: Application,
    /// Set with `Request::AccessRequest`.
    pub user: Option<String>,
    /// Set with `Request::AccessRequest`.
    pub password_file: Option<PathBuf>,
    pub reject_is_up: bool,
    pub nas_identifier: String,
    pub require_message_authenticator: bool,
}

/// Parse and validate the radius plugin's `check_options`. With
/// `read_files` the secret and password files are read to check their
/// content; without, only that their paths are absolute (a check reads
/// them itself).
pub fn parse(options: &Options, read_files: bool) -> Result<Settings, Vec<OptionError>> {
    let mut errors = Vec::new();
    for key in options.keys() {
        if !KEYS.contains(&key.as_str()) {
            errors.push(OptionError::new(key, "unknown key"));
        }
    }

    let secret_file = file(
        options,
        "secret_file",
        read_files,
        check_secret_file,
        &mut errors,
    );
    if !options.contains_key("secret_file") {
        errors.push(OptionError::new("secret_file", "missing required key"));
    }

    let request = match string(options, "request", &mut errors) {
        None if options.contains_key("request") => None,
        None | Some("status-server") => Some(Request::StatusServer),
        Some("access-request") => Some(Request::AccessRequest),
        Some(_) => {
            errors.push(OptionError::new(
                "request",
                "must be status-server or access-request",
            ));
            None
        }
    };

    let application = match string(options, "application", &mut errors) {
        None | Some("auth") => Application::Auth,
        Some("acct") => Application::Acct,
        Some(_) => {
            errors.push(OptionError::new("application", "must be auth or acct"));
            Application::Auth
        }
    };

    let user = match string(options, "user", &mut errors) {
        None => None,
        Some(s) if (1..=MAX_VALUE).contains(&s.len()) => Some(s.to_owned()),
        Some(_) => {
            errors.push(OptionError::new("user", "must be 1 to 253 bytes"));
            None
        }
    };
    let password_file = file(
        options,
        "password_file",
        read_files,
        check_radius_password_file,
        &mut errors,
    );
    let reject_is_up = boolean(options, "reject_is_up", false, &mut errors);
    let require_message_authenticator =
        boolean(options, "require_message_authenticator", false, &mut errors);

    let nas_identifier = match string(options, "nas_identifier", &mut errors) {
        None => "emfor".to_owned(),
        Some(s) if (1..=MAX_VALUE).contains(&s.len()) => s.to_owned(),
        Some(_) => {
            errors.push(OptionError::new("nas_identifier", "must be 1 to 253 bytes"));
            String::new()
        }
    };

    // Rules that depend on `request`; skipped when `request` is invalid.
    match request {
        Some(Request::AccessRequest) => {
            for key in ["user", "password_file"] {
                if !options.contains_key(key) {
                    errors.push(OptionError::new(key, "missing required key"));
                }
            }
            if application == Application::Acct {
                errors.push(OptionError::new(
                    "application",
                    "requires request = \"status-server\"",
                ));
            }
        }
        Some(Request::StatusServer) => {
            for key in ["user", "password_file", "reject_is_up"] {
                if options.contains_key(key) {
                    errors.push(OptionError::new(
                        key,
                        "requires request = \"access-request\"",
                    ));
                }
            }
        }
        None => {}
    }

    match (secret_file, request) {
        (Some(secret_file), Some(request)) if errors.is_empty() => Ok(Settings {
            secret_file,
            request,
            application,
            user,
            password_file,
            reject_is_up,
            nas_identifier,
            require_message_authenticator,
        }),
        _ => Err(errors),
    }
}

fn check_secret_file(path: &Path) -> Result<(), String> {
    let bytes = read_option_file(path)?;
    if trim_newline(&bytes).is_empty() {
        return Err("file is empty".into());
    }
    Ok(())
}

/// A password file usable for PAP: the common `password_file` rules, and
/// at most 128 bytes, the longest User-Password hides (RFC 2865 5.2).
fn check_radius_password_file(path: &Path) -> Result<(), String> {
    check_password_file(path)?;
    let bytes = read_option_file(path)?;
    if trim_newline(&bytes).len() > MAX_PASSWORD {
        return Err("password longer than 128 bytes".into());
    }
    Ok(())
}

/// Read the shared secret for one check. The error is the check's `error`
/// reason; it names the I/O error kind and never the file's content.
fn read_secret(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("secret_file: {}", e.kind()))?;
    let secret = trim_newline(&bytes);
    if secret.is_empty() {
        return Err("secret_file: file is empty".into());
    }
    Ok(secret.to_vec())
}

/// Run one check.
pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let secret = match read_secret(&settings.secret_file) {
        Ok(s) => s,
        Err(reason) => return CheckOutcome::Error(reason),
    };
    let login = match (&settings.user, &settings.password_file) {
        (Some(user), Some(path)) if settings.request == Request::AccessRequest => {
            match read_password(path) {
                Ok(password) => Some((user.as_str(), password)),
                Err(reason) => return CheckOutcome::Error(reason),
            }
        }
        _ => None,
    };
    let id: u8 = rand::random();
    let mut authenticator = [0u8; 16];
    rand::fill(&mut authenticator);
    let code = match settings.request {
        Request::StatusServer => STATUS_SERVER,
        Request::AccessRequest => ACCESS_REQUEST,
    };
    let login = login
        .as_ref()
        .map(|(user, password)| (user.as_bytes(), password.as_bytes()));
    let packet = match build_request(
        code,
        id,
        &authenticator,
        &secret,
        login,
        settings.nas_identifier.as_bytes(),
    ) {
        Ok(p) => p,
        Err(reason) => return CheckOutcome::Error(reason),
    };
    let response = net::udp_exchange(req.ip, req.port, &packet, req.timeout, |d| {
        (d.len() >= HEADER && d[1] == id).then(|| d.to_vec())
    })
    .await;
    match response {
        Ok(response) => judge(&response, &authenticator, &secret, settings),
        Err(outcome) => outcome,
    }
}

/// A request: Message-Authenticator first, then User-Name and the hidden
/// User-Password when `login` is set, then NAS-Identifier. The error is a
/// check `error` reason for a user or password too long to send.
fn build_request(
    code: u8,
    id: u8,
    authenticator: &[u8; 16],
    secret: &[u8],
    login: Option<(&[u8], &[u8])>,
    nas_identifier: &[u8],
) -> Result<Vec<u8>, String> {
    let mut packet = vec![code, id, 0, 0];
    packet.extend_from_slice(authenticator);
    push_attribute(&mut packet, MESSAGE_AUTHENTICATOR, &[0u8; 16]);
    if let Some((user, password)) = login {
        if user.len() > MAX_VALUE {
            return Err("check_options: user: longer than 253 bytes".into());
        }
        if password.len() > MAX_PASSWORD {
            return Err("password_file: password longer than 128 bytes".into());
        }
        push_attribute(&mut packet, USER_NAME, user);
        push_attribute(
            &mut packet,
            USER_PASSWORD,
            &hide_password(password, secret, authenticator),
        );
    }
    push_attribute(&mut packet, NAS_IDENTIFIER, nas_identifier);
    let len = u16::try_from(packet.len())
        .map_err(|_| "internal: RADIUS request longer than 65535 bytes".to_owned())?;
    packet[2..4].copy_from_slice(&len.to_be_bytes());
    let mac = hmac_md5(secret, &packet);
    // The Message-Authenticator value follows the header and its own
    // type and length bytes.
    packet[HEADER + 2..HEADER + 18].copy_from_slice(&mac);
    Ok(packet)
}

/// Append one attribute; `value` is at most 253 bytes.
fn push_attribute(packet: &mut Vec<u8>, kind: u8, value: &[u8]) {
    debug_assert!(
        value.len() <= MAX_VALUE,
        "attribute values are at most 253 bytes"
    );
    // Callers keep values to 253 bytes, so the length fits in a byte.
    let len = u8::try_from(value.len() + 2).unwrap_or(u8::MAX);
    packet.push(kind);
    packet.push(len);
    packet.extend_from_slice(value);
}

/// User-Password as RFC 2865 section 5.2 hides it: the password padded
/// with zeros to a multiple of 16 bytes, each block XORed with
/// MD5(secret + the previous ciphertext block, or the Request
/// Authenticator for the first).
fn hide_password(password: &[u8], secret: &[u8], authenticator: &[u8; 16]) -> Vec<u8> {
    let blocks = password.len().div_ceil(16).max(1);
    let mut out = password.to_vec();
    out.resize(blocks * 16, 0);
    let mut previous: [u8; 16] = *authenticator;
    for block in out.chunks_mut(16) {
        let mut md5 = Md5::new();
        md5.update(secret);
        md5.update(previous);
        let key: [u8; 16] = md5.finalize().into();
        for (b, k) in block.iter_mut().zip(key) {
            *b ^= k;
        }
        previous.copy_from_slice(block);
    }
    out
}

fn hmac_md5(key: &[u8], data: &[u8]) -> [u8; 16] {
    let mut mac = Hmac::<Md5>::new_from_slice(key)
        .expect("HMAC accepts keys of any length (RFC 2104), so new_from_slice never fails");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// The attributes of a packet whose Length is already checked, as
/// (offset of the type byte, type, value); `None` when one is malformed.
fn attributes(packet: &[u8]) -> Option<Vec<(usize, u8, &[u8])>> {
    let mut out = Vec::new();
    let mut i = HEADER;
    while i < packet.len() {
        let kind = *packet.get(i)?;
        let len = usize::from(*packet.get(i + 1)?);
        if len < 2 || i + len > packet.len() {
            return None;
        }
        out.push((i, kind, &packet[i + 2..i + len]));
        i += len;
    }
    Some(out)
}

/// The result for the response to a request with `authenticator`.
fn judge(
    response: &[u8],
    authenticator: &[u8; 16],
    secret: &[u8],
    settings: &Settings,
) -> CheckOutcome {
    let length = usize::from(u16::from_be_bytes([response[2], response[3]]));
    let attrs = match attributes(response) {
        Some(a) if length == response.len() => a,
        _ => return CheckOutcome::Down("query: malformed response".into()),
    };

    // Response Authenticator: MD5(Code + Identifier + Length + Request
    // Authenticator + attributes + secret).
    let mut md5 = Md5::new();
    md5.update(&response[..4]);
    md5.update(authenticator);
    md5.update(&response[HEADER..]);
    md5.update(secret);
    let expected: [u8; 16] = md5.finalize().into();
    if response[4..HEADER] != expected {
        return CheckOutcome::Down(
            "auth: response authenticator does not match (wrong secret?)".into(),
        );
    }

    match attrs
        .iter()
        .find(|(_, kind, _)| *kind == MESSAGE_AUTHENTICATOR)
    {
        Some(&(offset, _, value)) => {
            let mut copy = response.to_vec();
            copy[4..HEADER].copy_from_slice(authenticator);
            let end = offset + 2 + value.len();
            copy[offset + 2..end].fill(0);
            if value.len() != 16 || hmac_md5(secret, &copy) != value {
                return CheckOutcome::Down("auth: Message-Authenticator does not match".into());
            }
        }
        None if settings.require_message_authenticator => {
            return CheckOutcome::Down("auth: response has no Message-Authenticator".into());
        }
        None => {}
    }

    let auth_down = |what: &str| {
        let messages: Vec<String> = attrs
            .iter()
            .filter(|(_, kind, _)| *kind == REPLY_MESSAGE)
            .map(|(_, _, value)| String::from_utf8_lossy(value).into_owned())
            .collect();
        if messages.is_empty() {
            CheckOutcome::Down(format!("auth: {what}"))
        } else {
            CheckOutcome::Down(format!("auth: {what}: {}", messages.join(" ")))
        }
    };
    match (settings.application, response[0]) {
        (Application::Auth, ACCESS_ACCEPT) => CheckOutcome::Up,
        (Application::Auth, ACCESS_REJECT) if settings.reject_is_up => CheckOutcome::Up,
        (Application::Auth, ACCESS_REJECT) => auth_down("Access-Reject"),
        (Application::Auth, ACCESS_CHALLENGE) => auth_down("Access-Challenge"),
        (Application::Acct, ACCOUNTING_RESPONSE) => CheckOutcome::Up,
        (_, code) => CheckOutcome::Down(format!("query: unexpected response code {code}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::net::UdpSocket;

    const SECRET: &[u8] = b"testing123";
    const PASSWORD: &str = "probe-pw";

    struct Files {
        dir: tempfile::TempDir,
    }

    impl Files {
        fn new() -> Files {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("radius.secret"), b"testing123\n").unwrap();
            std::fs::write(dir.path().join("probe.pass"), format!("{PASSWORD}\n")).unwrap();
            std::fs::write(dir.path().join("empty"), b"\n").unwrap();
            Files { dir }
        }
        fn path(&self, name: &str) -> String {
            self.dir.path().join(name).to_str().unwrap().to_owned()
        }
        /// Options with `secret_file` and the keys of `extra`; a string
        /// value starting with `@` names a file in the directory.
        fn options(&self, extra: Value) -> Options {
            let mut o = Options::new();
            o.insert("secret_file".into(), json!(self.path("radius.secret")));
            for (k, v) in extra.as_object().unwrap() {
                let v = match v.as_str().and_then(|s| s.strip_prefix('@')) {
                    Some(name) => json!(self.path(name)),
                    None => v.clone(),
                };
                o.insert(k.clone(), v);
            }
            o
        }
    }

    fn errors(options: &Options) -> Vec<(String, String)> {
        match parse(options, true) {
            Ok(_) => Vec::new(),
            Err(errors) => errors.into_iter().map(|e| (e.key, e.message)).collect(),
        }
    }

    fn one(key: &str, message: &str) -> Vec<(String, String)> {
        vec![(key.to_owned(), message.to_owned())]
    }

    // ---- options ----

    #[test]
    fn status_server_with_a_secret() {
        let f = Files::new();
        let s = parse(&f.options(json!({})), true).unwrap();
        assert_eq!(s.request, Request::StatusServer);
        assert_eq!(s.application, Application::Auth);
        assert_eq!(s.nas_identifier, "emfor");
        assert!(!s.reject_is_up && !s.require_message_authenticator);
        assert_eq!(s.user, None);
    }

    #[test]
    fn radius_option_validation() {
        let f = Files::new();
        assert_eq!(
            errors(&Options::new()),
            one("secret_file", "missing required key")
        );
        assert_eq!(
            errors(&f.options(json!({ "secret_file": "radius.secret" }))),
            one("secret_file", "not an absolute path")
        );
        assert_eq!(
            errors(&f.options(json!({ "secret_file": "@empty" }))),
            one("secret_file", "file is empty")
        );
        assert_eq!(
            errors(&f.options(json!({ "secret": "testing123" }))),
            one("secret", "unknown key")
        );
        assert_eq!(
            errors(&f.options(json!({ "request": "access-request" }))),
            vec![
                ("user".to_owned(), "missing required key".to_owned()),
                (
                    "password_file".to_owned(),
                    "missing required key".to_owned()
                ),
            ]
        );
        assert_eq!(
            errors(&f.options(json!({ "user": "probe" }))),
            one("user", "requires request = \"access-request\"")
        );
        assert_eq!(
            errors(&f.options(json!({
                "request": "access-request", "application": "acct",
                "user": "probe", "password_file": "@probe.pass",
            }))),
            one("application", "requires request = \"status-server\"")
        );
        assert_eq!(
            errors(&f.options(json!({ "reject_is_up": true }))),
            one("reject_is_up", "requires request = \"access-request\"")
        );
        assert_eq!(
            errors(&f.options(json!({ "request": "coa" }))),
            one("request", "must be status-server or access-request")
        );
        assert_eq!(
            errors(&f.options(json!({ "nas_identifier": "" }))),
            one("nas_identifier", "must be 1 to 253 bytes")
        );
    }

    #[test]
    fn other_option_errors() {
        let f = Files::new();
        assert_eq!(
            errors(&f.options(json!({ "password": "x" }))),
            one("password", "unknown key")
        );
        assert_eq!(
            errors(&f.options(json!({ "secret_file": "@missing" }))),
            one("secret_file", "file not found")
        );
        assert_eq!(
            errors(&f.options(json!({ "application": "coa" }))),
            one("application", "must be auth or acct")
        );
        assert_eq!(
            errors(&f.options(json!({ "require_message_authenticator": "yes" }))),
            one("require_message_authenticator", "must be a boolean")
        );
        assert_eq!(
            errors(&f.options(json!({ "nas_identifier": "x".repeat(254) }))),
            one("nas_identifier", "must be 1 to 253 bytes")
        );
        assert_eq!(
            errors(&f.options(json!({ "request": 1 }))),
            one("request", "must be a string")
        );
        let s = parse(
            &f.options(json!({
                "request": "access-request", "user": "probe",
                "password_file": "@probe.pass", "reject_is_up": true,
                "nas_identifier": "edge-1", "require_message_authenticator": true,
            })),
            true,
        )
        .unwrap();
        assert_eq!(s.request, Request::AccessRequest);
        assert_eq!(s.user.as_deref(), Some("probe"));
        assert!(s.reject_is_up && s.require_message_authenticator);
        assert_eq!(s.nas_identifier, "edge-1");
    }

    #[test]
    fn login_lengths_are_checked_when_validating() {
        let f = Files::new();
        std::fs::write(f.path("long.pass"), "p".repeat(129)).unwrap();
        std::fs::write(f.path("max.pass"), "p".repeat(128)).unwrap();
        assert_eq!(
            errors(&f.options(with(access(), json!({ "user": "u".repeat(254) })))),
            one("user", "must be 1 to 253 bytes")
        );
        assert_eq!(
            errors(&f.options(with(access(), json!({ "user": "" })))),
            one("user", "must be 1 to 253 bytes")
        );
        assert_eq!(
            errors(&f.options(with(access(), json!({ "password_file": "@long.pass" })))),
            one("password_file", "password longer than 128 bytes")
        );
        assert_eq!(
            errors(&f.options(with(
                access(),
                json!({ "user": "u".repeat(253), "password_file": "@max.pass" })
            ))),
            Vec::new()
        );
    }

    #[test]
    fn options_for_a_check_do_not_read_files() {
        let f = Files::new();
        let o = f.options(json!({ "secret_file": "@missing" }));
        assert!(parse(&o, false).is_ok());
        assert!(parse(&o, true).is_err());
    }

    // ---- packets ----

    /// RFC 2865 section 7.1: secret, Request Authenticator, password, the
    /// hidden password and the Access-Accept answering it.
    const RFC_SECRET: &[u8] = b"xyzzy5461";
    const RFC_AUTHENTICATOR: &str = "0f403f9473978057bd83d5cb98f4227a";
    const RFC_HIDDEN: &str = "0dbe708d93d413ce3196e43f782a0aee";
    const RFC_ACCEPT: &str = "0200002686fe220e7624ba2a1005f6bf9b55e0b2\
                              0606000000010f06000000000e06c0a80103";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn rfc_authenticator() -> [u8; 16] {
        hex(RFC_AUTHENTICATOR).try_into().unwrap()
    }

    fn settings(extra: Value) -> Settings {
        let f = Files::new();
        parse(&f.options(extra), true).unwrap()
    }

    #[test]
    fn password_hidden_as_in_rfc_2865() {
        let hidden = hide_password(b"arctangent", RFC_SECRET, &rfc_authenticator());
        assert_eq!(hidden, hex(RFC_HIDDEN));
    }

    #[test]
    fn long_passwords_chain_the_blocks() {
        // 17 bytes: two blocks, the second keyed with the first ciphertext.
        let a = rfc_authenticator();
        let hidden = hide_password(b"0123456789abcdefg", RFC_SECRET, &a);
        assert_eq!(hidden.len(), 32);
        let mut md5 = Md5::new();
        md5.update(RFC_SECRET);
        md5.update(&hidden[..16]);
        let key: [u8; 16] = md5.finalize().into();
        assert_eq!(hidden[16] ^ key[0], b'g');
        assert!(hidden[17..].iter().zip(&key[1..]).all(|(h, k)| h == k));
    }

    #[test]
    fn response_authenticator_as_in_rfc_2865() {
        let s = settings(
            json!({ "request": "access-request", "user": "nemo", "password_file": "@probe.pass" }),
        );
        let accept = hex(RFC_ACCEPT);
        assert_eq!(
            judge(&accept, &rfc_authenticator(), RFC_SECRET, &s),
            CheckOutcome::Up
        );
        assert_eq!(
            judge(&accept, &rfc_authenticator(), b"wrong", &s),
            CheckOutcome::Down(
                "auth: response authenticator does not match (wrong secret?)".into()
            )
        );
    }

    #[test]
    fn hmac_md5_matches_rfc_2104() {
        // RFC 2104 appendix test vector 1.
        assert_eq!(
            hmac_md5(&[0x0b; 16], b"Hi There").to_vec(),
            hex("9294727a3638bb1c13f48ef8158bfc9d")
        );
    }

    /// Check a request's Message-Authenticator the way RFC 3579 says a
    /// server does: HMAC-MD5 over the packet with its value zeroed.
    fn request_mac_is_valid(packet: &[u8], secret: &[u8]) -> bool {
        assert_eq!(packet[HEADER], MESSAGE_AUTHENTICATOR, "first attribute");
        assert_eq!(packet[HEADER + 1], 18);
        let mut copy = packet.to_vec();
        copy[HEADER + 2..HEADER + 18].fill(0);
        hmac_md5(secret, &copy)[..] == packet[HEADER + 2..HEADER + 18]
    }

    #[test]
    fn requests_carry_message_authenticator_first() {
        let a = rfc_authenticator();
        let status = build_request(STATUS_SERVER, 7, &a, SECRET, None, b"emfor").unwrap();
        assert_eq!(status[0], STATUS_SERVER);
        assert_eq!(status[1], 7);
        assert_eq!(
            usize::from(u16::from_be_bytes([status[2], status[3]])),
            status.len()
        );
        assert!(request_mac_is_valid(&status, SECRET));
        let attrs = attributes(&status).unwrap();
        let kinds: Vec<u8> = attrs.iter().map(|(_, k, _)| *k).collect();
        assert_eq!(kinds, vec![MESSAGE_AUTHENTICATOR, NAS_IDENTIFIER]);
        assert_eq!(attrs[1].2, b"emfor");

        let access = build_request(
            ACCESS_REQUEST,
            8,
            &a,
            SECRET,
            Some((b"probe", PASSWORD.as_bytes())),
            b"emfor",
        )
        .unwrap();
        assert!(request_mac_is_valid(&access, SECRET));
        let attrs = attributes(&access).unwrap();
        let kinds: Vec<u8> = attrs.iter().map(|(_, k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                MESSAGE_AUTHENTICATOR,
                USER_NAME,
                USER_PASSWORD,
                NAS_IDENTIFIER
            ]
        );
        assert_eq!(attrs[1].2, b"probe");
        assert_eq!(attrs[2].2, hide_password(PASSWORD.as_bytes(), SECRET, &a));
    }

    #[test]
    fn too_long_logins_are_errors() {
        let a = rfc_authenticator();
        let long_user = vec![b'u'; 254];
        let r = build_request(
            ACCESS_REQUEST,
            1,
            &a,
            SECRET,
            Some((&long_user, b"pw")),
            b"emfor",
        );
        assert_eq!(
            r,
            Err("check_options: user: longer than 253 bytes".to_owned())
        );
        let long_pw = vec![b'p'; 129];
        let r = build_request(
            ACCESS_REQUEST,
            1,
            &a,
            SECRET,
            Some((b"probe", &long_pw)),
            b"emfor",
        );
        assert_eq!(
            r,
            Err("password_file: password longer than 128 bytes".to_owned())
        );
    }

    // ---- checks against a fake server ----

    /// How the fake server answers.
    #[derive(Clone, Copy)]
    struct Answer {
        code: u8,
        reply_message: Option<&'static [u8]>,
        message_authenticator: Mac,
        secret: &'static [u8],
        /// Write this into the Length field instead of the real length.
        length: Option<u16>,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Mac {
        Good,
        Wrong,
        Missing,
    }

    fn answer(code: u8) -> Answer {
        Answer {
            code,
            reply_message: None,
            message_authenticator: Mac::Good,
            secret: SECRET,
            length: None,
        }
    }

    /// A response to `request` signed as `a` says.
    fn respond(request: &[u8], a: Answer) -> Vec<u8> {
        let mut r = vec![a.code, request[1], 0, 0];
        r.extend_from_slice(&request[4..HEADER]);
        if a.message_authenticator != Mac::Missing {
            push_attribute(&mut r, MESSAGE_AUTHENTICATOR, &[0u8; 16]);
        }
        if let Some(m) = a.reply_message {
            push_attribute(&mut r, REPLY_MESSAGE, m);
        }
        let len = u16::try_from(r.len()).unwrap();
        r[2..4].copy_from_slice(&len.to_be_bytes());
        if a.message_authenticator != Mac::Missing {
            let mut mac = hmac_md5(a.secret, &r);
            if a.message_authenticator == Mac::Wrong {
                mac[0] ^= 1;
            }
            r[HEADER + 2..HEADER + 18].copy_from_slice(&mac);
        }
        let mut md5 = Md5::new();
        md5.update(&r);
        md5.update(a.secret);
        let response_authenticator: [u8; 16] = md5.finalize().into();
        r[4..HEADER].copy_from_slice(&response_authenticator);
        if let Some(l) = a.length {
            r[2..4].copy_from_slice(&l.to_be_bytes());
        }
        r
    }

    /// A UDP RADIUS server that checks each request's Message-Authenticator
    /// and User-Password, then answers with `reply(request)`.
    async fn server(
        reply: impl Fn(&[u8]) -> Option<Vec<u8>> + Send + 'static,
    ) -> (SocketAddr, std::sync::Arc<std::sync::Mutex<usize>>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let count = std::sync::Arc::new(std::sync::Mutex::new(0));
        let seen = std::sync::Arc::clone(&count);
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok((n, from)) = socket.recv_from(&mut buf).await {
                *seen.lock().unwrap() += 1;
                let request = &buf[..n];
                assert!(request_mac_is_valid(request, SECRET), "bad request MAC");
                let attrs = attributes(request).unwrap();
                if let Some((_, _, hidden)) = attrs.iter().find(|(_, k, _)| *k == USER_PASSWORD) {
                    let a: [u8; 16] = request[4..HEADER].try_into().unwrap();
                    assert_eq!(*hidden, hide_password(PASSWORD.as_bytes(), SECRET, &a));
                }
                if let Some(r) = reply(request) {
                    let _ = socket.send_to(&r, from).await;
                }
            }
        });
        (addr, count)
    }

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest {
            service: "radius".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: Default::default(),
            timeout: Duration::from_millis(900),
        }
    }

    async fn outcome(extra: Value, a: Answer) -> CheckOutcome {
        let f = Files::new();
        let s = parse(&f.options(extra), true).unwrap();
        let (addr, _) = server(move |request| Some(respond(request, a))).await;
        check(&req(addr), &s).await
    }

    fn down(reason: &str) -> CheckOutcome {
        CheckOutcome::Down(reason.to_owned())
    }

    fn access() -> Value {
        json!({ "request": "access-request", "user": "probe", "password_file": "@probe.pass" })
    }

    fn with(base: Value, extra: Value) -> Value {
        let mut o = base;
        for (k, v) in extra.as_object().unwrap() {
            o[k] = v.clone();
        }
        o
    }

    #[tokio::test]
    async fn radius_outcomes() {
        let acct = json!({ "application": "acct" });
        let cases: Vec<(Value, Answer, CheckOutcome)> = vec![
            (json!({}), answer(ACCESS_ACCEPT), CheckOutcome::Up),
            (acct.clone(), answer(ACCOUNTING_RESPONSE), CheckOutcome::Up),
            (
                acct,
                answer(ACCESS_ACCEPT),
                down("query: unexpected response code 2"),
            ),
            (access(), answer(ACCESS_ACCEPT), CheckOutcome::Up),
            (
                access(),
                Answer {
                    reply_message: Some(b"Unknown user"),
                    ..answer(ACCESS_REJECT)
                },
                down("auth: Access-Reject: Unknown user"),
            ),
            (
                with(access(), json!({ "reject_is_up": true })),
                answer(ACCESS_REJECT),
                CheckOutcome::Up,
            ),
            (
                access(),
                answer(ACCESS_CHALLENGE),
                down("auth: Access-Challenge"),
            ),
            (
                json!({}),
                Answer {
                    message_authenticator: Mac::Wrong,
                    ..answer(ACCESS_ACCEPT)
                },
                down("auth: Message-Authenticator does not match"),
            ),
            (
                json!({ "require_message_authenticator": true }),
                Answer {
                    message_authenticator: Mac::Missing,
                    ..answer(ACCESS_ACCEPT)
                },
                down("auth: response has no Message-Authenticator"),
            ),
            (
                json!({}),
                Answer {
                    message_authenticator: Mac::Missing,
                    ..answer(ACCESS_ACCEPT)
                },
                CheckOutcome::Up,
            ),
            (
                json!({}),
                Answer {
                    length: Some(40),
                    ..answer(ACCESS_ACCEPT)
                },
                down("query: malformed response"),
            ),
        ];
        for (i, (extra, a, expected)) in cases.into_iter().enumerate() {
            let got = outcome(extra, a).await;
            assert_eq!(got, expected, "row {i}");
            if let CheckOutcome::Down(r) | CheckOutcome::Error(r) = &got {
                assert!(
                    !r.contains("testing123") && !r.contains(PASSWORD),
                    "row {i}: {r}"
                );
            }
        }
    }

    #[tokio::test]
    async fn malformed_length_in_a_twenty_byte_datagram() {
        let f = Files::new();
        let s = parse(&f.options(json!({})), true).unwrap();
        let (addr, _) = server(|request| {
            let mut r = vec![ACCESS_ACCEPT, request[1], 0, 40];
            r.extend_from_slice(&[0u8; 16]);
            Some(r)
        })
        .await;
        assert_eq!(
            check(&req(addr), &s).await,
            down("query: malformed response")
        );
    }

    #[tokio::test]
    async fn reply_messages_are_joined_and_made_valid_utf8() {
        let f = Files::new();
        let s = parse(&f.options(access()), true).unwrap();
        let (addr, _) = server(|request| {
            let mut r = vec![ACCESS_REJECT, request[1], 0, 0];
            r.extend_from_slice(&request[4..HEADER]);
            push_attribute(&mut r, REPLY_MESSAGE, b"Bad");
            push_attribute(&mut r, REPLY_MESSAGE, b"pass\xffword");
            let len = u16::try_from(r.len()).unwrap();
            r[2..4].copy_from_slice(&len.to_be_bytes());
            let mut md5 = Md5::new();
            md5.update(&r);
            md5.update(SECRET);
            let ra: [u8; 16] = md5.finalize().into();
            r[4..HEADER].copy_from_slice(&ra);
            Some(r)
        })
        .await;
        assert_eq!(
            check(&req(addr), &s).await,
            down("auth: Access-Reject: Bad pass\u{fffd}word")
        );
    }

    #[tokio::test]
    async fn server_answers_status_server() {
        let f = Files::new();
        let s = parse(&f.options(json!({})), true).unwrap();
        let (addr, _) = server(|request| {
            assert_eq!(request[0], STATUS_SERVER);
            Some(respond(request, answer(ACCESS_ACCEPT)))
        })
        .await;
        assert_eq!(check(&req(addr), &s).await, CheckOutcome::Up);
    }

    #[tokio::test]
    async fn wrong_shared_secret() {
        let r = outcome(
            json!({}),
            Answer {
                secret: b"other-secret",
                ..answer(ACCESS_ACCEPT)
            },
        )
        .await;
        assert_eq!(
            r,
            down("auth: response authenticator does not match (wrong secret?)")
        );
    }

    #[tokio::test]
    async fn answer_with_another_identifier_is_ignored() {
        let f = Files::new();
        let s = parse(&f.options(json!({})), true).unwrap();
        let (addr, _) = server(|request| {
            let mut other = request.to_vec();
            other[1] = request[1].wrapping_add(1);
            // Only the answer to another Identifier: the check times out.
            Some(respond(&other, answer(ACCESS_ACCEPT)))
        })
        .await;
        assert_eq!(check(&req(addr), &s).await, down("timeout"));
    }

    #[tokio::test]
    async fn secret_file_removed() {
        let f = Files::new();
        let s = parse(&f.options(json!({})), true).unwrap();
        std::fs::remove_file(f.path("radius.secret")).unwrap();
        let (addr, count) = server(|request| Some(respond(request, answer(ACCESS_ACCEPT)))).await;
        match check(&req(addr), &s).await {
            CheckOutcome::Error(r) => assert!(r.starts_with("secret_file: "), "{r}"),
            other => panic!("expected error, got {other:?}"),
        }
        assert_eq!(*count.lock().unwrap(), 0, "no request is sent");
    }

    #[tokio::test]
    async fn password_file_removed() {
        let f = Files::new();
        let s = parse(&f.options(access()), true).unwrap();
        std::fs::remove_file(f.path("probe.pass")).unwrap();
        let (addr, count) = server(|request| Some(respond(request, answer(ACCESS_ACCEPT)))).await;
        match check(&req(addr), &s).await {
            CheckOutcome::Error(r) => assert!(r.starts_with("password_file: "), "{r}"),
            other => panic!("expected error, got {other:?}"),
        }
        assert_eq!(*count.lock().unwrap(), 0, "no request is sent");
    }

    #[tokio::test]
    async fn server_without_status_server_support() {
        let f = Files::new();
        let s = parse(&f.options(json!({})), true).unwrap();
        let (addr, count) = server(|_| None).await;
        let start = std::time::Instant::now();
        assert_eq!(check(&req(addr), &s).await, down("timeout"));
        assert!(start.elapsed() < Duration::from_millis(1200));
        assert_eq!(
            *count.lock().unwrap(),
            2,
            "sent once more at half the timeout"
        );
    }
}
