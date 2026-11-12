//! One tokio task per backend: wait, check, feed the state machine, report.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use emfor_format::HealthState;
use rand::{RngExt, SeedableRng, rngs::StdRng};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::checker::{Checker, run_check};
use crate::config::{Backend, Settings};
use crate::state::{BackendHealth, Thresholds, Transition};
use crate::writer::WriterMsg;

/// Result of one finished check, sent to the store writer.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendUpdate {
    pub service: Arc<str>,
    /// Generation of the service layout this task was spawned for; the writer
    /// drops updates from tasks of an older generation.
    pub generation: u64,
    pub backend_id: String,
    pub state: HealthState,
    /// Unix milliseconds when the check finished.
    pub checked_at: u64,
    pub reason: Option<String>,
    pub transition: Option<Transition>,
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Delay before a backend's first check: uniform in `[0, interval)`.
pub fn first_delay(interval: Duration, rng: &mut impl RngExt) -> Duration {
    interval.mul_f64(rng.random_range(0.0..1.0))
}

/// Wait between the end of one check and the start of the next:
/// `interval` adjusted by a uniform offset within `±jitter × interval`.
pub fn next_wait(interval: Duration, jitter: f64, rng: &mut impl RngExt) -> Duration {
    if jitter <= 0.0 {
        return interval;
    }
    interval.mul_f64(1.0 + rng.random_range(-jitter..=jitter))
}

