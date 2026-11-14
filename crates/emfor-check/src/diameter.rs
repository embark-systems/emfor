//! The `diameter` plugin: on a fresh TCP (or TLS) connection per check,
//! a capabilities exchange (CER / CEA) and a device watchdog (DWR / DWA),
//! then a Disconnect-Peer-Request (RFC 6733).

use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options};
use emfor_tls::{ClientCertificate, TlsSettings};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::time::Instant;

use crate::frame::{Frame, FrameError, FrameReader};
use crate::net::{self, Stream, io_failure};
use crate::options::{boolean, check_file, string, tls_settings};

const KEYS: &[&str] = &[
    "origin_host",
    "origin_realm",
    "auth_application_ids",
    "acct_application_ids",
    "vendor_specific_application_ids",
    "supported_vendor_ids",
    "host_ip_address",
    "tls",
    "tls_verify",
    "server_name",
    "ca_file",
    "tls_cert_file",
    "tls_key_file",
];

const IDS_MESSAGE: &str = "must be an array of integers from 0 to 4294967295";
const VENDOR_APP_MESSAGE: &str =
    "each entry needs vendor_id and one of auth_application_id, acct_application_id";
/// The longest message read.
const MAX_MESSAGE: usize = 65536;
/// How long to wait for the Disconnect-Peer-Answer at most.
const DPA_WAIT: Duration = Duration::from_millis(200);

// Command codes.
const CAPABILITIES_EXCHANGE: u32 = 257;
const DEVICE_WATCHDOG: u32 = 280;
const DISCONNECT_PEER: u32 = 282;

// AVP codes.
const HOST_IP_ADDRESS: u32 = 257;
const AUTH_APPLICATION_ID: u32 = 258;
const ACCT_APPLICATION_ID: u32 = 259;
const VENDOR_SPECIFIC_APPLICATION_ID: u32 = 260;
const ORIGIN_HOST: u32 = 264;
const SUPPORTED_VENDOR_ID: u32 = 265;
const VENDOR_ID: u32 = 266;
const RESULT_CODE: u32 = 268;
const PRODUCT_NAME: u32 = 269;
const DISCONNECT_CAUSE: u32 = 273;
const ORIGIN_REALM: u32 = 296;

/// Disconnect-Cause `DO_NOT_WANT_TO_TALK_TO_YOU`.
const DO_NOT_WANT_TO_TALK_TO_YOU: u32 = 2;

// Header flags.
const FLAG_REQUEST: u8 = 0x80;
// AVP flags.
const AVP_VENDOR: u8 = 0x80;
const AVP_MANDATORY: u8 = 0x40;

/// An application declared in a Vendor-Specific-Application-Id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Application {
    Auth(u32),
    Acct(u32),
}

/// One `vendor_specific_application_ids` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorApplication {
    pub vendor_id: u32,
    pub application: Application,
}

/// Validated `check_options` of the diameter plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub origin_host: String,
    pub origin_realm: String,
    pub auth_application_ids: Vec<u32>,
    pub acct_application_ids: Vec<u32>,
    pub vendor_specific_application_ids: Vec<VendorApplication>,
    pub supported_vendor_ids: Vec<u32>,
    /// `None` for the local address of the connection.
    pub host_ip_address: Option<IpAddr>,
    /// Present only with `tls = true`.
    pub tls: Option<TlsSettings>,
    /// `tls_cert_file` and `tls_key_file`; only with `tls = true`.
    pub client_certificate: Option<ClientCertificate>,
}

/// Parse and validate the diameter plugin's `check_options`; with
/// `read_files` a `ca_file` is read to check its content.
pub fn parse(options: &Options, read_files: bool) -> Result<Settings, Vec<OptionError>> {
    let mut errors = Vec::new();
    for key in options.keys() {
        if !KEYS.contains(&key.as_str()) {
            errors.push(OptionError::new(key, "unknown key"));
        }
    }

    let origin_host = identity(options, "origin_host", &mut errors);
    let origin_realm = identity(options, "origin_realm", &mut errors);

    let before = errors.len();
    let auth_application_ids = ids(options, "auth_application_ids", &mut errors);
    let acct_application_ids = ids(options, "acct_application_ids", &mut errors);
    let vendor_specific_application_ids = vendor_applications(options, &mut errors);
    if errors.len() == before
        && auth_application_ids.is_empty()
        && acct_application_ids.is_empty()
        && vendor_specific_application_ids.is_empty()
    {
        errors.push(OptionError::new(
            "auth_application_ids",
            "at least one application is required",
        ));
    }
    let supported_vendor_ids = ids(options, "supported_vendor_ids", &mut errors);

    let host_ip_address = match string(options, "host_ip_address", &mut errors) {
        None => None,
        Some(s) => match s.parse::<IpAddr>() {
            Ok(ip) => Some(ip),
            Err(_) => {
                errors.push(OptionError::new("host_ip_address", "not an IP address"));
                None
            }
        },
    };

    let tls_on = boolean(options, "tls", false, &mut errors);
    let tls = tls_settings(
        options,
        tls_on,
        "requires tls = true",
        read_files,
        &mut errors,
    );
    let client_certificate = client_certificate(options, tls_on, read_files, &mut errors);

    match (origin_host, origin_realm) {
        (Some(origin_host), Some(origin_realm)) if errors.is_empty() => Ok(Settings {
            origin_host,
            origin_realm,
            auth_application_ids,
            acct_application_ids,
            vendor_specific_application_ids,
            supported_vendor_ids,
            host_ip_address,
            tls,
            client_certificate,
        }),
        _ => Err(errors),
    }
}

/// `tls_cert_file` and `tls_key_file`: both or neither, only with TLS.
fn client_certificate(
    options: &Options,
    tls_on: bool,
    read_files: bool,
    errors: &mut Vec<OptionError>,
) -> Option<ClientCertificate> {
    let file =
        |key: &str, check: fn(&Path) -> Result<(), String>, errors: &mut Vec<OptionError>| {
            let path = Path::new(string(options, key, errors)?);
            match check_file(path, read_files, check) {
                Ok(()) => Some(path.to_path_buf()),
                Err(message) => {
                    errors.push(OptionError::new(key, message));
                    None
                }
            }
        };
    let cert_file: Option<PathBuf> = file("tls_cert_file", emfor_tls::check_cert_file, errors);
    let key_file: Option<PathBuf> = file("tls_key_file", emfor_tls::check_key_file, errors);
    let has = |key: &str| options.contains_key(key);
    match (has("tls_cert_file"), has("tls_key_file")) {
        (true, false) => errors.push(OptionError::new("tls_cert_file", "requires tls_key_file")),
        (false, true) => errors.push(OptionError::new("tls_key_file", "requires tls_cert_file")),
        _ => {}
    }
    if !tls_on {
        for key in ["tls_cert_file", "tls_key_file"] {
            if has(key) {
                errors.push(OptionError::new(key, "requires tls = true"));
            }
        }
        return None;
    }
    let client = ClientCertificate {
        cert_file: cert_file?,
        key_file: key_file?,
    };
    // Each file is usable on its own; build the client configuration a
    // check builds, which also needs the key to fit the certificate.
    if read_files && let Err(message) = emfor_tls::check_client_certificate(&client) {
        errors.push(OptionError::new("tls_key_file", message));
        return None;
    }
    Some(client)
}

