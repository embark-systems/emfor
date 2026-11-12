//! Measure lookup latency against a running daemon's store:
//!
//! ```text
//! cargo run --release -p emfor-client --example bench -- <service> [options]
//!   --count N      lookups per measurement (default 100000)
//!   --threads T    threads for the throughput run (default 8)
//!   --dns ADDR     also time DNS A queries for <service> sent to ADDR
//!                  (for example 127.0.0.1:5300)
//! ```
//!
//! The store comes from `EMFOR_STORE` (default `/var/lib/emfor/state.mdb`),
//! as for any client. Each timed call is wrapped in two `Instant::now()`
//! reads, which add a few tens of nanoseconds to every figure.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut service = None;
    let (mut count, mut threads, mut dns) = (100_000usize, 8usize, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().unwrap_or_else(|| usage());
        match a.as_str() {
            "--count" => count = value().parse().unwrap_or_else(|_| usage()),
            "--threads" => threads = value().parse().unwrap_or_else(|_| usage()),
            "--dns" => dns = Some(value()),
            s if !s.starts_with('-') && service.is_none() => service = Some(s.to_owned()),
            _ => usage(),
        }
    }
    let service = service.unwrap_or_else(|| usage());
    if count == 0 || threads == 0 {
        usage();
    }

    // The first call opens the store.
    let start = Instant::now();
    let addr = lookup(&service);
    println!(
        "first lookup (opens the store): {:?} -> {addr}",
        start.elapsed()
    );

    let mut times = Vec::with_capacity(count);
    for _ in 0..count {
        let start = Instant::now();
        lookup(&service);
        times.push(start.elapsed());
    }
    report("lookup, 1 thread", &mut times);

    let start = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                for _ in 0..count {
                    lookup(&service);
                }
            });
        }
    });
    let total = (count * threads) as f64;
    println!(
        "lookup, {threads} threads: {:.0} lookups/s in all",
        total / start.elapsed().as_secs_f64()
    );

    if let Some(server) = dns {
        let sock = UdpSocket::bind("127.0.0.1:0").expect("bind");
        sock.connect(&server).expect("connect");
        sock.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let query = dns_query(&service);
        let mut buf = [0u8; 512];
        let mut times = Vec::with_capacity(count);
        for _ in 0..count {
            let start = Instant::now();
            sock.send(&query).expect("send");
            let n = sock.recv(&mut buf).expect("no DNS answer within 1s");
            times.push(start.elapsed());
            // RCODE 0 and at least one answer.
            if n < 12 || buf[3] & 0x0f != 0 || buf[6..8] == [0, 0] {
                eprintln!("DNS answer has no address for {service}");
                std::process::exit(1);
            }
        }
        report(&format!("DNS A query to {server}"), &mut times);
    }
}

fn lookup(service: &str) -> String {
    emfor_client::lookup(service, None).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    })
}

fn report(what: &str, times: &mut [Duration]) {
    times.sort();
    let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
    let mean = times.iter().sum::<Duration>() / times.len() as u32;
    println!(
        "{what} ({} calls): min {:?}, median {:?}, p99 {:?}, p99.9 {:?}, max {:?}, mean {:?}",
        times.len(),
        times[0],
        at(0.5),
        at(0.99),
        at(0.999),
        times[times.len() - 1],
        mean
    );
}

/// A recursion-not-desired A query for `name`.
fn dns_query(name: &str) -> Vec<u8> {
    let mut q = vec![0x45, 0x4d, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in name.trim_end_matches('.').split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]);
    q
}

fn usage() -> ! {
    eprintln!("usage: bench <service> [--count N] [--threads T] [--dns ADDR]");
    std::process::exit(2)
}
