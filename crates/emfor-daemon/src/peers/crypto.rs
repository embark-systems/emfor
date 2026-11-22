//! Peer secrets and their rotation (spec peer-group "Peer secret reload"),
//! and the three datagram formats: `EMP1`, signed with HMAC-SHA256; `EMP2`,
//! encrypted with ChaCha20-Poly1305; and `EMP3`, encrypted with
//! XChaCha20-Poly1305. Each encrypted format has its own key, derived from
//! the secret with HKDF.

use std::time::Duration;

use chacha20poly1305::aead::{Aead, Generate, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tokio::time::Instant;
use zeroize::Zeroize;

use super::wire::{
    HEADER_LEN, MAGIC, MAGIC_V2, MAGIC_V3, MAX_DATAGRAM, MIN_DATAGRAM_V2, MIN_DATAGRAM_V3, Message,
    NONCE_LEN, OVERHEAD_V2, OVERHEAD_V3, XNONCE_LEN, payload_for,
};

const HKDF_SALT: &[u8] = b"emfor peers";
const HKDF_INFO: &[u8] = b"EMP2 chacha20-poly1305";
const HKDF_INFO_V3: &[u8] = b"EMP3 xchacha20-poly1305";

/// How long messages are still signed with a replaced secret, so members
/// whose file has not changed yet keep accepting them.
pub const SIGN_PREVIOUS_FOR: Duration = Duration::from_secs(60);
/// How long a replaced secret is still accepted: twice
/// [`SIGN_PREVIOUS_FOR`], so that when every member changes its file within
/// that time, no message between any two of them is dropped.
pub const ACCEPT_PREVIOUS_FOR: Duration = Duration::from_secs(120);

/// A peer secret and the `EMP2` and `EMP3` keys derived from it. The bytes
/// are overwritten when the value is dropped, which includes a secret that
/// was replaced and has outlived [`ACCEPT_PREVIOUS_FOR`]; each clone is wiped
/// when it is dropped. The cipher states built from the keys for a datagram
/// (`cipher`, `cipher_v3`) are not.
#[derive(Clone)]
pub struct Secret {
    raw: Vec<u8>,
    aead: [u8; 32],
    aead_v3: [u8; 32],
}

impl Secret {
    /// Overwrite the secret and the keys with zeros. The secret is empty
    /// afterwards.
    fn wipe(&mut self) {
        self.raw.zeroize();
        self.aead.zeroize();
        self.aead_v3.zeroize();
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.wipe();
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Secret {
    pub fn new(raw: Vec<u8>) -> Secret {
        let hkdf = Hkdf::<Sha256>::new(Some(HKDF_SALT), &raw);
        let key = |info: &[u8]| {
            let mut key = [0u8; 32];
            hkdf.expand(info, &mut key)
                .expect("32 bytes is far below HKDF-SHA256's limit of 255 * 32 bytes of output");
            key
        };
        Secret {
            aead: key(HKDF_INFO),
            aead_v3: key(HKDF_INFO_V3),
            raw,
        }
    }

    /// The secret as read from the file.
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    pub(super) fn cipher(&self) -> ChaCha20Poly1305 {
        ChaCha20Poly1305::new(&self.aead.into())
    }

    pub(super) fn cipher_v3(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(&self.aead_v3.into())
    }
}

pub(super) fn mac(key: &[u8]) -> Hmac<Sha256> {
    Hmac::<Sha256>::new_from_slice(key)
        .expect("HMAC accepts keys of any length (RFC 2104), so new_from_slice never fails")
}

/// The `EMP1` datagram for `msg`, signed with `key`. A reason (of a `down`
/// or `result` message) is cut to [`MAX_REASON_BYTES`], and further when
/// the datagram would otherwise exceed [`MAX_DATAGRAM`].
pub fn encode(msg: &Message, key: &[u8]) -> Vec<u8> {
    let payload = payload_for(msg, HEADER_LEN);
    let mut m = mac(key);
    m.update(MAGIC);
    m.update(&payload);
    let tag = m.finalize().into_bytes();
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&tag);
    out.extend_from_slice(&payload);
    out
}

/// The `EMP2` datagram for `msg`, encrypted with the key of `secret` and a
/// nonce from the operating system's random generator. Reasons are cut as
/// for [`encode`].
pub fn encode_encrypted(msg: &Message, secret: &Secret) -> Vec<u8> {
    seal(&payload_for(msg, OVERHEAD_V2), secret, &Nonce::generate())
}

/// `EMP2`, the nonce, then `payload` encrypted under `nonce`.
pub(super) fn seal(payload: &[u8], secret: &Secret, nonce: &Nonce) -> Vec<u8> {
    let sealed = secret
        .cipher()
        .encrypt(
            nonce,
            Payload {
                msg: payload,
                aad: MAGIC_V2,
            },
        )
        .expect(
            "ChaCha20-Poly1305 encrypts any payload below 256 GiB, and ours is at most 1200 bytes",
        );
    let mut out = Vec::with_capacity(MAGIC_V2.len() + NONCE_LEN + sealed.len());
    out.extend_from_slice(MAGIC_V2);
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&sealed);
    out
}

/// The `EMP3` datagram for `msg`, encrypted with the `EMP3` key of
/// `secret` and a nonce from the operating system's random generator.
/// Reasons are cut as for [`encode`].
pub fn encode_encrypted_v3(msg: &Message, secret: &Secret) -> Vec<u8> {
    seal_v3(&payload_for(msg, OVERHEAD_V3), secret, &XNonce::generate())
}

/// `EMP3`, the nonce, then `payload` encrypted under `nonce`.
pub(super) fn seal_v3(payload: &[u8], secret: &Secret, nonce: &XNonce) -> Vec<u8> {
    let sealed = secret
        .cipher_v3()
        .encrypt(
            nonce,
            Payload {
                msg: payload,
                aad: MAGIC_V3,
            },
        )
        .expect(
            "XChaCha20-Poly1305 encrypts any payload below 256 GiB, and ours is at most 1200 bytes",
        );
    let mut out = Vec::with_capacity(MAGIC_V3.len() + XNONCE_LEN + sealed.len());
    out.extend_from_slice(MAGIC_V3);
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&sealed);
    out
}