/// A required DiameterIdentity option.
fn identity(options: &Options, key: &str, errors: &mut Vec<OptionError>) -> Option<String> {
    if !options.contains_key(key) {
        errors.push(OptionError::new(key, "missing required key"));
        return None;
    }
    let s = string(options, key, errors)?;
    if is_identity(s) {
        Some(s.to_owned())
    } else {
        errors.push(OptionError::new(key, "not a valid DiameterIdentity"));
        None
    }
}

/// 1 to 255 ASCII letters, digits, `-` and `.`, not starting or ending
/// with `.`.
fn is_identity(s: &str) -> bool {
    (1..=255).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        && !s.starts_with('.')
        && !s.ends_with('.')
}

/// An Unsigned32 value.
fn unsigned32(v: &Value) -> Option<u32> {
    v.as_u64().and_then(|n| u32::try_from(n).ok())
}

/// An optional array of Unsigned32 values, empty when absent.
fn ids(options: &Options, key: &str, errors: &mut Vec<OptionError>) -> Vec<u32> {
    let parsed = match options.get(key) {
        None => return Vec::new(),
        Some(Value::Array(list)) => list.iter().map(unsigned32).collect::<Option<Vec<_>>>(),
        Some(_) => None,
    };
    parsed.unwrap_or_else(|| {
        errors.push(OptionError::new(key, IDS_MESSAGE));
        Vec::new()
    })
}

/// `vendor_specific_application_ids`, empty when absent.
fn vendor_applications(options: &Options, errors: &mut Vec<OptionError>) -> Vec<VendorApplication> {
    let key = "vendor_specific_application_ids";
    let parsed = match options.get(key) {
        None => return Vec::new(),
        Some(Value::Array(list)) => list
            .iter()
            .map(vendor_application)
            .collect::<Option<Vec<_>>>(),
        Some(_) => None,
    };
    parsed.unwrap_or_else(|| {
        errors.push(OptionError::new(key, VENDOR_APP_MESSAGE));
        Vec::new()
    })
}

fn vendor_application(entry: &Value) -> Option<VendorApplication> {
    let table = entry.as_object()?;
    if table.len() != 2 {
        return None;
    }
    let vendor_id = unsigned32(table.get("vendor_id")?)?;
    let application = match (
        table.get("auth_application_id"),
        table.get("acct_application_id"),
    ) {
        (Some(id), None) => Application::Auth(unsigned32(id)?),
        (None, Some(id)) => Application::Acct(unsigned32(id)?),
        _ => return None,
    };
    Some(VendorApplication {
        vendor_id,
        application,
    })
}

// ---- encoding ----

/// One AVP without a Vendor-Id, padded to a multiple of 4 bytes.
fn avp(code: u32, flags: u8, data: &[u8]) -> Vec<u8> {
    let length = 8 + data.len();
    let mut out = Vec::with_capacity(length + 3);
    out.extend_from_slice(&code.to_be_bytes());
    out.push(flags);
    out.extend_from_slice(&length_bytes(length));
    out.extend_from_slice(data);
    out.resize(out.len().next_multiple_of(4), 0);
    out
}

fn avp_u32(code: u32, value: u32) -> Vec<u8> {
    avp(code, AVP_MANDATORY, &value.to_be_bytes())
}

/// The Address type: address family (1 IPv4, 2 IPv6) and the address.
fn address(ip: IpAddr) -> Vec<u8> {
    match ip {
        IpAddr::V4(v4) => [&[0, 1][..], &v4.octets()].concat(),
        IpAddr::V6(v6) => [&[0, 2][..], &v6.octets()].concat(),
    }
}

/// The low 3 bytes of `n`, which callers keep below 2^24.
fn length_bytes(n: usize) -> [u8; 3] {
    let [_, a, b, c] = u32::try_from(n).unwrap_or(u32::MAX).to_be_bytes();
    [a, b, c]
}

/// The identifiers of the requests of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ids {
    hop_by_hop: u32,
    end_to_end: u32,
}

impl Ids {
    fn random() -> Ids {
        Ids {
            hop_by_hop: net::random(),
            end_to_end: net::random(),
        }
    }

    /// The identifiers for the next request on the same connection.
    fn next(self) -> Ids {
        Ids {
            hop_by_hop: self.hop_by_hop.wrapping_add(1),
            end_to_end: self.end_to_end.wrapping_add(1),
        }
    }
}

/// A whole message: the 20-byte header and the AVPs.
fn message(flags: u8, command: u32, application_id: u32, ids: Ids, avps: &[u8]) -> Vec<u8> {
    let length = 20 + avps.len();
    let mut out = Vec::with_capacity(length);
    out.push(1);
    out.extend_from_slice(&length_bytes(length));
    out.push(flags);
    out.extend_from_slice(&length_bytes(command as usize));
    out.extend_from_slice(&application_id.to_be_bytes());
    out.extend_from_slice(&ids.hop_by_hop.to_be_bytes());
    out.extend_from_slice(&ids.end_to_end.to_be_bytes());
    out.extend_from_slice(avps);
    out
}

/// Origin-Host and Origin-Realm, which every request starts with.
fn origin(settings: &Settings) -> Vec<u8> {
    [
        avp(ORIGIN_HOST, AVP_MANDATORY, settings.origin_host.as_bytes()),
        avp(
            ORIGIN_REALM,
            AVP_MANDATORY,
            settings.origin_realm.as_bytes(),
        ),
    ]
    .concat()
}

fn capabilities_exchange_request(settings: &Settings, host_ip: IpAddr, ids: Ids) -> Vec<u8> {
    let mut avps = origin(settings);
    avps.extend(avp(HOST_IP_ADDRESS, AVP_MANDATORY, &address(host_ip)));
    avps.extend(avp_u32(VENDOR_ID, 0));
    avps.extend(avp(PRODUCT_NAME, 0, b"emfor"));
    for id in &settings.supported_vendor_ids {
        avps.extend(avp_u32(SUPPORTED_VENDOR_ID, *id));
    }
    for id in &settings.auth_application_ids {
        avps.extend(avp_u32(AUTH_APPLICATION_ID, *id));
    }
    for id in &settings.acct_application_ids {
        avps.extend(avp_u32(ACCT_APPLICATION_ID, *id));
    }
    for app in &settings.vendor_specific_application_ids {
        let inner = match app.application {
            Application::Auth(id) => avp_u32(AUTH_APPLICATION_ID, id),
            Application::Acct(id) => avp_u32(ACCT_APPLICATION_ID, id),
        };
        let group = [avp_u32(VENDOR_ID, app.vendor_id), inner].concat();
        avps.extend(avp(VENDOR_SPECIFIC_APPLICATION_ID, AVP_MANDATORY, &group));
    }
    message(FLAG_REQUEST, CAPABILITIES_EXCHANGE, 0, ids, &avps)
}

fn device_watchdog_request(settings: &Settings, ids: Ids) -> Vec<u8> {
    message(FLAG_REQUEST, DEVICE_WATCHDOG, 0, ids, &origin(settings))
}

