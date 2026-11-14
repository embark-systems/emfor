//! Helpers shared by the tests of the peer modules.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::time::Instant;

use super::crypto::{Keys, mac};
use super::group::Group;
use super::stats::PeerStats;
use super::targets::{Recheck, ServiceTargets, Targets};
use super::wire::{Down, Kind, MAGIC, Message, Outcome, ResultMsg, parse_payload};
use hmac::Mac;

pub(super) const KEY: &[u8] = &[7u8; 32];
pub(super) const DIGEST: &str = "0123456789abcdef0123456789abcdef";
pub(super) const ME: &str = "10.0.0.5:5302";
pub(super) const B: &str = "10.0.0.6:5302";

pub(super) fn addr(s: &str) -> SocketAddr {
    s.parse()
        .expect("test addresses are valid socket addresses")
}

pub(super) fn msg(from: &str, boot: u64, seq: u64, ts: u64, kind: Kind) -> Message {
    Message {
        from: addr(from),
        site: "dc1".into(),
        boot,
        seq,
        ts,
        kind,
    }
}

pub(super) fn group(now: Instant) -> Group {
    let _ = now;
    Group::new(
        addr(ME),
        "dc1".into(),
        vec![addr(ME), addr(B)],
        Keys::new(KEY.to_vec()),
        1,
        Arc::new(PeerStats::default()),
    )
}

pub(super) fn down(reason: &str) -> Kind {
    Kind::Down(Down {
        service: "api.svc.emfor".into(),
        digest: "0123456789abcdef0123456789abcdef".into(),
        backend: "10.0.0.11:8080".into(),
        reason: reason.into(),
    })
}

pub(super) fn result(outcome: Outcome) -> Kind {
    Kind::Result(ResultMsg {
        service: "api.svc.emfor".into(),
        digest: "0123456789abcdef0123456789abcdef".into(),
        backend: "10.0.0.11:8080".into(),
        outcome,
        interval_ms: 2000,
    })
}

/// A signed datagram whose payload is `json`.
pub(super) fn raw(json: &str) -> Vec<u8> {
    let mut m = mac(KEY);
    m.update(MAGIC);
    m.update(json.as_bytes());
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&m.finalize().into_bytes());
    out.extend_from_slice(json.as_bytes());
    out
}

/// The message in an `EMP2` datagram encrypted with `key`.
pub(super) fn open_v2(data: &[u8], key: &[u8]) -> Option<Message> {
    Keys::new(key.to_vec())
        .open(data, Instant::now())
        .and_then(|p| parse_payload(&p))
}

pub(super) fn targets(digest: &str) -> (Targets, Arc<Recheck>) {
    let r = Arc::new(Recheck::default());
    let t = Targets::default();
    t.replace(HashMap::from([(
        "api.svc.emfor".to_string(),
        ServiceTargets {
            digest: digest.into(),
            backends: HashMap::from([("10.0.0.11:8080".to_string(), r.clone())]),
        },
    )]));
    (t, r)
}
