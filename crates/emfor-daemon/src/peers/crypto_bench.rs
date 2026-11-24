//! How long it takes to seal and open a datagram in each format. Ignored
//! test, run by `tools/bench-peer-crypto.sh` (prints numbers; asserts
//! nothing).

use std::hint::black_box;
use std::time::Instant;

use super::crypto::*;
use super::test_support::*;
use super::wire::{Kind, Message, Outcome};

const ROUNDS: usize = 200_000;

fn per_op(label: &str, size: usize, mut f: impl FnMut()) {
    for _ in 0..ROUNDS / 10 {
        f();
    }
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        for _ in 0..ROUNDS {
            f();
        }
        best = best.min(t.elapsed().as_nanos() as f64 / ROUNDS as f64);
    }
    println!(
        "{label:<28} {size:>4} B  {best:>8.0} ns/op  {:>9.0} op/s",
        1e9 / best
    );
}

fn run(name: &str, m: &Message) {
    println!("--- {name}");
    let secret = Secret::new(KEY.to_vec());
    let d1 = encode(m, KEY);
    let d2 = encode_encrypted(m, &secret);
    let d3 = encode_encrypted_v3(m, &secret);
    let d4 = encode_encrypted_v4(m, &secret, 1);
    per_op("EMP1 sign", d1.len(), || {
        black_box(encode(black_box(m), KEY));
    });
    per_op("EMP2 seal", d2.len(), || {
        black_box(encode_encrypted(black_box(m), &secret));
    });
    per_op("EMP3 seal", d3.len(), || {
        black_box(encode_encrypted_v3(black_box(m), &secret));
    });
    let mut c = 1u64;
    per_op("EMP4 seal", d4.len(), || {
        c += 1;
        black_box(encode_encrypted_v4(black_box(m), &secret, c));
    });
    per_op("EMP1 open", d1.len(), || {
        black_box(open_with(black_box(&d1), KEY));
    });
    per_op("EMP2 open", d2.len(), || {
        black_box(open_with(black_box(&d2), KEY));
    });
    per_op("EMP3 open", d3.len(), || {
        black_box(open_with(black_box(&d3), KEY));
    });
    per_op("EMP4 open", d4.len(), || {
        black_box(open_with(black_box(&d4), KEY));
    });
    per_op("EMP3 open, wrong secret", d3.len(), || {
        black_box(open_with(black_box(&d3), &[9u8; 32]));
    });
    per_op("EMP4 open, wrong secret", d4.len(), || {
        black_box(open_with(black_box(&d4), &[9u8; 32]));
    });
}

#[test]
#[ignore = "a measurement; run by tools/bench-peer-crypto.sh"]
fn seal_and_open_by_format() {
    run(
        "heartbeat",
        &msg(ME, 1, 1, 1_700_000_000_000, Kind::Heartbeat(vec![])),
    );
    run(
        "result",
        &msg(ME, 1, 1, 1_700_000_000_000, result(Outcome::Up)),
    );
    run(
        "down with a long reason",
        &msg(ME, 1, 1, 1_700_000_000_000, down(&"x".repeat(240))),
    );
}
