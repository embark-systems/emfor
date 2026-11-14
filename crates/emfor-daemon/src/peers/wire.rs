//! Peer messages and their JSON payload, and the framing of the datagrams
//! that carry them (spec peer-group "Peer message format").

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// First four bytes of every message: magic and format version 1.
pub const MAGIC: &[u8; 4] = b"EMP1";
pub(super) const TAG_LEN: usize = 32;
pub(super) const HEADER_LEN: usize = MAGIC.len() + TAG_LEN;
/// First four bytes of an encrypted message: magic and format version 2.
pub const MAGIC_V2: &[u8; 4] = b"EMP2";
pub(super) const NONCE_LEN: usize = 12;
const AEAD_TAG_LEN: usize = 16;
/// Bytes of an `EMP2` datagram besides the payload: magic, nonce, tag.
pub(super) const OVERHEAD_V2: usize = MAGIC_V2.len() + NONCE_LEN + AEAD_TAG_LEN;
/// Shortest `EMP2` datagram: the overhead and a payload of `{}`.
pub(super) const MIN_DATAGRAM_V2: usize = OVERHEAD_V2 + 2;

/// Largest datagram sent or accepted; small enough not to be fragmented.
pub const MAX_DATAGRAM: usize = 1200;
/// Longest `reason` of a `down` message, in bytes of UTF-8.
pub const MAX_REASON_BYTES: usize = 256;

/// One peer message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub from: SocketAddr,
    pub site: String,
    pub boot: u64,
    pub seq: u64,
    /// Unix milliseconds when sent.
    pub ts: u64,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// With the service tags the sender leaves out of the peer group
    /// (`off`), sorted; empty when it leaves none out.
    Heartbeat(Vec<u32>),
    Down(Down),
    Result(ResultMsg),
    /// A kind this version does not know (a newer member): it counts for
    /// liveness only.
    Unknown(String),
}

/// A backend of `service` turned unhealthy at the sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Down {
    pub service: String,
    pub digest: String,
    pub backend: String,
    pub reason: String,
}

/// An assigned checker's own result of one check of a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultMsg {
    pub service: String,
    pub digest: String,
    pub backend: String,
    pub outcome: Outcome,
    /// The sender's `interval` of the service, in milliseconds; positive.
    pub interval_ms: u64,
}

/// A vote: what an assigned checker's own check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Up,
    /// Down, with the check's reason.
    Down(String),
}

/// The datagram format of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `EMP1`: authenticated, payload in plaintext.
    Plain,
    /// `EMP2`: payload encrypted.
    Encrypted,
}

/// The JSON payload as sent.
#[derive(Serialize)]
struct Wire {
    from: String,
    site: String,
    boot: String,
    seq: u64,
    ts: u64,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interval_ms: Option<u64>,
    /// Service tags of a heartbeat, as 8 lowercase hex digits each.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    off: Option<Vec<String>>,
}

/// The longest prefix of `s` of at most `max` bytes that ends on a
/// character boundary.
pub fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The JSON payload for `msg`, short enough that `overhead` more bytes fit
/// in [`MAX_DATAGRAM`] (by cutting the reason when there is one).
pub(super) fn payload_for(msg: &Message, overhead: usize) -> Vec<u8> {
    let mut wire = Wire {
        from: msg.from.to_string(),
        site: msg.site.clone(),
        boot: format!("{:016x}", msg.boot),
        seq: msg.seq,
        ts: msg.ts,
        kind: String::new(),
        service: None,
        digest: None,
        backend: None,
        reason: None,
        outcome: None,
        interval_ms: None,
        off: None,
    };
    match &msg.kind {
        Kind::Heartbeat(tags) => {
            wire.kind = "heartbeat".into();
            if !tags.is_empty() {
                wire.off = Some(tags.iter().map(|t| format!("{t:08x}")).collect());
            }
        }
        Kind::Unknown(k) => wire.kind = k.clone(),
        Kind::Down(d) => {
            wire.kind = "down".into();
            wire.service = Some(d.service.clone());
            wire.digest = Some(d.digest.clone());
            wire.backend = Some(d.backend.clone());
            wire.reason = Some(truncate_utf8(&d.reason, MAX_REASON_BYTES).to_string());
        }
        Kind::Result(r) => {
            wire.kind = "result".into();
            wire.service = Some(r.service.clone());
            wire.digest = Some(r.digest.clone());
            wire.backend = Some(r.backend.clone());
            wire.interval_ms = Some(r.interval_ms);
            match &r.outcome {
                Outcome::Up => wire.outcome = Some("up".into()),
                Outcome::Down(reason) => {
                    wire.outcome = Some("down".into());
                    wire.reason = Some(truncate_utf8(reason, MAX_REASON_BYTES).to_string());
                }
            }
        }
    }
    let mut payload = serde_json::to_vec(&wire)
        .expect("a struct of strings and integers always serializes to JSON");
    // Names at their longest and a reason of escaped characters can still
    // overflow one datagram: shorten the reason until it fits. Each round
    // removes at least as many bytes as the payload is over.
    while overhead + payload.len() > MAX_DATAGRAM
        && let Some(reason) = wire.reason.as_mut()
        && !reason.is_empty()
    {
        let over = overhead + payload.len() - MAX_DATAGRAM;
        let keep = truncate_utf8(reason, reason.len().saturating_sub(over)).len();
        reason.truncate(keep);
        payload = serde_json::to_vec(&wire)
            .expect("a struct of strings and integers always serializes to JSON");
    }
    payload
}

