//! Measurements of how a changed secret file is noticed: by the watch, and
//! by reading it on every heartbeat. Ignored tests, run by
//! `tools/bench-secret-watch.sh` (they take minutes and print numbers; they
//! assert nothing). Where there is no /proc only the latency is measured.

use std::sync::atomic::AtomicUsize;

use super::*;
use crate::config::tests::secret_file;
use crate::peers::test_support::group;

static READS: AtomicUsize = AtomicUsize::new(0);

fn counting(path: &Path) -> Result<Vec<u8>, String> {
    READS.fetch_add(1, Ordering::SeqCst);
    read_secret(path)
}

fn no_watch(_: &Path) -> Result<SecretWatch, String> {
    Err("measured without a watch".into())
}

fn io(watched: bool) -> SecretIo {
    SecretIo {
        read: counting,
        watch: if watched {
            SecretWatch::start
        } else {
            no_watch
        },
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

fn report(label: &str, mut ms: Vec<f64>) {
    ms.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    println!(
        "{label:<34} n={:<4} min {:>8.2}  p50 {:>8.2}  p90 {:>8.2}  p99 {:>8.2}  max {:>8.2}  (ms)",
        ms.len(),
        ms[0],
        percentile(&ms, 0.5),
        percentile(&ms, 0.9),
        percentile(&ms, 0.99),
        ms[ms.len() - 1],
    );
}

/// From the change of the file to the new secret being the group's
/// current one, with the loop of the peer task as it is (a heartbeat tick
/// every second, and the watch's events).
async fn latency(watched: bool, rename: bool, trials: usize, spread_ms: u64) -> Vec<f64> {
    use std::os::unix::fs::PermissionsExt;
    let (dir, path) = secret_file(&[1u8; 40], 0o600);
    let mut g = group(Instant::now());
    let mut source = SecretSource::new(path.clone(), io(watched));
    source.on_tick(&mut g, Instant::now()).await;
    let mut tick = tokio::time::interval(HEARTBEAT_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;
    let mut out = Vec::new();
    for n in 0..trials {
        // Letters: a secret of white space (byte 10 is a newline) is not valid.
        let key = [b'a' + (n % 26) as u8; 40];
        // A random point of the heartbeat period.
        let wait = rand::rng().random::<u64>() % spread_ms.max(1);
        tokio::time::sleep(Duration::from_millis(wait)).await;
        let t0 = std::time::Instant::now();
        if rename {
            let new = dir.path().join("secret.new");
            std::fs::write(&new, key).expect("the new file can be written");
            std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o600))
                .expect("the new file can be made private");
            std::fs::rename(&new, &path).expect("the new file can replace the old one");
        } else {
            std::fs::write(&path, key).expect("the file can be overwritten");
        }
        let settled = tokio::time::timeout(Duration::from_secs(40), async {
            while g.keys.current() != key {
                let events = source.notifier();
                tokio::select! {
                    _ = watch_event(&events) => source.on_event(&mut g, Instant::now()).await,
                    _ = tick.tick() => source.on_tick(&mut g, Instant::now()).await,
                }
            }
        })
        .await;
        assert!(
            settled.is_ok(),
            "trial {n}: the new secret was not applied within 40 s (watched {watched}, rename {rename}, notifier {})",
            source.notifier().is_some()
        );
        out.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    out
}

#[tokio::test]
#[ignore = "a benchmark; tools/bench-secret-watch.sh"]
async fn bench_latency() {
    println!();
    for (watched, rename, spread, name) in [
        (true, true, 300, "watch, rename over the file"),
        (true, false, 300, "watch, write in place"),
        (false, true, 1000, "read every second, rename"),
        (false, false, 1000, "read every second, write in place"),
    ] {
        report(name, latency(watched, rename, 50, spread).await);
    }
}

/// What reading the file costs: one read of a 40-byte file, with the
/// checks `read_secret` makes.
#[test]
#[ignore = "a benchmark; tools/bench-secret-watch.sh"]
fn bench_read_cost() {
    let (_d, path) = secret_file(&[1u8; 40], 0o600);
    for _ in 0..1000 {
        read_secret(&path).expect("warm up");
    }
    let n = 50_000;
    let t = std::time::Instant::now();
    for _ in 0..n {
        read_secret(&path).expect("a valid secret file");
    }
    let per = t.elapsed().as_secs_f64() / n as f64 * 1e6;
    println!(
        "\nread_secret: {per:.1} us per read ({n} reads); every second for a day: {:.2} s of CPU",
        per * 86_400.0 / 1e6
    );
}

#[cfg(target_os = "linux")]
fn cpu_and_switches() -> (u64, u64) {
    let stat = std::fs::read_to_string("/proc/self/stat").expect("/proc/self/stat");
    let after = &stat[stat.rfind(')').expect("a command name in parentheses") + 2..];
    let f: Vec<&str> = after.split_whitespace().collect();
    let ticks: u64 = f[11].parse::<u64>().expect("utime") + f[12].parse::<u64>().expect("stime");
    let mut switches = 0;
    for task in std::fs::read_dir("/proc/self/task")
        .expect("/proc/self/task")
        .flatten()
    {
        if let Ok(s) = std::fs::read_to_string(task.path().join("status")) {
            for l in s.lines() {
                if let Some(v) = l.strip_prefix("voluntary_ctxt_switches:") {
                    switches += v.trim().parse::<u64>().unwrap_or(0);
                }
            }
        }
    }
    (ticks * 10, switches) // clock ticks of 10 ms
}

/// Reads, CPU time and thread wake-ups over `secs` seconds of an idle
/// secret file, with the heartbeat tick of the peer loop as it is.
#[cfg(target_os = "linux")]
async fn idle(watched: bool, secs: u64) -> (usize, u64, u64) {
    let (_d, path) = secret_file(&[1u8; 40], 0o600);
    let mut g = group(Instant::now());
    let mut source = SecretSource::new(path, io(watched));
    let mut tick = tokio::time::interval(HEARTBEAT_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;
    source.on_tick(&mut g, Instant::now()).await;
    READS.store(0, Ordering::SeqCst);
    let (cpu0, sw0) = cpu_and_switches();
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < end {
        let events = source.notifier();
        tokio::select! {
            _ = watch_event(&events) => source.on_event(&mut g, Instant::now()).await,
            _ = tick.tick() => source.on_tick(&mut g, Instant::now()).await,
        }
    }
    let (cpu1, sw1) = cpu_and_switches();
    (READS.load(Ordering::SeqCst), cpu1 - cpu0, sw1 - sw0)
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "a benchmark; tools/bench-secret-watch.sh"]
async fn bench_idle() {
    let secs = 120;
    println!();
    for (watched, name) in [
        (false, "read every second"),
        (true, "watch (+ read every 30 s)"),
    ] {
        let (reads, cpu_ms, switches) = idle(watched, secs).await;
        println!(
            "idle {secs} s, {name:<26} reads {reads:<4} cpu {cpu_ms} ms (10 ms resolution)  voluntary context switches {switches}"
        );
    }
}