/// The JSON payload of a datagram of any format that `secret` signed
/// (`EMP1`) or encrypted (`EMP2`, `EMP3`); `None` otherwise. For tests and
/// tools that read what members send.
pub fn open_with(data: &[u8], secret: &[u8]) -> Option<Vec<u8>> {
    let keys = Keys::new(secret.to_vec());
    let now = Instant::now();
    match data.get(..4) {
        Some(m) if m == MAGIC && (HEADER_LEN..=MAX_DATAGRAM).contains(&data.len()) => keys
            .verifies(&data[HEADER_LEN..], &data[MAGIC.len()..HEADER_LEN], now)
            .then(|| data[HEADER_LEN..].to_vec()),
        Some(m) if m == MAGIC_V2 && (MIN_DATAGRAM_V2..=MAX_DATAGRAM).contains(&data.len()) => {
            keys.open(data, now)
        }
        Some(m) if m == MAGIC_V3 && (MIN_DATAGRAM_V3..=MAX_DATAGRAM).contains(&data.len()) => {
            keys.open_v3(data, now)
        }
        _ => None,
    }
}

/// The current secret and the ones it replaced: messages are signed with
/// the replaced one for [`SIGN_PREVIOUS_FOR`] and it is accepted for
/// [`ACCEPT_PREVIOUS_FOR`] after the change.
#[derive(Debug, Clone)]
pub struct Keys {
    current: Secret,
    /// The secret signed with until [`SIGN_PREVIOUS_FOR`] after the time
    /// given, and accepted until [`ACCEPT_PREVIOUS_FOR`] after it.
    pub(super) previous: Option<(Secret, Instant)>,
    /// Other secrets replaced, with when: each is accepted until
    /// [`ACCEPT_PREVIOUS_FOR`] after it was replaced.
    replaced: Vec<(Secret, Instant)>,
    /// Where a change of the current secret is recorded, when the daemon
    /// keeps a record (see [`super::record`]).
    record: Option<std::sync::Arc<super::record::RecordFile>>,
}

impl Keys {
    pub fn new(current: Vec<u8>) -> Keys {
        Keys {
            current: Secret::new(current),
            previous: None,
            replaced: Vec::new(),
            record: None,
        }
    }

    /// Record every later change of the current secret in `record`.
    pub(super) fn with_record(mut self, record: std::sync::Arc<super::record::RecordFile>) -> Keys {
        self.record = Some(record);
        self
    }

