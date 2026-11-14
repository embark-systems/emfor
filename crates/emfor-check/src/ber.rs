//! The few LDAPv3 messages (RFC 4511) the `ldap` plugin sends and reads, in
//! BER with definite lengths: BindRequest, the StartTLS ExtendedRequest and
//! UnbindRequest out; BindResponse and ExtendedResponse in.

/// The longest LDAPMessage the plugin reads, header included.
pub const MAX_MESSAGE: usize = 65536;

/// requestName of the StartTLS extended operation (RFC 4511 4.14.1).
pub const STARTTLS_OID: &str = "1.3.6.1.4.1.1466.20037";

/// protocolOp tags.
pub const BIND_RESPONSE: u8 = 0x61;
pub const EXTENDED_RESPONSE: u8 = 0x78;

const SEQUENCE: u8 = 0x30;
const INTEGER: u8 = 0x02;
const OCTET_STRING: u8 = 0x04;
const ENUMERATED: u8 = 0x0a;
const BIND_REQUEST: u8 = 0x60;
const UNBIND_REQUEST: u8 = 0x42;
const EXTENDED_REQUEST: u8 = 0x77;
/// `[0]` primitive: the simple password of a bind, or an extended
/// request's name.
const CONTEXT_0: u8 = 0x80;

/// A message that does not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BerError(pub String);

fn err<T>(message: impl Into<String>) -> Result<T, BerError> {
    Err(BerError(message.into()))
}

/// What the plugin needs from a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub message_id: i64,
    /// protocolOp tag, e.g. [`BIND_RESPONSE`].
    pub op: u8,
    pub result_code: i64,
    pub diagnostic: String,
}

/// LDAPMessage with a BindRequest: version 3, simple authentication.
/// An empty `name` and `password` is an anonymous bind.
pub fn bind_request(id: i32, name: &str, password: &str) -> Vec<u8> {
    let mut bind = tlv(INTEGER, &integer(3));
    bind.extend(tlv(OCTET_STRING, name.as_bytes()));
    bind.extend(tlv(CONTEXT_0, password.as_bytes()));
    message(id, &tlv(BIND_REQUEST, &bind))
}

/// LDAPMessage with the StartTLS ExtendedRequest.
pub fn starttls_request(id: i32) -> Vec<u8> {
    message(
        id,
        &tlv(EXTENDED_REQUEST, &tlv(CONTEXT_0, STARTTLS_OID.as_bytes())),
    )
}

/// LDAPMessage with an UnbindRequest.
pub fn unbind_request(id: i32) -> Vec<u8> {
    message(id, &tlv(UNBIND_REQUEST, &[]))
}

fn message(id: i32, op: &[u8]) -> Vec<u8> {
    let mut content = tlv(INTEGER, &integer(id.into()));
    content.extend_from_slice(op);
    tlv(SEQUENCE, &content)
}

fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = content.len();
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let skip = bytes.iter().take_while(|b| **b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(content);
    out
}

/// Minimal two's-complement encoding.
fn integer(n: i64) -> Vec<u8> {
    let bytes = n.to_be_bytes();
    let mut start = 0;
    while start < 7 {
        let (b, next) = (bytes[start], bytes[start + 1]);
        if (b == 0x00 && next & 0x80 == 0) || (b == 0xff && next & 0x80 != 0) {
            start += 1;
        } else {
            break;
        }
    }
    bytes[start..].to_vec()
}

/// The header of an LDAPMessage at the start of `buf`: how many header
/// bytes it has and how long its content is. `Ok(None)` when `buf` does not
/// hold the whole header yet.
pub fn message_header(buf: &[u8]) -> Result<Option<(usize, usize)>, BerError> {
    let Some(&tag) = buf.first() else {
        return Ok(None);
    };
    if tag != SEQUENCE {
        return err(format!("not an LDAP message (tag 0x{tag:02x})"));
    }
    let Some((header, len)) = length(&buf[1..])? else {
        return Ok(None);
    };
    let header = 1 + header;
    if header
        .checked_add(len)
        .is_none_or(|total| total > MAX_MESSAGE)
    {
        return err(format!("response longer than {MAX_MESSAGE} bytes"));
    }
    Ok(Some((header, len)))
}

/// A definite length at the start of `buf`: (bytes used, value).
fn length(buf: &[u8]) -> Result<Option<(usize, usize)>, BerError> {
    let Some(&first) = buf.first() else {
        return Ok(None);
    };
    if first < 0x80 {
        return Ok(Some((1, first.into())));
    }
    let n = usize::from(first & 0x7f);
    if n == 0 {
        return err("indefinite length");
    }
    if n > 4 {
        return err("length too long");
    }
    let Some(bytes) = buf.get(1..1 + n) else {
        return Ok(None);
    };
    let value = bytes.iter().fold(0usize, |v, b| (v << 8) | usize::from(*b));
    Ok(Some((1 + n, value)))
}