/// A service tag written as exactly 8 lowercase hex digits.
fn parse_tag(s: &str) -> Option<u32> {
    let ok = s.len() == 8 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    ok.then(|| u32::from_str_radix(s, 16).ok()).flatten()
}

/// Parse an `EMP1` datagram without checking its tag; `None` when it is
/// malformed. For tests and fuzzing: a receiver checks the tag first.
#[cfg(any(test, fuzzing))]
pub(super) fn decode(data: &[u8]) -> Option<Message> {
    if data.len() < HEADER_LEN || data.len() > MAX_DATAGRAM || &data[..4] != MAGIC {
        return None;
    }
    parse_payload(&data[HEADER_LEN..])
}

/// The JSON payload as received: the common fields with their types, and
/// the fields of the known kinds as any JSON value, checked only for the
/// kind that uses them, so that a newer kind may give one of these names
/// another type.
#[derive(Deserialize)]
struct WireIn {
    from: String,
    site: String,
    boot: String,
    seq: u64,
    ts: u64,
    kind: String,
    #[serde(default)]
    service: Option<serde_json::Value>,
    #[serde(default)]
    digest: Option<serde_json::Value>,
    #[serde(default)]
    backend: Option<serde_json::Value>,
    #[serde(default)]
    reason: Option<serde_json::Value>,
    #[serde(default)]
    outcome: Option<serde_json::Value>,
    #[serde(default)]
    interval_ms: Option<serde_json::Value>,
    #[serde(default)]
    off: Option<serde_json::Value>,
}

/// A string field of a known kind; `None` when absent or not a string.
fn string(v: Option<serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::String(s) => Some(s),
        _ => None,
    }
}