    /// The secrets at start (spec peer-group "Peer secret reload"): when
    /// `previous` (from `previous_secret_file`) differs from `current` and
    /// the current secret's file changed `changed_ago` before `now`, less
    /// than [`ACCEPT_PREVIOUS_FOR`] ago, `previous` is used as if the
    /// daemon had been running when the file changed: signed with until
    /// [`SIGN_PREVIOUS_FOR`] after the change, accepted until
    /// [`ACCEPT_PREVIOUS_FOR`] after it. Otherwise only `current` is used.
    pub fn resumed(
        current: Vec<u8>,
        previous: Option<Vec<u8>>,
        changed_ago: Duration,
        now: Instant,
    ) -> Keys {
        let mut keys = Keys::new(current);
        if let Some(previous) = previous
            && previous != keys.current.raw
            && changed_ago < ACCEPT_PREVIOUS_FOR
        {
            // An Instant cannot go back before the clock's origin (shortly
            // after the host booted); the change then counts as now.
            let at = now.checked_sub(changed_ago).unwrap_or(now);
            keys.previous = Some((Secret::new(previous), at));
        }
        keys
    }

    /// The newest secret read from the file.
    pub fn current(&self) -> &[u8] {
        self.current.raw()
    }

    /// The secret to sign or encrypt with at `now`.
    pub fn signing(&self, now: Instant) -> &Secret {
        match &self.previous {
            Some((k, at)) if now.duration_since(*at) < SIGN_PREVIOUS_FOR => k,
            _ => &self.current,
        }
    }

    /// Make `new` the current secret. Returns false when it is the current
    /// one already. While messages are still signed with an earlier secret
    /// (within [`SIGN_PREVIOUS_FOR`] of the last change), they stay signed
    /// with it, now for [`SIGN_PREVIOUS_FOR`] after this change: the
    /// secret this change replaces was never signed with, so members whose
    /// file has not changed yet do not know it.
    pub fn rotate(&mut self, new: Vec<u8>, now: Instant) -> bool {
        if new == self.current.raw {
            return false;
        }
        let old = std::mem::replace(&mut self.current, Secret::new(new));
        if let Some(record) = &self.record {
            record.replaced(&old.raw, &self.current.raw, super::record::unix_now());
        }
        self.replaced
            .retain(|(_, at)| now.duration_since(*at) < ACCEPT_PREVIOUS_FOR);
        match &mut self.previous {
            Some((_, at)) if now.duration_since(*at) < SIGN_PREVIOUS_FOR => {
                *at = now;
                self.replaced.push((old, now));
            }
            previous => {
                if let Some(earlier) = previous.replace((old, now))
                    && now.duration_since(earlier.1) < ACCEPT_PREVIOUS_FOR
                {
                    self.replaced.push(earlier);
                }
            }
        }
        true
    }

    /// The secrets a received message may use at `now`: the current one,
    /// and each replaced one for [`ACCEPT_PREVIOUS_FOR`].
    pub(super) fn accepted(&self, now: Instant) -> impl Iterator<Item = &Secret> {
        let recent =
            move |(_, at): &&(Secret, Instant)| now.duration_since(*at) < ACCEPT_PREVIOUS_FOR;
        std::iter::once(&self.current)
            .chain(self.previous.iter().filter(recent).map(|(k, _)| k))
            .chain(self.replaced.iter().filter(recent).map(|(k, _)| k))
    }

    /// Constant-time check of `tag` over `MAGIC` and `payload`.
    pub(super) fn verifies(&self, payload: &[u8], tag: &[u8], now: Instant) -> bool {
        self.accepted(now).any(|k| {
            let mut m = mac(k.raw());
            m.update(MAGIC);
            m.update(payload);
            m.verify_slice(tag).is_ok()
        })
    }

    /// The payload of an `EMP2` datagram of a valid length, or `None` when
    /// no accepted secret decrypts it.
    pub(super) fn open(&self, data: &[u8], now: Instant) -> Option<Vec<u8>> {
        let nonce = Nonce::try_from(&data[MAGIC_V2.len()..MAGIC_V2.len() + NONCE_LEN]).ok()?;
        let sealed = &data[MAGIC_V2.len() + NONCE_LEN..];
        self.accepted(now).find_map(|k| {
            k.cipher()
                .decrypt(
                    &nonce,
                    Payload {
                        msg: sealed,
                        aad: MAGIC_V2,
                    },
                )
                .ok()
        })
    }