/// Reads elements from a complete buffer.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    /// The next element: (tag, content).
    fn next(&mut self) -> Result<(u8, &'a [u8]), BerError> {
        let Some(&tag) = self.0.first() else {
            return err("message ends early");
        };
        let Some((used, len)) = length(&self.0[1..])? else {
            return err("message ends early");
        };
        let start = 1 + used;
        let Some(content) = self.0.get(start..start + len) else {
            return err("message ends early");
        };
        self.0 = &self.0[start + len..];
        Ok((tag, content))
    }

    fn expect(&mut self, tag: u8, what: &str) -> Result<&'a [u8], BerError> {
        match self.next()? {
            (t, content) if t == tag => Ok(content),
            (t, _) => err(format!("expected {what}, got tag 0x{t:02x}")),
        }
    }
}

fn to_integer(content: &[u8], what: &str) -> Result<i64, BerError> {
    if content.is_empty() || content.len() > 8 {
        return err(format!("bad {what}"));
    }
    let negative = content[0] & 0x80 != 0;
    let init: i64 = if negative { -1 } else { 0 };
    Ok(content.iter().fold(init, |v, b| (v << 8) | i64::from(*b)))
}

/// Parse a whole LDAPMessage holding an LDAPResult-shaped response
/// (BindResponse, ExtendedResponse, ...). Fields after
/// diagnosticMessage are ignored.
pub fn decode_response(msg: &[u8]) -> Result<Response, BerError> {
    let mut outer = Reader(msg);
    let body = outer.expect(SEQUENCE, "an LDAP message")?;
    let mut body = Reader(body);
    let message_id = to_integer(body.expect(INTEGER, "a messageID")?, "messageID")?;
    let (op, result) = body.next()?;
    if op & 0x20 == 0 {
        return err(format!("unexpected operation (tag 0x{op:02x})"));
    }
    let mut result = Reader(result);
    let result_code = to_integer(result.expect(ENUMERATED, "a resultCode")?, "resultCode")?;
    result.expect(OCTET_STRING, "a matchedDN")?;
    let diagnostic = result.expect(OCTET_STRING, "a diagnosticMessage")?;
    // Active Directory ends its diagnosticMessage with a NUL byte, as a C
    // string; it is not part of the message.
    let end = diagnostic
        .iter()
        .rposition(|b| *b != 0)
        .map_or(0, |last| last + 1);
    Ok(Response {
        message_id,
        op,
        result_code,
        diagnostic: String::from_utf8_lossy(&diagnostic[..end]).into_owned(),
    })
}

/// `<code> <name>` with the RFC 4511 name when it has one, then
/// `: <diagnosticMessage>` when the server sent one.
pub fn result_text(code: i64, diagnostic: &str) -> String {
    let mut text = match result_name(code) {
        Some(name) => format!("{code} {name}"),
        None => code.to_string(),
    };
    if !diagnostic.is_empty() {
        text.push_str(": ");
        text.push_str(diagnostic);
    }
    text
}

/// resultCode names from RFC 4511 section 4.1.9.
fn result_name(code: i64) -> Option<&'static str> {
    Some(match code {
        0 => "success",
        1 => "operationsError",
        2 => "protocolError",
        3 => "timeLimitExceeded",
        4 => "sizeLimitExceeded",
        5 => "compareFalse",
        6 => "compareTrue",
        7 => "authMethodNotSupported",
        8 => "strongerAuthRequired",
        10 => "referral",
        11 => "adminLimitExceeded",
        12 => "unavailableCriticalExtension",
        13 => "confidentialityRequired",
        14 => "saslBindInProgress",
        16 => "noSuchAttribute",
        17 => "undefinedAttributeType",
        18 => "inappropriateMatching",
        19 => "constraintViolation",
        20 => "attributeOrValueExists",
        21 => "invalidAttributeSyntax",
        32 => "noSuchObject",
        33 => "aliasProblem",
        34 => "invalidDNSyntax",
        36 => "aliasDereferencingProblem",
        48 => "inappropriateAuthentication",
        49 => "invalidCredentials",
        50 => "insufficientAccessRights",
        51 => "busy",
        52 => "unavailable",
        53 => "unwillingToPerform",
        54 => "loopDetect",
        64 => "namingViolation",
        65 => "objectClassViolation",
        66 => "notAllowedOnNonLeaf",
        67 => "notAllowedOnRDN",
        68 => "entryAlreadyExists",
        69 => "objectClassModsProhibited",
        71 => "affectsMultipleDSAs",
        80 => "other",
        _ => return None,
    })
}