fn disconnect_peer_request(settings: &Settings, ids: Ids) -> Vec<u8> {
    let mut avps = origin(settings);
    avps.extend(avp_u32(DISCONNECT_CAUSE, DO_NOT_WANT_TO_TALK_TO_YOU));
    message(FLAG_REQUEST, DISCONNECT_PEER, 0, ids, &avps)
}

// ---- decoding ----

/// One decoded AVP.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Avp {
    code: u32,
    flags: u8,
    vendor_id: Option<u32>,
    data: Vec<u8>,
}

/// One decoded message.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Message {
    flags: u8,
    command: u32,
    ids: Ids,
    avps: Vec<Avp>,
}

impl Message {
    fn is_request(&self) -> bool {
        self.flags & FLAG_REQUEST != 0
    }

    /// Whether this is the answer to the request `command` sent with `ids`.
    fn answers(&self, command: u32, ids: Ids) -> bool {
        !self.is_request() && self.command == command && self.ids.hop_by_hop == ids.hop_by_hop
    }

    /// The Result-Code, `Err` when it is missing or malformed.
    fn result_code(&self) -> Result<u32, String> {
        let avp = self
            .avps
            .iter()
            .find(|a| a.code == RESULT_CODE && a.vendor_id.is_none())
            .ok_or("query: answer without Result-Code")?;
        let bytes: [u8; 4] = avp
            .data
            .as_slice()
            .try_into()
            .map_err(|_| "query: malformed Result-Code")?;
        Ok(u32::from_be_bytes(bytes))
    }

    /// The Unsigned32 value of the first AVP `code` without a Vendor-Id.
    #[cfg(test)]
    fn u32_avp(&self, code: u32) -> Option<u32> {
        let avp = self
            .avps
            .iter()
            .find(|a| a.code == code && a.vendor_id.is_none())?;
        Some(u32::from_be_bytes(avp.data.as_slice().try_into().ok()?))
    }
}

/// Check the start of a message, which needs at least 4 bytes: the
/// version and the message length.
fn message_length(head: &[u8]) -> Result<usize, String> {
    let [version, a, b, c, ..] = *head else {
        return Err("query: message shorter than its header".into());
    };
    if version != 1 {
        return Err(format!("query: unsupported Diameter version {version}"));
    }
    let length = u32::from_be_bytes([0, a, b, c]) as usize;
    if length < 20 {
        return Err(format!("query: message length {length} below 20"));
    }
    if length > MAX_MESSAGE {
        return Err("query: message longer than 65536 bytes".into());
    }
    Ok(length)
}

/// Decode a whole message.
fn decode(bytes: &[u8]) -> Result<Message, String> {
    let length = message_length(bytes)?;
    if bytes.len() != length || length % 4 != 0 {
        return Err("query: message length does not match its AVPs".into());
    }
    let word =
        |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let ids = Ids {
        hop_by_hop: word(12),
        end_to_end: word(16),
    };
    Ok(Message {
        flags: bytes[4],
        command: word(4) & 0x00ff_ffff,
        ids,
        avps: decode_avps(&bytes[20..])?,
    })
}

/// Decode a run of AVPs that fills `bytes` exactly.
fn decode_avps(mut bytes: &[u8]) -> Result<Vec<Avp>, String> {
    let invalid = || "query: invalid AVP length".to_owned();
    let mut avps = Vec::new();
    while !bytes.is_empty() {
        let [c0, c1, c2, c3, flags, l0, l1, l2, ..] = *bytes else {
            return Err(invalid());
        };
        let length = u32::from_be_bytes([0, l0, l1, l2]) as usize;
        let header = if flags & AVP_VENDOR != 0 { 12 } else { 8 };
        let padded = length.next_multiple_of(4);
        if length < header || padded > bytes.len() {
            return Err(invalid());
        }
        let vendor_id =
            (header == 12).then(|| u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]));
        avps.push(Avp {
            code: u32::from_be_bytes([c0, c1, c2, c3]),
            flags,
            vendor_id,
            data: bytes[header..length].to_vec(),
        });
        bytes = &bytes[padded..];
    }
    Ok(avps)
}

/// The RFC 6733 section 7.1 name of a result code.
fn result_name(code: u32) -> Option<&'static str> {
    Some(match code {
        1001 => "DIAMETER_MULTI_ROUND_AUTH",
        2001 => "DIAMETER_SUCCESS",
        2002 => "DIAMETER_LIMITED_SUCCESS",
        3001 => "DIAMETER_COMMAND_UNSUPPORTED",
        3002 => "DIAMETER_UNABLE_TO_DELIVER",
        3003 => "DIAMETER_REALM_NOT_SERVED",
        3004 => "DIAMETER_TOO_BUSY",
        3005 => "DIAMETER_LOOP_DETECTED",
        3006 => "DIAMETER_REDIRECT_INDICATION",
        3007 => "DIAMETER_APPLICATION_UNSUPPORTED",
        3008 => "DIAMETER_INVALID_HDR_BITS",
        3009 => "DIAMETER_INVALID_AVP_BITS",
        3010 => "DIAMETER_UNKNOWN_PEER",
        4001 => "DIAMETER_AUTHENTICATION_REJECTED",
        4002 => "DIAMETER_OUT_OF_SPACE",
        4003 => "ELECTION_LOST",
        5001 => "DIAMETER_AVP_UNSUPPORTED",
        5002 => "DIAMETER_UNKNOWN_SESSION_ID",
        5003 => "DIAMETER_AUTHORIZATION_REJECTED",
        5004 => "DIAMETER_INVALID_AVP_VALUE",
        5005 => "DIAMETER_MISSING_AVP",
        5006 => "DIAMETER_RESOURCES_EXCEEDED",
        5007 => "DIAMETER_CONTRADICTING_AVPS",
        5008 => "DIAMETER_AVP_NOT_ALLOWED",
        5009 => "DIAMETER_AVP_OCCURS_TOO_MANY_TIMES",
        5010 => "DIAMETER_NO_COMMON_APPLICATION",
        5011 => "DIAMETER_UNSUPPORTED_VERSION",
        5012 => "DIAMETER_UNABLE_TO_COMPLY",
        5013 => "DIAMETER_INVALID_BIT_IN_HEADER",
        5014 => "DIAMETER_INVALID_AVP_LENGTH",
        5015 => "DIAMETER_INVALID_MESSAGE_LENGTH",
        5016 => "DIAMETER_INVALID_AVP_BIT_COMBO",
        5017 => "DIAMETER_NO_COMMON_SECURITY",
        _ => return None,
    })
}

/// `down` unless the answer's Result-Code is from 2000 to 2999.
fn judge(answer: &Message, stage: &str) -> Result<(), CheckOutcome> {
    let code = answer.result_code().map_err(CheckOutcome::Down)?;
    if (2000..=2999).contains(&code) {
        return Ok(());
    }
    Err(CheckOutcome::Down(match result_name(code) {
        Some(name) => format!("{stage}: {code} {name}"),
        None => format!("{stage}: {code}"),
    }))
}

// ---- the check ----

/// A connection on which the capabilities exchange succeeded.
struct Peer {
    conn: Box<dyn Stream>,
    buf: FrameReader,
    ids: Ids,
}

pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let deadline = Instant::now() + req.timeout;
    let mut peer = None;
    let outcome = match tokio::time::timeout_at(deadline, exchange(req, settings, &mut peer)).await
    {
        Ok(Ok(())) => CheckOutcome::Up,
        Ok(Err(outcome)) => outcome,
        Err(_) => CheckOutcome::Down("timeout".into()),
    };
    if let Some(peer) = peer {
        // The result is already decided: disconnect in the background so a
        // slow or missing Disconnect-Peer-Answer cannot delay it past the
        // plugin runner's own timeout.
        let ids = peer.ids.next();
        let dpr = disconnect_peer_request(settings, ids);
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(DPA_WAIT);
        tokio::spawn(disconnect(peer, dpr, ids, wait));
    }
    outcome
}

/// Connect, exchange capabilities and send a watchdog; `peer` is set once
/// the capabilities exchange succeeded.
async fn exchange(
    req: &CheckRequest,
    settings: &Settings,
    peer: &mut Option<Peer>,
) -> Result<(), CheckOutcome> {
    let start = Instant::now();
    // The certificate files are read before connecting: a missing one is
    // this host's problem and nothing is sent.
    let tls = match &settings.tls {
        None => None,
        Some(tls) => Some((
            tls,
            emfor_tls::client_config_with_client(tls, settings.client_certificate.as_ref())
                .map_err(net::tls_outcome)?,
        )),
    };
    let stream = net::connect(req.ip, req.port).await?;
    let local = stream
        .local_addr()
        .map_err(|e| io_failure("connect: ", &e))?;
    let mut conn: Box<dyn Stream> = match tls {
        None => Box::new(stream),
        Some((tls, config)) => {
            let remaining = req.timeout.saturating_sub(start.elapsed());
            Box::new(
                emfor_tls::handshake_with_config(stream, config, tls, req.ip, remaining)
                    .await
                    .map_err(net::tls_outcome)?,
            )
        }
    };
    let host_ip = settings.host_ip_address.unwrap_or(local.ip());
    let ids = Ids::random();
    let mut buf = FrameReader::read_ahead(MAX_MESSAGE);
    send(
        &mut conn,
        &capabilities_exchange_request(settings, host_ip, ids),
    )
    .await?;
    let cea = answer(&mut conn, &mut buf, CAPABILITIES_EXCHANGE, ids, "cea").await?;
    judge(&cea, "cea")?;

    let peer = peer.insert(Peer {
        conn,
        buf,
        ids: ids.next(),
    });
    send(&mut peer.conn, &device_watchdog_request(settings, peer.ids)).await?;
    let dwa = answer(
        &mut peer.conn,
        &mut peer.buf,
        DEVICE_WATCHDOG,
        peer.ids,
        "dwa",
    )
    .await?;
    judge(&dwa, "dwa")
}

/// Send the Disconnect-Peer-Request `dpr` (sent with `ids`), wait at most
/// `wait` for the answer, then close the connection.
async fn disconnect(mut peer: Peer, dpr: Vec<u8>, ids: Ids, wait: Duration) {
    let _ = tokio::time::timeout(wait, async {
        send(&mut peer.conn, &dpr).await?;
        answer(&mut peer.conn, &mut peer.buf, DISCONNECT_PEER, ids, "dpa").await
    })
    .await;
    let _ = tokio::time::timeout(wait, peer.conn.shutdown()).await;
}

async fn send(conn: &mut Box<dyn Stream>, bytes: &[u8]) -> Result<(), CheckOutcome> {
    conn.write_all(bytes)
        .await
        .map_err(|e| io_failure("query: ", &e))
}

/// Read messages until the answer to the request `command` sent with
/// `ids`; requests from the peer and other answers are skipped. `stage`
/// names the awaited answer in reasons (`cea: connection closed by peer`).
async fn answer(
    conn: &mut Box<dyn Stream>,
    buf: &mut FrameReader,
    command: u32,
    ids: Ids,
    stage: &str,
) -> Result<Message, CheckOutcome> {
    loop {
        let message = read_message(conn, buf, stage).await?;
        if message.answers(command, ids) {
            return Ok(message);
        }
    }
}

/// Read one message from `conn`; `buf` keeps bytes that arrived early.
async fn read_message(
    conn: &mut Box<dyn Stream>,
    buf: &mut FrameReader,
    stage: &str,
) -> Result<Message, CheckOutcome> {
    let mut framer = |bytes: &[u8]| {
        if bytes.len() < 4 {
            return Frame::Need(4);
        }
        match message_length(bytes) {
            Ok(length) if bytes.len() >= length => Frame::Complete(length),
            Ok(length) => Frame::Need(length),
            Err(reason) => Frame::Invalid(reason),
        }
    };
    match buf.next(conn, &mut framer).await {
        Ok(bytes) => decode(&bytes).map_err(CheckOutcome::Down),
        Err(FrameError::Io(e)) if !closed(&e) => Err(io_failure("query: ", &e)),
        Err(FrameError::Closed { .. } | FrameError::Io(_)) => Err(CheckOutcome::Down(format!(
            "{stage}: connection closed by peer"
        ))),
        // message_length already refuses longer messages.
        Err(FrameError::TooLarge) => Err(CheckOutcome::Down(
            "query: message longer than 65536 bytes".into(),
        )),
        Err(FrameError::Invalid(reason)) => Err(CheckOutcome::Down(reason)),
    }
}