/// Parse a JSON payload; `None` when it is malformed.
pub(super) fn parse_payload(payload: &[u8]) -> Option<Message> {
    let wire: WireIn = serde_json::from_slice(payload).ok()?;
    if wire.boot.len() != 16 || !wire.boot.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let kind = match wire.kind.as_str() {
        "heartbeat" => Kind::Heartbeat(match wire.off {
            None => Vec::new(),
            Some(serde_json::Value::Array(off)) => off
                .iter()
                .map(|v| v.as_str().and_then(parse_tag))
                .collect::<Option<Vec<u32>>>()?,
            Some(_) => return None,
        }),
        "down" => Kind::Down(Down {
            service: string(wire.service)?,
            digest: string(wire.digest)?,
            backend: string(wire.backend)?,
            reason: string(wire.reason)?,
        }),
        "result" => Kind::Result(ResultMsg {
            service: string(wire.service)?,
            digest: string(wire.digest)?,
            backend: string(wire.backend)?,
            outcome: match string(wire.outcome)?.as_str() {
                "up" => Outcome::Up,
                "down" => Outcome::Down(string(wire.reason)?),
                _ => return None,
            },
            interval_ms: wire
                .interval_ms
                .as_ref()
                .and_then(serde_json::Value::as_u64)
                .filter(|ms| *ms > 0)?,
        }),
        other => Kind::Unknown(other.to_string()),
    };
    Some(Message {
        from: wire.from.parse().ok()?,
        site: wire.site,
        boot: u64::from_str_radix(&wire.boot, 16).ok()?,
        seq: wire.seq,
        ts: wire.ts,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peers::crypto::{Secret, encode, encode_encrypted, open_with};
    use crate::peers::test_support::*;

    #[test]
    fn encode_then_decode() {
        for kind in [
            Kind::Heartbeat(Vec::new()),
            down("connection refused"),
            result(Outcome::Up),
            result(Outcome::Down("connect: Connection refused".into())),
        ] {
            let m = msg(B, 0xaaaa_aaaa_aaaa_aaaa, 42, 1_763_000_000_123, kind);
            let data = encode(&m, KEY);
            assert_eq!(&data[..4], MAGIC);
            assert_eq!(decode(&data), Some(m));
        }
        let m = msg("[2001:db8::1]:5302", 1, 1, 1, Kind::Heartbeat(Vec::new()));
        assert_eq!(decode(&encode(&m, KEY)), Some(m));
    }

    #[test]
    fn reason_is_cut_at_a_character_boundary() {
        // 255 ASCII bytes then a 3-byte character: 258 bytes.
        let reason = format!("{}€", "x".repeat(255));
        let data = encode(&msg(B, 1, 1, 1, down(&reason)), KEY);
        let Some(Message {
            kind: Kind::Down(d),
            ..
        }) = decode(&data)
        else {
            panic!("a down message decodes as down");
        };
        assert_eq!(d.reason, "x".repeat(255));
        assert_eq!(truncate_utf8("ab€", 4), "ab");
        assert_eq!(truncate_utf8("ab€", 5), "ab€");
    }

    #[test]
    fn largest_message_fits_one_datagram() {
        let service = format!("{}.svc", "a".repeat(248));
        let backend = "[ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff]:65535".to_string();
        // Quotes double when escaped in JSON.
        let reason = "\"".repeat(400);
        let kinds = [
            Kind::Down(Down {
                service: service.clone(),
                digest: "f".repeat(32),
                backend: backend.clone(),
                reason: reason.clone(),
            }),
            Kind::Result(ResultMsg {
                service,
                digest: "f".repeat(32),
                backend,
                outcome: Outcome::Down(reason),
                interval_ms: u64::MAX,
            }),
        ];
        for kind in kinds {
            let mut m = msg(
                "[ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff]:65535",
                u64::MAX,
                u64::MAX,
                u64::MAX,
                kind,
            );
            m.site = "s".repeat(63);
            let data = encode(&m, KEY);
            assert!(data.len() <= MAX_DATAGRAM, "{} bytes", data.len());
            assert!(
                decode(&data).is_some(),
                "the shortened message still decodes"
            );
            let secret = Secret::new(KEY.to_vec());
            let data = encode_encrypted(&m, &secret);
            assert!(data.len() <= MAX_DATAGRAM, "EMP2: {} bytes", data.len());
            assert!(
                open_with(&data, KEY)
                    .and_then(|p| parse_payload(&p))
                    .is_some(),
                "the shortened EMP2 message still decrypts and parses"
            );
        }
    }

    /// Spec peer-group "Heartbeat announces services left out": tags are
    /// written as 8 lowercase hex digits, and no `off` without tags.
    #[test]
    fn heartbeat_off_on_the_wire() {
        let json = |kind| {
            let data = encode(&msg(B, 1, 1, 1, kind), KEY);
            String::from_utf8(data[HEADER_LEN..].to_vec()).expect("the payload is UTF-8 JSON")
        };
        assert!(!json(Kind::Heartbeat(Vec::new())).contains("off"));
        assert!(
            json(Kind::Heartbeat(vec![0x0000_00ff, 0xa513_a047]))
                .ends_with(r#""kind":"heartbeat","off":["000000ff","a513a047"]}"#)
        );
    }

    /// Spec peer-group "More services left out than fit": the largest
    /// heartbeat, with [`crate::quorum::MAX_OFF_TAGS`] tags, fits one
    /// datagram.
    #[test]
    fn largest_heartbeat_fits_one_datagram() {
        let from = SocketAddr::V6(std::net::SocketAddrV6::new(
            "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"
                .parse()
                .expect("a valid IPv6 address"),
            65535,
            0,
            u32::MAX,
        ));
        let tags = vec![u32::MAX; crate::quorum::MAX_OFF_TAGS];
        let m = Message {
            from,
            site: "s".repeat(63),
            boot: u64::MAX,
            seq: u64::MAX,
            ts: u64::MAX,
            kind: Kind::Heartbeat(tags),
        };
        let data = encode(&m, KEY);
        assert!(data.len() <= MAX_DATAGRAM, "{} bytes", data.len());
        assert_eq!(decode(&data), Some(m.clone()));
        let data = encode_encrypted(&m, &Secret::new(KEY.to_vec()));
        assert!(data.len() <= MAX_DATAGRAM, "EMP2: {} bytes", data.len());
        assert_eq!(open_v2(&data, KEY), Some(m));
    }
}