/// An LDAPResult-shaped response, for tests and fake servers.
#[cfg(test)]
pub fn response(id: i32, op: u8, code: i64, diagnostic: &str) -> Vec<u8> {
    let mut result = tlv(ENUMERATED, &integer(code));
    result.extend(tlv(OCTET_STRING, b""));
    result.extend(tlv(OCTET_STRING, diagnostic.as_bytes()));
    message(id, &tlv(op, &result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn anonymous_bind_bytes() {
        assert_eq!(
            hex(&bind_request(1, "", "")),
            "30 0c 02 01 01 60 07 02 01 03 04 00 80 00"
        );
    }

    #[test]
    fn bind_with_name_and_password_bytes() {
        assert_eq!(
            hex(&bind_request(2, "cn=a", "pw")),
            "30 12 02 01 02 60 0d 02 01 03 04 04 63 6e 3d 61 80 02 70 77"
        );
    }

    #[test]
    fn starttls_and_unbind_bytes() {
        let mut want = "30 1d 02 01 01 77 18 80 16".to_string();
        for b in STARTTLS_OID.bytes() {
            want.push_str(&format!(" {b:02x}"));
        }
        assert_eq!(hex(&starttls_request(1)), want);
        assert_eq!(hex(&unbind_request(2)), "30 05 02 01 02 42 00");
    }

    #[test]
    fn long_name_uses_long_form_length() {
        let name = "a".repeat(300);
        let msg = bind_request(1, &name, "");
        // 300 = 0x012c: two length bytes.
        assert!(hex(&msg).contains("04 82 01 2c"), "{}", hex(&msg));
        assert_eq!(message_header(&msg), Ok(Some((4, msg.len() - 4))));
    }

    #[test]
    fn integers_are_minimal() {
        assert_eq!(integer(0), [0]);
        assert_eq!(integer(127), [0x7f]);
        assert_eq!(integer(128), [0x00, 0x80]);
        assert_eq!(integer(4096), [0x10, 0x00]);
        assert_eq!(integer(-1), [0xff]);
    }

    #[test]
    fn responses_decode() {
        let r = decode_response(&response(1, BIND_RESPONSE, 49, "")).unwrap();
        assert_eq!(
            r,
            Response {
                message_id: 1,
                op: BIND_RESPONSE,
                result_code: 49,
                diagnostic: String::new()
            }
        );
        let r = decode_response(&response(1, EXTENDED_RESPONSE, 4096, "x")).unwrap();
        assert_eq!(
            (r.op, r.result_code, r.diagnostic.as_str()),
            (EXTENDED_RESPONSE, 4096, "x")
        );
    }

    #[test]
    fn trailing_nul_bytes_are_not_part_of_the_diagnostic() {
        // As Active Directory sends it (Windows Server, invalid password).
        let ad = "80090308: LdapErr: DSID-0C090434, comment: AcceptSecurityContext error, data 52e, v4f7c\0";
        for (sent, want) in [
            (ad, &ad[..ad.len() - 1]),
            ("x\0\0", "x"),
            ("\0", ""),
            ("a\0b", "a\0b"),
        ] {
            let r = decode_response(&response(1, BIND_RESPONSE, 49, sent)).unwrap();
            assert_eq!(r.diagnostic, want, "{sent:?}");
        }
        assert_eq!(
            result_text(
                49,
                &decode_response(&response(1, BIND_RESPONSE, 49, "\0"))
                    .unwrap()
                    .diagnostic
            ),
            "49 invalidCredentials"
        );
    }

    #[test]
    fn headers() {
        assert_eq!(message_header(&[]), Ok(None));
        assert_eq!(message_header(&[0x30]), Ok(None));
        assert_eq!(message_header(&[0x30, 0x82, 0x01]), Ok(None));
        assert_eq!(message_header(&[0x30, 0x05]), Ok(Some((2, 5))));
        assert_eq!(
            message_header(&[0x30, 0x80]),
            Err(BerError("indefinite length".into()))
        );
        assert_eq!(
            message_header(&[0x30, 0x83, 0x01, 0x00, 0x00]),
            Err(BerError("response longer than 65536 bytes".into()))
        );
        assert_eq!(
            message_header(&[0x30, 0x85, 0, 0, 0, 0, 1]),
            Err(BerError("length too long".into()))
        );
        assert!(message_header(b"HTTP/1.1").is_err());
    }

    #[test]
    fn truncated_or_wrong_messages_are_errors() {
        let full = response(1, BIND_RESPONSE, 0, "");
        assert!(decode_response(&full[..full.len() - 1]).is_err());
        // An UnbindRequest is not a response.
        assert!(decode_response(&unbind_request(1)).is_err());
    }

    #[test]
    fn result_texts() {
        assert_eq!(result_text(49, ""), "49 invalidCredentials");
        assert_eq!(
            result_text(53, "unauthenticated bind (DN with no password) disallowed"),
            "53 unwillingToPerform: unauthenticated bind (DN with no password) disallowed"
        );
        assert_eq!(result_text(8, ""), "8 strongerAuthRequired");
        assert_eq!(result_text(4096, ""), "4096");
        assert_eq!(result_text(2, ""), "2 protocolError");
    }
}
