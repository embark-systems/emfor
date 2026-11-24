//! Peer group (spec peer-group): emfor daemons of one site that list each
//! other in `[peers]` exchange authenticated UDP messages. Members send a
//! heartbeat every second, and when a backend of theirs turns unhealthy they
//! tell the others, which then recheck it at once. A message never changes a
//! backend's state by itself; every daemon decides from its own checks.
//!
//! One message is one datagram whose payload is a JSON object, in one of two
//! formats. `EMP1`: the magic, a 32-byte HMAC-SHA256 tag over the magic and
//! the payload, then the payload in plaintext. `EMP2`: the magic, a 12-byte
//! random nonce, then the payload encrypted with ChaCha20-Poly1305 and its
//! 16-byte tag, with the magic as additional data and a key derived from
//! the secret with HKDF-SHA256. `EMP3`: the same with XChaCha20-Poly1305,
//! a 24-byte random nonce and a key of its own. `[peers] encrypt` picks
//! plaintext or encryption; when encrypting, each member is sent `EMP3`
//! while the latest encrypted message accepted from it is `EMP3`, or `EMP2`
//! with `"emp3": true` (which every `EMP2` this version sends has), and
//! `EMP2` otherwise. All three are accepted.
//!
//! The parts: `wire` (messages and their JSON payload), `crypto`
//! (secrets, their rotation, and the `EMP1`/`EMP2`/`EMP3` datagrams), `stats`
//! (counters read by `/metrics`), `group` (the [`Group`] state machine:
//! acceptance checks, replay state and liveness), `targets` (the backend
//! tasks peers can reach, by service digest) and `task` (the async task).

mod crypto;
#[cfg(test)]
mod crypto_bench;
mod group;
mod record;
mod stats;
mod targets;
mod task;
#[cfg(test)]
mod test_support;
mod watch;
mod wire;

pub use crypto::{
    ACCEPT_PREVIOUS_FOR, Keys, SIGN_PREVIOUS_FOR, Secret, encode, encode_encrypted,
    encode_encrypted_v3, open_with,
};
pub use group::{Group, LIVE_WINDOW, MAX_SKEW_MS, Outgoing};
pub use stats::{PeerStats, Reject};
pub use targets::{PeerReport, Recheck, ServiceTargets, Targets};
pub use task::{
    BackendDown, BackendResult, HEARTBEAT_EVERY, PeerHooks, PeerOut, PeerSender, PeerSettings,
    PeersHandle, start, start_on,
};
pub use wire::{
    Down, Format, Kind, MAGIC, MAGIC_V2, MAGIC_V3, MAX_DATAGRAM, MAX_REASON_BYTES, Message,
    Outcome, ResultMsg, truncate_utf8,
};

/// The secret of the `peers` fuzz target and its seeds
/// (`fuzz/seeds/peers.py`).
#[cfg(fuzzing)]
pub const FUZZ_SECRET: &[u8] = &[0x5a; 32];

/// Fuzzing entry point (`fuzz/fuzz_targets/peers.rs`). The first byte picks
/// what the rest is (modulo 4): 0, an `EMP1` datagram; 1, an `EMP2` or
/// `EMP3` datagram, opened with [`FUZZ_SECRET`]; 2 and 3, a payload,
/// encrypted with [`FUZZ_SECRET`] and then received as an `EMP2` (2) or
/// `EMP3` (3) datagram, so that mutations reach the JSON parsing behind the
/// decryption, which a mutated datagram almost never passes.
#[cfg(fuzzing)]
pub fn fuzz_decode(data: &[u8]) {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let datagram = match mode % 4 {
        0 => {
            let _ = wire::decode(rest);
            return;
        }
        1 => rest.to_vec(),
        2 => crypto::seal(
            rest,
            &Secret::new(FUZZ_SECRET.to_vec()),
            &chacha20poly1305::Nonce::from([0; wire::NONCE_LEN]),
        ),
        _ => crypto::seal_v3(
            rest,
            &Secret::new(FUZZ_SECRET.to_vec()),
            &chacha20poly1305::XNonce::from([0; wire::XNONCE_LEN]),
        ),
    };
    if let Some(payload) = open_with(&datagram, FUZZ_SECRET) {
        let _ = wire::parse_payload(&payload);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::open_with;
    use super::wire::{decode, parse_payload};

    /// Every seed of the `peers` fuzz target (`fuzz/seeds/peers.py`) is a
    /// valid message in the form its first byte picks, so the fuzzer starts
    /// past the tag and the decryption.
    #[test]
    fn fuzz_seeds_are_valid_messages() {
        let secret = [0x5au8; 32];
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/peers");
        let mut seen = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("fuzz/seeds/peers exists") {
            let path = entry.expect("the seed directory can be listed").path();
            let data = std::fs::read(&path).expect("a seed can be read");
            let (mode, rest) = data.split_first().expect("a seed is not empty");
            let payload = match mode {
                0 => {
                    assert!(decode(rest).is_some(), "{}", path.display());
                    open_with(rest, &secret)
                }
                1 => open_with(rest, &secret),
                _ => Some(rest.to_vec()),
            };
            assert!(
                payload.and_then(|p| parse_payload(&p)).is_some(),
                "{}",
                path.display()
            );
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                seen.push(name.to_string());
            }
        }
        for kind in ["heartbeat", "down", "result"] {
            for form in ["emp1", "emp2", "emp3", "payload", "payload-emp3"] {
                let name = format!("{kind}-{form}");
                assert!(seen.contains(&name), "seed {name} is missing");
            }
        }
    }
}