/// Errors that mean the peer closed the connection.
fn closed(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::UnexpectedEof
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::net::SocketAddr;
    use tokio::io::AsyncReadExt;
    use tokio::net::{TcpListener, TcpStream};

    /// The E flag of an error answer.
    const FLAG_ERROR: u8 = 0x20;

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

    /// Minimal valid options plus `extra`.
    fn base(extra: Value) -> Value {
        let mut o = json!({
            "origin_host": "emfor.example.org",
            "origin_realm": "example.org",
            "auth_application_ids": [4],
        });
        for (k, v) in extra.as_object().expect("extra is an object") {
            o[k] = v.clone();
        }
        o
    }

    // ---- options ----

    #[test]
    fn diameter_option_validation() {
        assert_eq!(
            errors(json!({ "origin_realm": "example.org", "auth_application_ids": [4] })),
            one("origin_host", "missing required key")
        );
        assert_eq!(
            errors(json!({ "origin_host": "emfor.example.org", "origin_realm": "example.org" })),
            one(
                "auth_application_ids",
                "at least one application is required"
            )
        );
        assert_eq!(errors(base(json!({}))), Vec::new());
        assert_eq!(
            errors(base(json!({ "origin_host": "emfor example" }))),
            one("origin_host", "not a valid DiameterIdentity")
        );
        assert_eq!(
            errors(base(json!({ "auth_application_ids": [-1] }))),
            one("auth_application_ids", IDS_MESSAGE)
        );
        assert_eq!(
            errors(json!({
                "origin_host": "emfor.example.org", "origin_realm": "example.org",
                "vendor_specific_application_ids": [{ "vendor_id": 10415 }],
            })),
            one("vendor_specific_application_ids", VENDOR_APP_MESSAGE)
        );
        let acct = |extra: Value| {
            let mut o = json!({
                "origin_host": "emfor.example.org", "origin_realm": "example.org",
                "acct_application_ids": [3],
            });
            for (k, v) in extra.as_object().expect("extra is an object") {
                o[k] = v.clone();
            }
            o
        };
        assert_eq!(
            errors(acct(json!({ "host_ip_address": "emfor01" }))),
            one("host_ip_address", "not an IP address")
        );
        // The example's /etc/emfor/ca.pem, as a readable CA file.
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, emfor_tls::testing::pki("dra.example.org").ca_pem).unwrap();
        assert_eq!(
            errors(acct(json!({ "ca_file": ca.to_str().unwrap() }))),
            one("ca_file", "requires tls = true")
        );
        // The example's /etc/emfor/diameter.crt and .key, as readable files.
        let pki = emfor_tls::testing::mutual_pki("dra.example.org", "emfor.example.org");
        let crt = dir.path().join("diameter.crt");
        let key = dir.path().join("diameter.key");
        std::fs::write(&crt, &pki.client_cert_pem).unwrap();
        std::fs::write(&key, &pki.client_key_pem).unwrap();
        let (crt, key) = (crt.to_str().unwrap(), key.to_str().unwrap());
        assert_eq!(
            errors(acct(json!({ "tls": true, "tls_cert_file": crt }))),
            one("tls_cert_file", "requires tls_key_file")
        );
        assert_eq!(
            errors(acct(json!({ "tls_cert_file": crt, "tls_key_file": key }))),
            vec![
                ("tls_cert_file".to_owned(), "requires tls = true".to_owned()),
                ("tls_key_file".to_owned(), "requires tls = true".to_owned()),
            ]
        );
        assert_eq!(
            errors(acct(json!({ "tls": true, "tls_key_file": key }))),
            one("tls_key_file", "requires tls_cert_file")
        );
        assert_eq!(
            errors(acct(
                json!({ "tls": true, "tls_cert_file": key, "tls_key_file": crt })
            )),
            vec![
                (
                    "tls_cert_file".to_owned(),
                    "no certificate found".to_owned()
                ),
                ("tls_key_file".to_owned(), "no private key found".to_owned()),
            ]
        );
        assert_eq!(
            errors(acct(
                json!({ "tls": true, "tls_cert_file": "rel.crt", "tls_key_file": "/nonexistent/k" })
            )),
            vec![
                (
                    "tls_cert_file".to_owned(),
                    "not an absolute path".to_owned()
                ),
                ("tls_key_file".to_owned(), "file not found".to_owned()),
            ]
        );
        assert_eq!(
            errors(acct(
                json!({ "tls": true, "tls_cert_file": crt, "tls_key_file": key })
            )),
            Vec::new()
        );
        // Each file is fine on its own, but the key belongs to another
        // certificate: a check could not build its TLS configuration.
        let other = dir.path().join("other.key");
        let other_pki = emfor_tls::testing::mutual_pki("dra.example.org", "other.example.org");
        std::fs::write(&other, &other_pki.client_key_pem).unwrap();
        let wrong = errors(acct(
            json!({ "tls": true, "tls_cert_file": crt, "tls_key_file": other.to_str().unwrap() }),
        ));
        assert_eq!(wrong.len(), 1, "{wrong:?}");
        assert_eq!(wrong[0].0, "tls_key_file", "{wrong:?}");
    }

    #[test]
    fn gx_peer_options() {
        let s = parse(
            &options(json!({
                "origin_host": "emfor-app01.example.org",
                "origin_realm": "example.org",
                "vendor_specific_application_ids": [{ "vendor_id": 10415, "auth_application_id": 16777238 }],
                "supported_vendor_ids": [10415],
            })),
            true,
        )
        .unwrap();
        assert_eq!(
            s.vendor_specific_application_ids,
            vec![VendorApplication {
                vendor_id: 10415,
                application: Application::Auth(16777238)
            }]
        );
        assert_eq!(s.supported_vendor_ids, vec![10415]);
        assert_eq!(s.host_ip_address, None);
        assert_eq!(s.tls, None);
    }

    #[test]
    fn other_option_errors() {
        assert_eq!(
            errors(base(json!({ "origin_realm": 5 }))),
            one("origin_realm", "must be a string")
        );
        assert_eq!(
            errors(json!({ "origin_host": "a.b", "auth_application_ids": [1] })),
            one("origin_realm", "missing required key")
        );
        for bad in [".example.org", "example.org.", "", "é.org"] {
            assert_eq!(
                errors(base(json!({ "origin_realm": bad }))),
                one("origin_realm", "not a valid DiameterIdentity"),
                "{bad:?}"
            );
        }
        assert_eq!(
            errors(base(json!({ "origin_host": "a".repeat(256) }))),
            one("origin_host", "not a valid DiameterIdentity")
        );
        assert_eq!(
            errors(base(json!({ "origin_host": "a".repeat(255) }))),
            Vec::new()
        );
        assert_eq!(
            errors(base(json!({ "supported_vendor_ids": [4294967296u64] }))),
            one("supported_vendor_ids", IDS_MESSAGE)
        );
        assert_eq!(
            errors(base(json!({ "acct_application_ids": "3" }))),
            one("acct_application_ids", IDS_MESSAGE)
        );
        assert_eq!(
            errors(base(json!({ "auth_application_ids": [4294967295u64, 0] }))),
            Vec::new()
        );
        for entry in [
            json!({ "vendor_id": 1, "auth_application_id": 2, "acct_application_id": 3 }),
            json!({ "vendor_id": 1, "auth_application_id": -2 }),
            json!({ "auth_application_id": 2, "x": 1 }),
            json!(7),
        ] {
            assert_eq!(
                errors(base(
                    json!({ "vendor_specific_application_ids": [entry.clone()] })
                )),
                one("vendor_specific_application_ids", VENDOR_APP_MESSAGE),
                "{entry}"
            );
        }
        assert_eq!(
            errors(base(json!({ "host_ip_address": 1 }))),
            one("host_ip_address", "must be a string")
        );
        assert_eq!(
            parse(
                &options(base(json!({ "host_ip_address": "2001:db8::5" }))),
                true
            )
            .unwrap()
            .host_ip_address,
            Some("2001:db8::5".parse().unwrap())
        );
        assert_eq!(
            errors(base(json!({ "tls": "yes" }))),
            one("tls", "must be a boolean")
        );
        assert_eq!(
            errors(base(json!({ "realm": "x" }))),
            one("realm", "unknown key")
        );
        assert_eq!(
            errors(base(
                json!({ "tls": true, "server_name": "dra.example.org" })
            )),
            Vec::new()
        );
    }

    // ---- codec ----

    fn settings(extra: Value) -> Settings {
        parse(&options(base(extra)), true).unwrap()
    }

    const ONES: Ids = Ids {
        hop_by_hop: 1,
        end_to_end: 1,
    };

    #[test]
    fn cer_encoding() {
        let s = settings(json!({}));
        let cer = capabilities_exchange_request(&s, "10.0.0.5".parse().unwrap(), ONES);
        let length = length_bytes(cer.len());
        let mut header = vec![1];
        header.extend_from_slice(&length);
        header.extend_from_slice(&[0x80, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1]);
        assert_eq!(&cer[..20], header.as_slice());

        // Origin-Host: code 264, flags 40, length 8 + 17, value, 3 bytes padding.
        let mut origin_host = vec![0, 0, 1, 8, 0x40, 0, 0, 25];
        origin_host.extend_from_slice(b"emfor.example.org\0\0\0");
        assert_eq!(&cer[20..48], origin_host.as_slice());

        // The whole message, written out by hand.
        let mut expected: Vec<u8> = vec![
            1, 0, 0, 124, 0x80, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1,
        ];
        expected.extend_from_slice(&[0, 0, 1, 8, 0x40, 0, 0, 25]);
        expected.extend_from_slice(b"emfor.example.org\0\0\0");
        expected.extend_from_slice(&[0, 0, 1, 0x28, 0x40, 0, 0, 19]);
        expected.extend_from_slice(b"example.org\0");
        expected.extend_from_slice(&[0, 0, 1, 1, 0x40, 0, 0, 14, 0, 1, 10, 0, 0, 5, 0, 0]);
        expected.extend_from_slice(&[0, 0, 1, 0x0a, 0x40, 0, 0, 12, 0, 0, 0, 0]);
        expected.extend_from_slice(&[0, 0, 1, 0x0d, 0, 0, 0, 13]);
        expected.extend_from_slice(b"emfor\0\0\0");
        expected.extend_from_slice(&[0, 0, 1, 2, 0x40, 0, 0, 12, 0, 0, 0, 4]);
        assert_eq!(cer, expected);

        let m = decode(&cer).unwrap();
        let codes: Vec<u32> = m.avps.iter().map(|a| a.code).collect();
        assert_eq!(
            codes,
            vec![
                ORIGIN_HOST,
                ORIGIN_REALM,
                HOST_IP_ADDRESS,
                VENDOR_ID,
                PRODUCT_NAME,
                AUTH_APPLICATION_ID
            ]
        );
        let host_ip = &m.avps[2];
        assert_eq!(host_ip.data, [0, 1, 10, 0, 0, 5]);
        assert_eq!(host_ip.flags, AVP_MANDATORY);
        let product = &m.avps[4];
        assert_eq!((product.flags, product.data.as_slice()), (0, &b"emfor"[..]));
        assert!(
            m.avps
                .iter()
                .filter(|a| a.code != PRODUCT_NAME)
                .all(|a| a.flags == AVP_MANDATORY)
        );
        assert_eq!(m.u32_avp(AUTH_APPLICATION_ID), Some(4));
        assert_eq!(m.u32_avp(VENDOR_ID), Some(0));
    }

    #[test]
    fn cer_with_ipv6_and_vendor_specific_applications() {
        let s = settings(json!({
            "supported_vendor_ids": [10415],
            "acct_application_ids": [3],
            "vendor_specific_application_ids": [
                { "vendor_id": 10415, "auth_application_id": 16777238 },
                { "vendor_id": 10415, "acct_application_id": 16777239 },
            ],
        }));
        let cer = capabilities_exchange_request(&s, "2001:db8::5".parse().unwrap(), ONES);
        let m = decode(&cer).unwrap();
        let host_ip = m.avps.iter().find(|a| a.code == HOST_IP_ADDRESS).unwrap();
        let mut v6 = vec![0, 2];
        v6.extend_from_slice(
            &"2001:db8::5"
                .parse::<std::net::Ipv6Addr>()
                .unwrap()
                .octets(),
        );
        assert_eq!(host_ip.data, v6);

        let codes: Vec<u32> = m.avps.iter().map(|a| a.code).collect();
        assert_eq!(
            &codes[5..],
            &[
                SUPPORTED_VENDOR_ID,
                AUTH_APPLICATION_ID,
                ACCT_APPLICATION_ID,
                VENDOR_SPECIFIC_APPLICATION_ID,
                VENDOR_SPECIFIC_APPLICATION_ID
            ]
        );
        let groups: Vec<&Avp> = m
            .avps
            .iter()
            .filter(|a| a.code == VENDOR_SPECIFIC_APPLICATION_ID)
            .collect();
        // Vendor-Id (12 bytes) and an application id (12 bytes).
        assert_eq!(groups[0].data.len(), 24);
        assert_eq!(
            decode_avps(&groups[0].data).unwrap(),
            vec![
                Avp {
                    code: VENDOR_ID,
                    flags: AVP_MANDATORY,
                    vendor_id: None,
                    data: 10415u32.to_be_bytes().to_vec()
                },
                Avp {
                    code: AUTH_APPLICATION_ID,
                    flags: AVP_MANDATORY,
                    vendor_id: None,
                    data: 16777238u32.to_be_bytes().to_vec()
                },
            ]
        );
        assert_eq!(
            decode_avps(&groups[1].data).unwrap()[1].code,
            ACCT_APPLICATION_ID
        );
        // The whole group AVP: 8-byte header and 24 bytes, no padding.
        let at = cer
            .windows(4)
            .position(|w| w == VENDOR_SPECIFIC_APPLICATION_ID.to_be_bytes())
            .unwrap();
        assert_eq!(&cer[at + 4..at + 8], &[0x40, 0, 0, 32]);
    }

    #[test]
    fn dwr_and_dpr_encoding() {
        let s = settings(json!({}));
        let dwr = decode(&device_watchdog_request(&s, ONES)).unwrap();
        assert_eq!((dwr.command, dwr.flags), (DEVICE_WATCHDOG, FLAG_REQUEST));
        assert_eq!(
            dwr.avps.iter().map(|a| a.code).collect::<Vec<_>>(),
            vec![ORIGIN_HOST, ORIGIN_REALM]
        );
        let dpr = decode(&disconnect_peer_request(&s, ONES)).unwrap();
        assert_eq!(dpr.command, DISCONNECT_PEER);
        assert_eq!(dpr.u32_avp(DISCONNECT_CAUSE), Some(2));
    }

    #[test]
    fn decoding_errors() {
        let ok = message(0, 257, 0, ONES, &avp_u32(RESULT_CODE, 2001));
        assert_eq!(decode(&ok).unwrap().result_code(), Ok(2001));

        let mut v2 = ok.clone();
        v2[0] = 2;
        assert_eq!(
            decode(&v2),
            Err("query: unsupported Diameter version 2".into())
        );

        let mut short = ok.clone();
        short[1..4].copy_from_slice(&[0, 0, 16]);
        assert_eq!(
            decode(&short),
            Err("query: message length 16 below 20".into())
        );

        let mut mismatch = ok.clone();
        mismatch[3] += 4;
        assert_eq!(
            decode(&mismatch),
            Err("query: message length does not match its AVPs".into())
        );

        // An AVP claiming more bytes than the message has, and one shorter
        // than its own header.
        for avp_length in [40u8, 4] {
            let mut bad = ok.clone();
            bad[27] = avp_length;
            assert_eq!(
                decode(&bad),
                Err("query: invalid AVP length".into()),
                "{avp_length}"
            );
        }

        let long = [1u8, 1, 0, 4];
        assert_eq!(
            message_length(&long),
            Err("query: message longer than 65536 bytes".into())
        );

        let vendor = {
            let mut a = vec![0, 0, 1, 12, AVP_VENDOR | AVP_MANDATORY, 0, 0, 16];
            a.extend_from_slice(&10415u32.to_be_bytes());
            a.extend_from_slice(&2001u32.to_be_bytes());
            a
        };
        let m = decode(&message(0, 257, 0, ONES, &vendor)).unwrap();
        assert_eq!(m.avps[0].vendor_id, Some(10415));
        assert_eq!(
            m.result_code(),
            Err("query: answer without Result-Code".into())
        );
        let m = decode(&message(
            0,
            257,
            0,
            ONES,
            &avp(RESULT_CODE, AVP_MANDATORY, &[1, 2]),
        ))
        .unwrap();
        assert_eq!(m.result_code(), Err("query: malformed Result-Code".into()));
    }

    #[test]
    fn result_names() {
        assert_eq!(result_name(3010), Some("DIAMETER_UNKNOWN_PEER"));
        assert_eq!(result_name(5010), Some("DIAMETER_NO_COMMON_APPLICATION"));
        assert_eq!(result_name(3004), Some("DIAMETER_TOO_BUSY"));
        assert_eq!(result_name(4999), None);
    }

    // ---- checks against a fake Diameter node ----

    /// What the fake node does next.
    #[derive(Clone)]
    enum Step {
        /// Read a request and answer it with this Result-Code (none when
        /// `None`), with the E flag when the bool is set.
        Answer(Option<u32>, bool),
        /// Send a DWR of its own, with the identifiers the client's next
        /// request will have, so that only the R flag sets it apart.
        PeerRequest,
        /// Read a request and answer with these bytes.
        Raw(Vec<u8>),
        /// Read a request and close the connection.
        Close,
        /// Read a request and never answer.
        Silent,
        /// Read a request and answer it with this Result-Code after a delay.
        AnswerAfter(Duration, u32),
    }

    async fn read_request(s: &mut TcpStream) -> Option<Message> {
        let mut head = [0u8; 20];
        s.read_exact(&mut head).await.ok()?;
        let length = message_length(&head).ok()?;
        let mut bytes = head.to_vec();
        bytes.resize(length, 0);
        s.read_exact(&mut bytes[20..]).await.ok()?;
        decode(&bytes).ok()
    }

    fn answer_to(request: &Message, code: Option<u32>, error: bool) -> Vec<u8> {
        let avps = code.map(|c| avp_u32(RESULT_CODE, c)).unwrap_or_default();
        let flags = if error { FLAG_ERROR } else { 0 };
        message(flags, request.command, 0, request.ids, &avps)
    }

    /// A fake node following `steps`, then answering a DPR and recording
    /// what else arrives until the client closes. Returns every request
    /// the client sent.
    async fn node(steps: Vec<Step>) -> (SocketAddr, tokio::task::JoinHandle<Vec<Message>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut seen: Vec<Message> = Vec::new();
            for step in steps {
                if let Step::PeerRequest = step {
                    let ids = seen.last().map_or(ONES, |m| m.ids.next());
                    let dwr = message(FLAG_REQUEST, DEVICE_WATCHDOG, 0, ids, &[]);
                    s.write_all(&dwr).await.unwrap();
                    continue;
                }
                let Some(request) = read_request(&mut s).await else {
                    return seen;
                };
                seen.push(request.clone());
                match step {
                    Step::AnswerAfter(delay, code) => {
                        tokio::time::sleep(delay).await;
                        s.write_all(&answer_to(&request, Some(code), false))
                            .await
                            .unwrap()
                    }
                    Step::Answer(code, error) => s
                        .write_all(&answer_to(&request, code, error))
                        .await
                        .unwrap(),
                    Step::Raw(bytes) => s.write_all(&bytes).await.unwrap(),
                    Step::Close => return seen,
                    Step::Silent => {
                        let _ = s.read(&mut [0u8; 64]).await;
                        return seen;
                    }
                    Step::PeerRequest => {}
                }
            }
            while let Some(request) = read_request(&mut s).await {
                let dpa = answer_to(&request, Some(2001), false);
                seen.push(request);
                let _ = s.write_all(&dpa).await;
            }
            seen
        });
        (addr, handle)
    }

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest {
            service: "diameter".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: Default::default(),
            timeout: Duration::from_millis(900),
        }
    }

    fn down(reason: &str) -> CheckOutcome {
        CheckOutcome::Down(reason.to_owned())
    }

    async fn run(steps: Vec<Step>) -> (CheckOutcome, Vec<Message>) {
        let (addr, node) = node(steps).await;
        let outcome = check(&req(addr), &settings(json!({}))).await;
        (outcome, node.await.unwrap())
    }

    fn commands(seen: &[Message]) -> Vec<u32> {
        seen.iter().map(|m| m.command).collect()
    }

    #[tokio::test]
    async fn diameter_outcomes() {
        use Step::*;
        let mut v2 = message(
            0,
            CAPABILITIES_EXCHANGE,
            0,
            ONES,
            &avp_u32(RESULT_CODE, 2001),
        );
        v2[0] = 2;
        let rows: Vec<(Vec<Step>, CheckOutcome)> = vec![
            (
                vec![Answer(Some(2001), false), Answer(Some(2001), false)],
                CheckOutcome::Up,
            ),
            (
                vec![Answer(Some(2002), false), Answer(Some(2001), false)],
                CheckOutcome::Up,
            ),
            (
                vec![Answer(Some(5010), true)],
                down("cea: 5010 DIAMETER_NO_COMMON_APPLICATION"),
            ),
            (
                vec![Answer(Some(2001), false), Answer(Some(3004), true)],
                down("dwa: 3004 DIAMETER_TOO_BUSY"),
            ),
            (vec![Answer(Some(4999), false)], down("cea: 4999")),
            (vec![Close], down("cea: connection closed by peer")),
            (
                vec![
                    Answer(Some(2001), false),
                    PeerRequest,
                    Answer(Some(2001), false),
                ],
                CheckOutcome::Up,
            ),
            (
                vec![Answer(None, false)],
                down("query: answer without Result-Code"),
            ),
            (vec![Raw(v2)], down("query: unsupported Diameter version 2")),
            (
                vec![Answer(Some(2001), false), Close],
                down("dwa: connection closed by peer"),
            ),
        ];
        for (i, (steps, expected)) in rows.into_iter().enumerate() {
            let (outcome, _) = run(steps).await;
            assert_eq!(outcome, expected, "row {i}");
        }
    }

    #[tokio::test]
    async fn peer_accepts_the_connection() {
        let (outcome, seen) = run(vec![
            Step::Answer(Some(2001), false),
            Step::Answer(Some(2001), false),
        ])
        .await;
        assert_eq!(outcome, CheckOutcome::Up);
        assert_eq!(
            commands(&seen),
            vec![CAPABILITIES_EXCHANGE, DEVICE_WATCHDOG, DISCONNECT_PEER]
        );
        assert_eq!(seen[2].u32_avp(DISCONNECT_CAUSE), Some(2));
        // Each request has its own Hop-by-Hop identifier.
        assert_ne!(seen[0].ids.hop_by_hop, seen[1].ids.hop_by_hop);
        assert_ne!(seen[1].ids.hop_by_hop, seen[2].ids.hop_by_hop);
    }

    #[tokio::test]
    async fn unknown_peer() {
        let (outcome, seen) = run(vec![Step::Answer(Some(3010), true)]).await;
        assert_eq!(outcome, down("cea: 3010 DIAMETER_UNKNOWN_PEER"));
        assert_eq!(commands(&seen), vec![CAPABILITIES_EXCHANGE]);
    }

    #[tokio::test]
    async fn failed_watchdog_still_disconnects() {
        let (outcome, seen) = run(vec![
            Step::Answer(Some(2001), false),
            Step::Answer(Some(3004), true),
        ])
        .await;
        assert_eq!(outcome, down("dwa: 3004 DIAMETER_TOO_BUSY"));
        assert_eq!(
            commands(&seen),
            vec![CAPABILITIES_EXCHANGE, DEVICE_WATCHDOG, DISCONNECT_PEER]
        );
    }

    #[tokio::test]
    async fn silent_peer() {
        let (addr, _node) = node(vec![Step::Silent]).await;
        let started = std::time::Instant::now();
        let outcome = check(&req(addr), &settings(json!({}))).await;
        assert_eq!(outcome, down("timeout"));
        assert!(started.elapsed() < Duration::from_millis(1500));
    }

    #[tokio::test]
    async fn cer_names_the_local_address() {
        let (outcome, seen) = run(vec![
            Step::Answer(Some(2001), false),
            Step::Answer(Some(2001), false),
        ])
        .await;
        assert_eq!(outcome, CheckOutcome::Up);
        let host_ip = seen[0]
            .avps
            .iter()
            .find(|a| a.code == HOST_IP_ADDRESS)
            .unwrap();
        assert_eq!(host_ip.data, [0, 1, 127, 0, 0, 1]);
    }

    #[tokio::test]
    async fn closed_port_is_connect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        match check(&req(addr), &settings(json!({}))).await {
            CheckOutcome::Down(r) => assert!(r.starts_with("connect: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn message_too_long() {
        let mut huge = vec![1u8, 1, 0, 4];
        huge.extend_from_slice(&[0; 16]);
        let (outcome, _) = run(vec![Step::Raw(huge)]).await;
        assert_eq!(outcome, down("query: message longer than 65536 bytes"));
    }

    async fn tls_node(name: &str) -> (SocketAddr, tempfile::TempDir) {
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
                loop {
                    let mut head = [0u8; 20];
                    if tls.read_exact(&mut head).await.is_err() {
                        break;
                    }
                    let Ok(length) = message_length(&head) else {
                        break;
                    };
                    let mut bytes = head.to_vec();
                    bytes.resize(length, 0);
                    if tls.read_exact(&mut bytes[20..]).await.is_err() {
                        break;
                    }
                    let Ok(request) = decode(&bytes) else { break };
                    let _ = tls.write_all(&answer_to(&request, Some(2001), false)).await;
                }
            }
        });
        (addr, dir)
    }

    #[tokio::test]
    async fn diameter_over_tls() {
        let (addr, dir) = tls_node("dra.example.org").await;
        let ca = dir.path().join("ca.pem");
        let opts = |name: &str| {
            settings(json!({
                "tls": true, "server_name": name, "ca_file": ca.to_str().unwrap(),
            }))
        };
        assert_eq!(
            check(&req(addr), &opts("dra.example.org")).await,
            CheckOutcome::Up
        );
        match check(&req(addr), &opts("other.example.org")).await {
            CheckOutcome::Down(r) => assert!(r.starts_with("tls: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    /// A fake node on TLS that requires a client certificate; it answers
    /// CER, DWR and DPR with 2001. Returns its address, the files (ca.pem,
    /// client.crt, client.key) and a counter of accepted connections.
    async fn mutual_tls_node() -> (
        SocketAddr,
        tempfile::TempDir,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio_rustls::TlsAcceptor;
        let pki = emfor_tls::testing::mutual_pki("dra.example.org", "emfor.example.org");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ca.pem"), &pki.ca_pem).unwrap();
        std::fs::write(dir.path().join("client.crt"), &pki.client_cert_pem).unwrap();
        std::fs::write(dir.path().join("client.key"), &pki.client_key_pem).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(pki.server_config);
        let accepted = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&accepted);
        tokio::spawn(async move {
            while let Ok((s, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let Ok(mut tls) = acceptor.accept(s).await else {
                    continue;
                };
                loop {
                    let mut head = [0u8; 20];
                    if tls.read_exact(&mut head).await.is_err() {
                        break;
                    }
                    let Ok(length) = message_length(&head) else {
                        break;
                    };
                    let mut bytes = head.to_vec();
                    bytes.resize(length, 0);
                    if tls.read_exact(&mut bytes[20..]).await.is_err() {
                        break;
                    }
                    let Ok(request) = decode(&bytes) else { break };
                    let _ = tls.write_all(&answer_to(&request, Some(2001), false)).await;
                }
            }
        });
        (addr, dir, accepted)
    }

    fn mutual_settings(dir: &Path, with_client: bool) -> Settings {
        let p = |name: &str| dir.join(name).to_str().unwrap().to_owned();
        let mut extra = json!({
            "tls": true, "server_name": "dra.example.org", "ca_file": p("ca.pem"),
        });
        if with_client {
            extra["tls_cert_file"] = json!(p("client.crt"));
            extra["tls_key_file"] = json!(p("client.key"));
        }
        settings(extra)
    }

    #[tokio::test]
    async fn peer_requires_a_client_certificate() {
        let (addr, dir, _) = mutual_tls_node().await;
        assert_eq!(
            check(&req(addr), &mutual_settings(dir.path(), true)).await,
            CheckOutcome::Up
        );
        // rustls answers a missing client certificate with an alert;
        // freeDiameter just closes (`cea: connection closed by peer`).
        match check(&req(addr), &mutual_settings(dir.path(), false)).await {
            CheckOutcome::Down(r) => assert!(r.starts_with("tls: "), "{r}"),
            other => panic!("expected down without a client certificate, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unreadable_client_key() {
        use std::sync::atomic::Ordering;
        let (addr, dir, accepted) = mutual_tls_node().await;
        let s = mutual_settings(dir.path(), true);
        std::fs::remove_file(dir.path().join("client.key")).unwrap();
        match check(&req(addr), &s).await {
            CheckOutcome::Error(r) => assert!(r.starts_with("tls_key_file: "), "{r}"),
            other => panic!("expected error, got {other:?}"),
        }
        assert_eq!(accepted.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn unanswered_dpr_does_not_delay_the_result() {
        // The DWA comes late in the budget and the DPR is never answered,
        // as the plugin runner sees it: under its own timeout.
        let (addr, node) = node(vec![
            Step::Answer(Some(2001), false),
            Step::AnswerAfter(Duration::from_millis(600), 2001),
            Step::Silent,
        ])
        .await;
        let r = req(addr);
        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(r.timeout, check(&r, &settings(json!({}))))
            .await
            .unwrap_or_else(|_| down("timeout"));
        assert_eq!(outcome, CheckOutcome::Up);
        assert!(
            started.elapsed() < Duration::from_millis(750),
            "{:?}",
            started.elapsed()
        );
        // The DPR is still sent, in the background.
        assert_eq!(
            commands(&node.await.unwrap()),
            vec![CAPABILITIES_EXCHANGE, DEVICE_WATCHDOG, DISCONNECT_PEER]
        );
    }
}