    /// The payload of an `EMP3` datagram of a valid length, or `None` when
    /// no accepted secret decrypts it.
    pub(super) fn open_v3(&self, data: &[u8], now: Instant) -> Option<Vec<u8>> {
        let nonce = XNonce::try_from(&data[MAGIC_V3.len()..MAGIC_V3.len() + XNONCE_LEN]).ok()?;
        let sealed = &data[MAGIC_V3.len() + XNONCE_LEN..];
        self.accepted(now).find_map(|k| {
            k.cipher_v3()
                .decrypt(
                    &nonce,
                    Payload {
                        msg: sealed,
                        aad: MAGIC_V3,
                    },
                )
                .ok()
        })
    }
}

#[cfg(test)]
mod tests {
    /// A dropped secret is overwritten: `wipe`, which `drop` calls, leaves
    /// no byte of the secret or of the keys derived from it.
    #[test]
    fn wipe_clears_the_secret_and_its_keys() {
        let mut s = Secret::new(vec![7u8; 40]);
        assert!(s.aead != [0u8; 32] && s.aead_v3 != [0u8; 32]);
        let copy = s.clone();
        s.wipe();
        assert!(s.raw().is_empty());
        assert_eq!(s.aead, [0u8; 32]);
        assert_eq!(s.aead_v3, [0u8; 32]);
        // A clone is its own value: wiping one leaves the other.
        assert_eq!(copy.raw(), &[7u8; 40][..]);
    }

    use super::*;
    use crate::peers::group::Group;
    use crate::peers::stats::{PeerStats, Reject};
    use crate::peers::test_support::*;
    use crate::peers::wire::{Kind, Outcome};
    use std::sync::Arc;

    /// Spec peer-group "Encrypted heartbeat": every kind survives the trip,
    /// and the payload is not readable on the wire.
    #[test]
    fn encrypted_round_trip() {
        let secret = Secret::new(KEY.to_vec());
        for kind in [
            Kind::Heartbeat(vec![1, 0xdeadbeef]),
            down("refused"),
            result(Outcome::Up),
            Kind::Unknown("assign".into()),
        ] {
            let m = msg(B, 0xabc, 7, 1_763_000_000_000, kind);
            let data = encode_encrypted(&m, &secret);
            assert_eq!(&data[..4], MAGIC_V2);
            assert!(!data.windows(3).any(|w| w == b"dc1"), "site in plaintext");
            assert_eq!(open_v2(&data, KEY), Some(m));
        }
        // A new nonce for every message.
        let m = msg(B, 1, 1, 1, Kind::Heartbeat(Vec::new()));
        assert_ne!(encode_encrypted(&m, &secret), encode_encrypted(&m, &secret));
    }

    /// `EMP3`: every kind survives the trip, the payload is not readable
    /// on the wire, and every message has a new nonce.
    #[test]
    fn emp3_round_trip() {
        let secret = Secret::new(KEY.to_vec());
        assert_ne!(secret.aead, secret.aead_v3, "EMP2 and EMP3 keys differ");
        for kind in [
            Kind::Heartbeat(vec![1, 0xdeadbeef]),
            down("refused"),
            result(Outcome::Up),
            Kind::Unknown("assign".into()),
        ] {
            let m = msg(B, 0xabc, 7, 1_763_000_000_000, kind);
            let data = encode_encrypted_v3(&m, &secret);
            assert_eq!(&data[..4], MAGIC_V3);
            assert!(!data.windows(3).any(|w| w == b"dc1"), "site in plaintext");
            assert_eq!(open_v3(&data, KEY), Some(m));
        }
        let m = msg(B, 1, 1, 1, Kind::Heartbeat(Vec::new()));
        assert_ne!(
            encode_encrypted_v3(&m, &secret),
            encode_encrypted_v3(&m, &secret)
        );
    }