/// Spawn the check loop of one backend. Settings changes that do not reset
/// state (timing, thresholds) arrive through `settings`.
pub fn spawn_backend(
    service: Arc<str>,
    backend: Backend,
    checker: Arc<dyn Checker>,
    generation: u64,
    settings: watch::Receiver<Settings>,
    updates: mpsc::UnboundedSender<WriterMsg>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut rng = StdRng::from_rng(&mut rand::rng());
        let mut health = BackendHealth::new();
        let first = first_delay(settings.borrow().interval, &mut rng);
        tokio::time::sleep(first).await;
        loop {
            let s = *settings.borrow();
            let result = run_check(checker.as_ref(), backend.addr, s.timeout).await;
            let thresholds = Thresholds {
                rise: s.rise,
                fall: s.fall,
                interval: s.interval,
            };
            let transition = health.apply(&result, Instant::now(), thresholds);
            let reason = result.reason().map(str::to_string);
            if let Some(t) = transition {
                tracing::info!(
                    service = %service,
                    backend = %backend.id,
                    from = %t.from,
                    to = %t.to,
                    reason = reason.as_deref().unwrap_or("-"),
                    "backend {} of {} changed from {} to {} ({})",
                    backend.id, service, t.from, t.to, reason.as_deref().unwrap_or("check passed"),
                );
            }
            let update = BackendUpdate {
                service: service.clone(),
                generation,
                backend_id: backend.id.clone(),
                state: health.state,
                checked_at: unix_ms(),
                reason,
                transition,
            };
            if updates.send(WriterMsg::Update(update)).is_err() {
                return; // writer gone: daemon is shutting down
            }
            tokio::time::sleep(next_wait(s.interval, s.jitter, &mut rng)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::CheckResult;
    use futures::future::BoxFuture;
    use std::net::SocketAddr;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn wait_bounds() {
        let mut rng = StdRng::seed_from_u64(7);
        for _ in 0..10_000 {
            let w = next_wait(Duration::from_secs(2), 0.10, &mut rng);
            assert!(
                w >= Duration::from_millis(1800) && w <= Duration::from_millis(2200),
                "{w:?}"
            );
            let f = first_delay(Duration::from_secs(2), &mut rng);
            assert!(f < Duration::from_secs(2));
        }
        assert_eq!(
            next_wait(Duration::from_secs(1), 0.0, &mut rng),
            Duration::from_secs(1)
        );
    }

    /// Checker that takes `took` and records in-flight count and start times.
    #[derive(Debug)]
    struct Recording {
        took: Duration,
        result: CheckResult,
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
        starts: Mutex<Vec<Instant>>,
    }

    impl Checker for Recording {
        fn check(&self, _: SocketAddr) -> BoxFuture<'_, CheckResult> {
            Box::pin(async move {
                self.starts.lock().unwrap().push(Instant::now());
                let n = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_in_flight.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(self.took).await;
                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                self.result.clone()
            })
        }
    }

    fn recording(took: Duration, result: CheckResult) -> Arc<Recording> {
        Arc::new(Recording {
            took,
            result,
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
            starts: Mutex::new(Vec::new()),
        })
    }

    fn update(m: WriterMsg) -> BackendUpdate {
        match m {
            WriterMsg::Update(u) => u,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn backend() -> Backend {
        Backend {
            id: "10.0.0.11:8080".into(),
            addr: "10.0.0.11:8080".parse().unwrap(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn checks_never_overlap_and_wait_after_finishing() {
        let chk = recording(Duration::from_millis(900), CheckResult::Up);
        let settings = Settings {
            interval: Duration::from_secs(1),
            jitter: 0.0,
            ..Settings::default()
        };
        let (_stx, srx) = watch::channel(settings);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let h = spawn_backend("api.svc.emfor".into(), backend(), chk.clone(), 0, srx, tx);
        tokio::time::sleep(Duration::from_secs(20)).await;
        h.abort();
        assert_eq!(chk.max_in_flight.load(Ordering::SeqCst), 1);
        let starts = chk.starts.lock().unwrap().clone();
        assert!(starts.len() >= 9, "{} checks", starts.len());
        for w in starts.windows(2) {
            // 900ms check + 1s wait, no jitter.
            assert_eq!(w[1] - w[0], Duration::from_millis(1900));
        }
        let first = update(rx.recv().await.unwrap());
        assert_eq!(first.state, HealthState::Unknown);
        let second = update(rx.recv().await.unwrap());
        assert_eq!(
            second.transition,
            Some(Transition {
                from: HealthState::Unknown,
                to: HealthState::Healthy
            })
        );
    }

    /// Checker returning `up` for the first `ups` checks, then `down("connection refused")`.
    #[derive(Debug)]
    struct UpThenRefused {
        ups: AtomicUsize,
    }

    impl Checker for UpThenRefused {
        fn check(&self, _: SocketAddr) -> BoxFuture<'_, CheckResult> {
            Box::pin(async move {
                let left = self.ups.load(Ordering::SeqCst);
                if left > 0 {
                    self.ups.store(left - 1, Ordering::SeqCst);
                    CheckResult::Up
                } else {
                    CheckResult::Down("connection refused".into())
                }
            })
        }
    }

    #[tokio::test(start_paused = true)]
    #[tracing_test::traced_test]
    async fn transition_is_logged() {
        // Spec scenario: healthy -> unhealthy after `down` results with reason "connection refused".
        let chk = Arc::new(UpThenRefused {
            ups: AtomicUsize::new(2),
        });
        let (_stx, srx) = watch::channel(Settings::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let h = spawn_backend("api.svc.emfor".into(), backend(), chk, 0, srx, tx);
        tokio::time::sleep(Duration::from_secs(20)).await;
        h.abort();
        assert!(logs_contain("INFO"));
        assert!(logs_contain(
            "backend 10.0.0.11:8080 of api.svc.emfor changed from healthy to unhealthy (connection refused)"
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn new_settings_apply_without_reset() {
        let chk = recording(Duration::from_millis(1), CheckResult::Up);
        let s1 = Settings {
            interval: Duration::from_secs(1),
            jitter: 0.0,
            ..Settings::default()
        };
        let (stx, srx) = watch::channel(s1);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let h = spawn_backend("a".into(), backend(), chk.clone(), 0, srx, tx);
        tokio::time::sleep(Duration::from_secs(5)).await;
        stx.send(Settings {
            interval: Duration::from_secs(3),
            ..s1
        })
        .unwrap();
        tokio::time::sleep(Duration::from_secs(20)).await;
        h.abort();
        let mut last = None;
        while let Ok(u) = rx.try_recv() {
            last = Some(update(u));
        }
        assert_eq!(last.unwrap().state, HealthState::Healthy);
        let starts = chk.starts.lock().unwrap().clone();
        let gap = starts[starts.len() - 1] - starts[starts.len() - 2];
        assert_eq!(gap, Duration::from_millis(3001));
    }
}