    /// `EMP3`: changing any byte, the magic (the AAD) and the nonce
    /// included, makes the datagram not decrypt; and the `EMP2` key does
    /// not open it.
    #[test]
    fn emp3_tampered_does_not_open() {
        let secret = Secret::new(KEY.to_vec());
        let data = encode_encrypted_v3(&msg(B, 1, 1, 1, Kind::Heartbeat(Vec::new())), &secret);
        assert!(open_with(&data, KEY).is_some());
        for i in 0..data.len() {
            let mut d = data.clone();
            d[i] ^= 1;
            assert!(open_with(&d, KEY).is_none(), "byte {i} changed");
        }
        let mut as_v2 = data.clone();
        as_v2[3] = b'2';
        assert!(open_with(&as_v2, KEY).is_none(), "read as EMP2");
        // Sealed without the magic as AAD.
        let nonce = XNonce::from([5; XNONCE_LEN]);
        let sealed = secret
            .cipher_v3()
            .encrypt(&nonce, br#"{"from":"10.0.0.6:5302"}"#.as_ref())
            .expect("encrypting a short payload works");
        let no_aad = [MAGIC_V3.as_slice(), nonce.as_slice(), &sealed].concat();
        assert!(open_with(&no_aad, KEY).is_none(), "no AAD");
    }

    /// `EMP3` is 46 to 1200 bytes: a valid datagram of 46 or 1200 bytes
    /// opens, one of 45 or 1201 bytes does not.
    #[test]
    fn emp3_length_bounds() {
        let secret = Secret::new(KEY.to_vec());
        let nonce = XNonce::from([3; XNONCE_LEN]);
        let sized = |n: usize| seal_v3(&vec![b' '; n - OVERHEAD_V3], &secret, &nonce);
        for n in [MIN_DATAGRAM_V3, MAX_DATAGRAM] {
            let d = sized(n);
            assert_eq!(d.len(), n);
            assert!(open_with(&d, KEY).is_some(), "{n} bytes");
        }
        assert_eq!(MIN_DATAGRAM_V3, 46);
        let d = sized(MAX_DATAGRAM + 1);
        assert_eq!(d.len(), 1201);
        assert!(open_with(&d, KEY).is_none(), "1201 bytes");
        // 45 bytes: a 1-byte payload, which no valid message has.
        let d = seal_v3(b" ", &secret, &nonce);
        assert_eq!(d.len(), 45);
        assert!(open_with(&d, KEY).is_none(), "45 bytes");
    }

    /// The replaced secret decrypts `EMP3` for 120 seconds.
    // spectra-scenario: peer-group / Sender restarts / acceptance checks
    #[test]
    fn previous_secret_decrypts_emp3_for_120_seconds() {
        let start = Instant::now();
        let mut keys = Keys::new(KEY.to_vec());
        assert!(keys.rotate(vec![9u8; 32], start));
        let old = encode_encrypted_v3(
            &msg(B, 1, 1, 1, Kind::Heartbeat(Vec::new())),
            &Secret::new(KEY.to_vec()),
        );
        for secs in [30, 90] {
            let at = start + Duration::from_secs(secs);
            assert!(keys.open_v3(&old, at).is_some(), "{secs}s");
        }
        assert!(
            keys.open_v3(&old, start + Duration::from_secs(130))
                .is_none()
        );
    }

    /// `EMP2` bytes for a fixed secret, nonce and payload, computed apart
    /// from this code by `crates/emfor-daemon/tests/emp2_kat.py`: a
    /// plain-Python HKDF (RFC 5869) and ChaCha20-Poly1305 (RFC 8439), itself
    /// checked against the RFCs' test vectors:
    /// key = HKDF-SHA256(salt "emfor peers", ikm 32 bytes of 7,
    /// info "EMP2 chacha20-poly1305"), nonce 01..0c, AAD "EMP2".
    #[test]
    fn encrypted_known_answer() {
        let secret = Secret::new(KEY.to_vec());
        assert_eq!(
            hex(&secret.aead),
            "1a8cfa78ce5c9f17376137c9bca6b8c67423e5ac4e5d4f0b23dde76d2e0d6e5c"
        );
        let payload = br#"{"from":"10.0.0.6:5302","site":"dc1","boot":"0000000000000001","seq":1,"ts":1763000000000,"kind":"heartbeat"}"#;
        let nonce = Nonce::from([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        let data = seal(payload, &secret, &nonce);
        assert_eq!(
            hex(&data),
            "454d50320102030405060708090a0b0c39106b94d7eceaad0107bea6d91c09c5e1a12774471db156617651d0910ffdb4d8a720346df809e69b5c6de9736f8985eccdfe454f6199f8fe5a600538f728575a067ca2018869b008b196b0526fd0b74119683f48abc64bfef0e2b589b740fa74add1e9a84795dc9ff89f18f5110df7d5f677fcab9f39121be20ab015"
        );
        let mut g = group(Instant::now());
        assert!(
            g.accept(&data, addr(B), 1_763_000_000_000, Instant::now())
                .is_ok()
        );
    }

    /// `EMP3` bytes for a fixed secret, nonce and payload, computed apart
    /// from this code by `crates/emfor-daemon/tests/emp2_kat.py`: a
    /// plain-Python HKDF, HChaCha20 and XChaCha20-Poly1305, checked against
    /// the test vectors of draft-irtf-cfrg-xchacha-03 (2.2.1 and A.3.1):
    /// key = HKDF-SHA256(salt "emfor peers", ikm 32 bytes of 7,
    /// info "EMP3 xchacha20-poly1305"), nonce 01..18, AAD "EMP3".
    #[test]
    fn emp3_known_answer() {
        let secret = Secret::new(KEY.to_vec());
        assert_eq!(
            hex(&secret.aead_v3),
            "780b1054085e43cae11b4b51d331cbfacc511926f3e97d0a4dc95fdea0396a48"
        );
        let payload = br#"{"from":"10.0.0.6:5302","site":"dc1","boot":"0000000000000001","seq":1,"ts":1763000000000,"kind":"heartbeat"}"#;
        let mut nonce = [0u8; XNONCE_LEN];
        for (i, b) in nonce.iter_mut().enumerate() {
            *b = i as u8 + 1;
        }
        let data = seal_v3(payload, &secret, &XNonce::from(nonce));
        assert_eq!(
            hex(&data),
            "454d50330102030405060708090a0b0c0d0e0f1011121314151617183c2d23cdef44cbd6f26312bce8878f12b1b1de854524875c2c223dd4c15895ca067ea94197a9f3854c8cb52c5a745e248366dda966af381056ddc1d6693c8009fc503aa3133f7c72a7b6ed7dace76b55ba70c167f9a9e07620b530398bda988097c8d4b185e0f1684c37d890f0fd71e086d7ef66ff0d42074c7113c506"
        );
        assert_eq!(open_with(&data, KEY).as_deref(), Some(payload.as_slice()));
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// The replaced secret is accepted for 120 seconds in `EMP2` too.
    // spectra-scenario: peer-group / Sender restarts / acceptance checks
    #[test]
    fn previous_secret_decrypts_for_120_seconds() {
        let start = Instant::now();
        let t = 1_763_000_000_000u64;
        let mut g = group(start);
        assert!(g.keys.rotate(vec![9u8; 32], start));
        let old = Secret::new(KEY.to_vec());
        let hb =
            |seq, s: &Secret| encode_encrypted(&msg(B, 1, seq, t, Kind::Heartbeat(Vec::new())), s);
        for (secs, seq) in [(30, 1), (90, 2)] {
            let at = start + Duration::from_secs(secs);
            assert!(g.accept(&hb(seq, &old), addr(B), t, at).is_ok(), "{secs}s");
        }
        let at130 = start + Duration::from_secs(130);
        assert_eq!(
            g.accept(&hb(3, &old), addr(B), t, at130).map(|_| ()),
            Err(Reject::Auth)
        );
        assert!(
            g.accept(&hb(4, &Secret::new(vec![9u8; 32])), addr(B), t, at130)
                .is_ok()
        );
    }

    /// Spec peer-group "acceptance checks", rows on a replaced secret.
    // spectra-scenario: peer-group / Sender restarts / acceptance checks
    #[test]
    fn previous_secret_accepted_for_120_seconds() {
        let start = Instant::now();
        let t = 1_763_000_000_000u64;
        let mut g = group(start);
        assert!(g.keys.rotate(vec![9u8; 32], start));
        assert!(!g.keys.rotate(vec![9u8; 32], start));
        let old = |seq| encode(&msg(B, 1, seq, t, Kind::Heartbeat(Vec::new())), KEY);
        for (secs, seq) in [(30, 1), (90, 2)] {
            let at = start + Duration::from_secs(secs);
            assert!(g.accept(&old(seq), addr(B), t, at).is_ok(), "{secs}s");
        }
        let at130 = start + Duration::from_secs(130);
        assert_eq!(
            g.accept(&old(3), addr(B), t, at130).map(|_| ()),
            Err(Reject::Auth)
        );
        let new = encode(&msg(B, 1, 4, t, Kind::Heartbeat(Vec::new())), &[9u8; 32]);
        assert!(g.accept(&new, addr(B), t, at130).is_ok());
    }

    /// Spec peer-group "Secret rotated on one host first": A's file changes
    /// first, B's 40 seconds later; every message both ways is accepted.
    // spectra-scenario: peer-group / Secret rotated on one host first
    #[test]
    fn secret_rotated_on_one_host_first() {
        let start = Instant::now();
        let t = 1_763_000_000_000u64;
        let stats = || Arc::new(PeerStats::default());
        let members = vec![addr(ME), addr(B)];
        let mut a = Group::new(
            addr(ME),
            "dc1".into(),
            members.clone(),
            Keys::new(KEY.to_vec()),
            1,
            stats(),
        );
        let mut b = Group::new(
            addr(B),
            "dc1".into(),
            members,
            Keys::new(KEY.to_vec()),
            2,
            stats(),
        );
        let new = vec![9u8; 32];
        for secs in (0..=200).step_by(5) {
            let now = start + Duration::from_secs(secs);
            if secs == 0 {
                a.keys.rotate(new.clone(), now);
            }
            if secs == 40 {
                b.keys.rotate(new.clone(), now);
            }
            let from_a = a.encode_one(Kind::Heartbeat(Vec::new()), t, now);
            assert!(
                b.accept(&from_a, addr(ME), t, now).is_ok(),
                "A to B at {secs}s"
            );
            let from_b = b.encode_one(Kind::Heartbeat(Vec::new()), t, now);
            assert!(
                a.accept(&from_b, addr(B), t, now).is_ok(),
                "B to A at {secs}s"
            );
        }
        // Signing switches to the new secret 60 seconds after the change.
        let mine = a.encode_one(
            Kind::Heartbeat(Vec::new()),
            t,
            start + Duration::from_secs(59),
        );
        assert_eq!(&mine[..4], MAGIC_V3, "A and B moved each other to EMP3");
        assert!(open_with(&mine, KEY).is_some());
        let mine = a.encode_one(
            Kind::Heartbeat(Vec::new()),
            t,
            start + Duration::from_secs(60),
        );
        assert!(open_with(&mine, &new).is_some());
    }

    /// Spec peer-group "Secret changed twice": a second change while the
    /// daemon still signs with the first secret keeps signing with the
    /// first one, for 60 seconds after the second change, and every
    /// secret replaced in the last 120 seconds is accepted.
    // spectra-scenario: peer-group / Secret changed twice
    #[test]
    fn second_rotation_keeps_signing_with_the_first_secret() {
        let start = Instant::now();
        let at = |s| start + Duration::from_secs(s);
        let (k1, k2) = (vec![8u8; 32], vec![9u8; 32]);
        let mut keys = Keys::new(KEY.to_vec());
        assert!(keys.rotate(k1.clone(), at(0)));
        assert!(keys.rotate(k2.clone(), at(30)));
        assert_eq!(keys.current(), &k2[..]);
        for s in [31, 89] {
            assert_eq!(keys.signing(at(s)).raw(), KEY, "{s}s");
        }
        assert_eq!(keys.signing(at(90)).raw(), &k2[..]);
        let accepted = |keys: &Keys, s| {
            keys.accepted(at(s))
                .map(|k| k.raw().to_vec())
                .collect::<Vec<_>>()
        };
        for want in [KEY.to_vec(), k1.clone(), k2.clone()] {
            assert!(accepted(&keys, 119).contains(&want), "{want:?} at 119s");
        }
        // Both replaced secrets are accepted until 120s after the second
        // change.
        assert_eq!(
            accepted(&keys, 149),
            vec![k2.clone(), KEY.to_vec(), k1.clone()]
        );
        assert_eq!(accepted(&keys, 150), vec![k2.clone()]);
        // A change after signing switched: the secret signed with last
        // is the one kept.
        let mut keys = Keys::new(KEY.to_vec());
        keys.rotate(k1.clone(), at(0));
        keys.rotate(k2.clone(), at(70));
        assert_eq!(keys.signing(at(71)).raw(), &k1[..]);
        assert!(accepted(&keys, 110).contains(&KEY.to_vec()));
        assert!(!accepted(&keys, 121).contains(&KEY.to_vec()));
    }
}
