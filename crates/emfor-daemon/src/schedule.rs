//! One tokio task per backend: wait, check, feed the state machine, report.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use emfor_format::HealthState;
use rand::{RngExt, SeedableRng, rngs::StdRng};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::checker::{CheckRequest, CheckResult, Checker, run_check};
use crate::config::{Backend, Settings};
use crate::peers::{BackendDown, BackendResult, Outcome, PeerHooks, PeerOut, Recheck};
use crate::state::{BackendHealth, Thresholds, Transition, next_latency};
use crate::writer::WriterMsg;

/// Which kind of result one check had, for the check counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOutcome {
    Up,
    Down,
    Error,
}

impl CheckOutcome {
    pub fn of(result: &CheckResult) -> CheckOutcome {
        match result {
            CheckResult::Up => CheckOutcome::Up,
            CheckResult::Down(_) => CheckOutcome::Down,
            CheckResult::Error(_) => CheckOutcome::Error,
        }
    }
}

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
    pub outcome: CheckOutcome,
    /// Smoothed up-check latency in microseconds (see [`next_latency`]).
    pub latency_us: Option<u64>,
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Longest wait between checks while a backend's state is still unknown.
pub const FAST_INTERVAL: Duration = Duration::from_millis(200);

/// `min(interval, 200ms)`: the pace while a backend's state is unknown, so
/// that after a start it is known within a few hundred milliseconds rather
/// than `rise × interval`.
pub fn fast_interval(interval: Duration) -> Duration {
    interval.min(FAST_INTERVAL)
}

/// Delay before a backend's first check: uniform in `[0, fast_interval)`.
pub fn first_delay(interval: Duration, rng: &mut impl RngExt) -> Duration {
    fast_interval(interval).mul_f64(rng.random_range(0.0..1.0))
}

/// Wait after a check, given the state it left the backend in: the fast
/// pace while the state is unknown and the check said something about the
/// backend (`up` or `down`); otherwise `interval ± jitter`. An `error` is a
/// local problem (a plugin not running yet), which checking faster would not
/// fix.
pub fn wait_after(
    state: HealthState,
    result: &CheckResult,
    interval: Duration,
    jitter: f64,
    rng: &mut impl RngExt,
) -> Duration {
    if state == HealthState::Unknown && !matches!(result, CheckResult::Error(_)) {
        fast_interval(interval)
    } else {
        next_wait(interval, jitter, rng)
    }
}

/// Wait between the end of one check and the start of the next:
/// `interval` adjusted by a uniform offset within `±jitter × interval`.
pub fn next_wait(interval: Duration, jitter: f64, rng: &mut impl RngExt) -> Duration {
    if jitter <= 0.0 {
        return interval;
    }
    interval.mul_f64(1.0 + rng.random_range(-jitter..=jitter))
}

/// Whether a peer's `down` report starts a fast recheck run: the backend is
/// healthy here, no run is in progress, and the last run ended more than
/// `fall × interval` ago.
pub fn starts_fast_run(
    state: HealthState,
    in_run: bool,
    last_run_end: Option<Instant>,
    now: Instant,
    s: &Settings,
) -> bool {
    state == HealthState::Healthy
        && !in_run
        && last_run_end.is_none_or(|end| now.duration_since(end) > s.interval * s.fall)
}

/// The peer side of one backend task: reports in, failures out.
#[derive(Debug, Clone, Default)]
pub struct PeerLink {
    /// Woken when a peer reports this backend down.
    pub recheck: Arc<Recheck>,
    /// The peer group: where a healthy → unhealthy transition is announced
    /// and own results are sent, and the votes each check is combined
    /// with; `None` without `[peers]`.
    pub peers: Option<PeerHooks>,
}

/// The combined result of this daemon's own result `own` (spec peer-group,
/// "Region verdict and combined result"); when this daemon is an assigned
/// checker of the backend, its own `up` or `down` result is sent to the
/// other members.
fn combine(
    p: &PeerHooks,
    service: &Arc<str>,
    backend: &str,
    own: CheckResult,
    interval: Duration,
) -> CheckResult {
    // A service the peer task does not know yet has no digest: no kept
    // result matches, so the own result stands.
    let digest = p.targets.digest(service).unwrap_or_default();
    let c = p
        .quorum
        .combine(service, backend, &digest, &own, Instant::now());
    let outcome = match &own {
        CheckResult::Up => Some(Outcome::Up),
        CheckResult::Down(r) => Some(Outcome::Down(r.clone())),
        CheckResult::Error(_) => None,
    };
    if c.assigned
        && let Some(outcome) = outcome
    {
        // The peer task is gone only while the daemon stops.
        let _ = p.out.send(PeerOut::Result(BackendResult {
            service: service.clone(),
            backend: backend.to_string(),
            outcome,
            interval,
        }));
    }
    c.result
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
    peers: PeerLink,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut rng = StdRng::from_rng(&mut rand::rng());
        let mut health = BackendHealth::new();
        let mut latency_us: Option<u64> = None;
        // Checks left in the current fast recheck run (0: no run).
        let mut fast_left: u32 = 0;
        let mut last_run_end: Option<Instant> = None;
        let first = first_delay(settings.borrow().interval, &mut rng);
        tokio::time::sleep(first).await;
        loop {
            let s = *settings.borrow();
            let req = CheckRequest::new(service.clone(), backend.addr, s.timeout);
            // One-time setup (an HTTP client) happens here, outside the
            // timed part, so it does not inflate the first latency sample.
            checker.prepare(backend.addr);
            let started = Instant::now();
            let own = run_check(checker.as_ref(), req).await;
            let sample_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            let result = match &peers.peers {
                Some(p) => combine(p, &service, &backend.id, own, s.interval),
                None => own,
            };
            let thresholds = Thresholds {
                rise: s.rise,
                fall: s.fall,
                interval: s.interval,
            };
            let transition = health.apply(&result, Instant::now(), thresholds);
            let reason = result.reason().map(str::to_string);
            if matches!(result, CheckResult::Up) {
                tracing::debug!(
                    service = %service,
                    backend = %backend.id,
                    sample_us,
                    "check of {} {} passed in {sample_us} us",
                    service, backend.id,
                );
            }
            latency_us = next_latency(latency_us, &result, sample_us, transition);
            if fast_left > 0 {
                fast_left -= 1;
                if matches!(result, CheckResult::Up) || health.state == HealthState::Unhealthy {
                    fast_left = 0;
                }
                if fast_left == 0 {
                    last_run_end = Some(Instant::now());
                }
            }
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
                outcome: CheckOutcome::of(&result),
                latency_us,
            };
            let announce = transition
                .is_some_and(|t| t.from == HealthState::Healthy && t.to == HealthState::Unhealthy);
            if announce && let Some(p) = &peers.peers {
                // The peer task is gone only while the daemon stops.
                let _ = p.out.send(PeerOut::Down(BackendDown {
                    service: service.clone(),
                    backend: backend.id.clone(),
                    reason: update.reason.clone().unwrap_or_default(),
                }));
            }
            if updates.send(WriterMsg::Update(update)).is_err() {
                return; // writer gone: daemon is shutting down
            }
            let wait = if fast_left > 0 {
                fast_interval(s.interval)
            } else {
                wait_after(health.state, &result, s.interval, s.jitter, &mut rng)
            };
            let sleep = tokio::time::sleep(wait);
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    _ = peers.recheck.notified() => {
                        let Some(report) = peers.recheck.take() else { continue };
                        let s = *settings.borrow();
                        if starts_fast_run(health.state, fast_left > 0, last_run_end, Instant::now(), &s) {
                            tracing::info!(
                                service = %service,
                                backend = %backend.id,
                                peer = %report.from,
                                "peer {} reports backend {} of {} down ({}); checking now",
                                report.from, backend.id, service, report.reason,
                            );
                            fast_left = s.fall;
                            break;
                        }
                        tracing::debug!(
                            service = %service,
                            backend = %backend.id,
                            peer = %report.from,
                            "peer {} reports backend {} of {} down; not rechecking now (state {}, fast run {})",
                            report.from, backend.id, service, health.state,
                            if fast_left > 0 { "in progress" } else { "recently ended" },
                        );
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::CheckResult;
    use futures::future::BoxFuture;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Spec "Check scheduling with jitter and no overlap", example table.
    #[test]
    fn wait_after_a_check() {
        use HealthState::*;
        let mut rng = StdRng::seed_from_u64(7);
        let ms = Duration::from_millis;
        let up = CheckResult::Up;
        let down = CheckResult::Down("refused".into());
        let error = CheckResult::Error("plugin p unavailable".into());
        for (interval, state, result, want) in [
            (ms(2000), Unknown, &up, Some(ms(200))),
            (ms(2000), Unknown, &down, Some(ms(200))),
            (ms(2000), Unknown, &error, None),
            (ms(2000), Healthy, &up, None),
            (ms(2000), Unhealthy, &down, None),
            (ms(100), Unknown, &up, Some(ms(100))),
        ] {
            for _ in 0..100 {
                let w = wait_after(state, result, interval, 0.1, &mut rng);
                match want {
                    Some(fast) => assert_eq!(w, fast, "{state} {result:?}"),
                    None => assert!(
                        w >= interval.mul_f64(0.9) && w <= interval.mul_f64(1.1),
                        "{state} {result:?}: {w:?}"
                    ),
                }
            }
        }
    }

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
            assert!(f < Duration::from_millis(200), "{f:?}");
            let f = first_delay(Duration::from_millis(50), &mut rng);
            assert!(f < Duration::from_millis(50), "{f:?}");
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
        fn check(&self, _: CheckRequest) -> BoxFuture<'_, CheckResult> {
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
        Backend::new("10.0.0.11:8080".parse().unwrap())
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
        let h = spawn_backend(
            "api.svc.emfor".into(),
            backend(),
            chk.clone(),
            0,
            srx,
            tx,
            PeerLink::default(),
        );
        tokio::time::sleep(Duration::from_secs(20)).await;
        h.abort();
        assert_eq!(chk.max_in_flight.load(Ordering::SeqCst), 1);
        let starts = chk.starts.lock().unwrap().clone();
        assert!(starts.len() >= 9, "{} checks", starts.len());
        // The first `up` leaves the backend unknown (rise = 2): 900ms check
        // + 200ms fast wait. Healthy after the second: 900ms + 1s, no jitter.
        assert_eq!(starts[1] - starts[0], Duration::from_millis(1100));
        for w in starts[1..].windows(2) {
            assert_eq!(w[1] - w[0], Duration::from_millis(1900));
        }
        let first = update(rx.recv().await.unwrap());
        assert_eq!(first.state, HealthState::Unknown);
        assert_eq!(first.outcome, CheckOutcome::Up);
        let second = update(rx.recv().await.unwrap());
        assert_eq!(
            second.transition,
            Some(Transition {
                from: HealthState::Unknown,
                to: HealthState::Healthy
            })
        );
    }

    #[test]
    fn outcome_of_each_result() {
        assert_eq!(CheckOutcome::of(&CheckResult::Up), CheckOutcome::Up);
        assert_eq!(
            CheckOutcome::of(&CheckResult::Down("refused".into())),
            CheckOutcome::Down
        );
        assert_eq!(
            CheckOutcome::of(&CheckResult::Error("no such file".into())),
            CheckOutcome::Error
        );
    }

    /// Checker returning `up` for the first `ups` checks, then `down("connection refused")`.
    #[derive(Debug)]
    struct UpThenRefused {
        ups: AtomicUsize,
    }

    impl Checker for UpThenRefused {
        fn check(&self, _: CheckRequest) -> BoxFuture<'_, CheckResult> {
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
        let h = spawn_backend(
            "api.svc.emfor".into(),
            backend(),
            chk,
            0,
            srx,
            tx,
            PeerLink::default(),
        );
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
        let h = spawn_backend(
            "a".into(),
            backend(),
            chk.clone(),
            0,
            srx,
            tx,
            PeerLink::default(),
        );
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

    /// Spec peer-group "whether a received down message starts a fast
    /// recheck run" (interval 2s, fall 3: cooldown 6s).
    #[test]
    fn whether_a_report_starts_a_fast_run() {
        use HealthState::*;
        let s = Settings::default();
        let now = Instant::now() + Duration::from_secs(100);
        let ago = |secs: u64| Some(now - Duration::from_secs(secs));
        for (state, in_run, last_end, want) in [
            (Healthy, false, None, true),
            (Healthy, false, ago(7), true),
            (Healthy, false, ago(1), false),
            (Healthy, true, None, false),
            (Unhealthy, false, None, false),
            (Unknown, false, None, false),
        ] {
            assert_eq!(
                starts_fast_run(state, in_run, last_end, now, &s),
                want,
                "{state} in_run={in_run} {last_end:?}"
            );
        }
    }

    /// Checker that is `up` until `down` is set; each check takes 10ms.
    #[derive(Debug, Default)]
    struct Switch {
        down: std::sync::atomic::AtomicBool,
        /// Answer `error` (checked before `down`).
        error: std::sync::atomic::AtomicBool,
        starts: Mutex<Vec<Instant>>,
    }

    impl Checker for Switch {
        fn check(&self, _: CheckRequest) -> BoxFuture<'_, CheckResult> {
            Box::pin(async move {
                self.starts.lock().unwrap().push(Instant::now());
                tokio::time::sleep(Duration::from_millis(10)).await;
                if self.error.load(Ordering::SeqCst) {
                    CheckResult::Error("plugin p unavailable".into())
                } else if self.down.load(Ordering::SeqCst) {
                    CheckResult::Down("connection refused".into())
                } else {
                    CheckResult::Up
                }
            })
        }
    }

    const DIGEST: &str = "0123456789abcdef0123456789abcdef";

    fn member(i: u8) -> std::net::SocketAddr {
        format!("10.0.0.{i}:5302").parse().unwrap()
    }

    /// Peer hooks of a daemon at `me` whose service `a` has [`DIGEST`]; no
    /// member is live yet.
    fn hooks(out: mpsc::UnboundedSender<PeerOut>, me: &str) -> PeerHooks {
        let targets = crate::peers::Targets::default();
        targets.replace(std::collections::HashMap::from([(
            "a".to_string(),
            crate::peers::ServiceTargets {
                digest: DIGEST.into(),
                backends: std::collections::HashMap::new(),
            },
        )]));
        PeerHooks {
            out,
            quorum: Arc::new(crate::quorum::Quorum::new(me.parse().unwrap())),
            targets,
        }
    }

    /// A backend of service `a` checked by `chk` (interval 1s, fall 3, no
    /// jitter) at daemon 10.0.0.`me` with members .5 to .9 live, which
    /// already holds the results `kept` (by member) for it. For backend
    /// 10.0.0.11:8080 of `a` the assigned checkers are .7, .6 and .9 in
    /// that order. Runs for 5 seconds.
    async fn with_votes(
        me: u8,
        kept: &[(u8, Outcome)],
        chk: Arc<Switch>,
    ) -> (Vec<BackendUpdate>, Vec<PeerOut>) {
        let (otx, mut orx) = mpsc::unbounded_channel();
        let h = hooks(otx, &member(me).to_string());
        h.quorum.set_live((5..=9).map(member));
        for (from, outcome) in kept {
            let r = crate::peers::ResultMsg {
                service: "a".into(),
                digest: DIGEST.into(),
                backend: "10.0.0.11:8080".into(),
                outcome: outcome.clone(),
                interval_ms: 3_600_000,
            };
            h.quorum.record(member(*from), &r, Instant::now());
        }
        let settings = Settings {
            interval: Duration::from_secs(1),
            jitter: 0.0,
            ..Settings::default()
        };
        let (_stx, srx) = watch::channel(settings);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let link = PeerLink {
            recheck: Arc::new(Recheck::default()),
            peers: Some(h),
        };
        let task = spawn_backend("a".into(), backend(), chk, 0, srx, tx, link);
        tokio::time::sleep(Duration::from_secs(5)).await;
        task.abort();
        let mut updates = Vec::new();
        while let Ok(u) = rx.try_recv() {
            updates.push(update(u));
        }
        let mut out = Vec::new();
        while let Ok(o) = orx.try_recv() {
            out.push(o);
        }
        (updates, out)
    }

    /// Spec health-checking "Combined result feeds the state machine": two
    /// other assigned checkers report down, this daemon's own checks pass.
    #[tokio::test(start_paused = true)]
    async fn two_down_votes_make_an_up_backend_unhealthy() {
        let refused = || Outcome::Down("refused".into());
        let (updates, out) = with_votes(
            7,
            &[(6, refused()), (9, refused())],
            Arc::new(Switch::default()),
        )
        .await;
        let states: Vec<_> = updates.iter().map(|u| u.state).collect();
        assert_eq!(
            &states[..3],
            &[
                HealthState::Unknown,
                HealthState::Unknown,
                HealthState::Unhealthy
            ]
        );
        assert_eq!(updates[2].reason.as_deref(), Some("peers 2/3: refused"));
        assert_eq!(
            updates[2].latency_us, None,
            "a combined down is no latency sample"
        );
        // Assigned: every own result (up) goes out, nothing else.
        assert_eq!(out.len(), updates.len());
        for o in &out {
            let PeerOut::Result(r) = o else {
                panic!("only results are sent: {o:?}");
            };
            assert_eq!(
                (&*r.service, r.backend.as_str(), &r.outcome, r.interval),
                ("a", "10.0.0.11:8080", &Outcome::Up, Duration::from_secs(1))
            );
        }
    }

    /// Spec peer-group "Only one daemon sees the backend down".
    #[tokio::test(start_paused = true)]
    async fn own_down_against_up_votes_is_local() {
        let chk = Arc::new(Switch::default());
        chk.down.store(true, Ordering::SeqCst);
        let (updates, out) = with_votes(7, &[(6, Outcome::Up), (9, Outcome::Up)], chk).await;
        assert_eq!(updates[2].state, HealthState::Unhealthy);
        assert_eq!(
            updates[2].reason.as_deref(),
            Some("local: connection refused")
        );
        assert!(
            matches!(&out[0], PeerOut::Result(r) if r.outcome == Outcome::Down("connection refused".into()))
        );
    }

    /// Spec peer-group "Not an assigned checker" and "Error results are not
    /// sent".
    #[tokio::test(start_paused = true)]
    async fn results_only_from_assigned_checkers_and_not_for_errors() {
        let (updates, out) = with_votes(5, &[], Arc::new(Switch::default())).await;
        assert_eq!(updates.last().unwrap().state, HealthState::Healthy);
        assert!(out.is_empty(), "{out:?}");
        let chk = Arc::new(Switch::default());
        chk.error.store(true, Ordering::SeqCst);
        let (_updates, out) = with_votes(7, &[], chk).await;
        assert!(out.is_empty(), "{out:?}");
    }

    fn report() -> crate::peers::PeerReport {
        crate::peers::PeerReport {
            from: "10.0.0.6:5302".parse().unwrap(),
            reason: "connection refused".into(),
        }
    }

    /// Spawn a `Switch` backend with interval 5s, fall 3, jitter 10% and a
    /// peer link; returns once it is healthy and waiting.
    async fn healthy_switch() -> (
        Arc<Switch>,
        PeerLink,
        mpsc::UnboundedReceiver<PeerOut>,
        mpsc::UnboundedReceiver<WriterMsg>,
        JoinHandle<()>,
        watch::Sender<Settings>,
    ) {
        let chk = Arc::new(Switch::default());
        let settings = Settings {
            interval: Duration::from_secs(5),
            jitter: 0.10,
            ..Settings::default()
        };
        let (stx, srx) = watch::channel(settings);
        let (tx, rx) = mpsc::unbounded_channel();
        let (dtx, drx) = mpsc::unbounded_channel();
        let link = PeerLink {
            recheck: Arc::new(Recheck::default()),
            peers: Some(hooks(dtx, "10.0.0.5:5302")),
        };
        let h = spawn_backend("a".into(), backend(), chk.clone(), 0, srx, tx, link.clone());
        tokio::time::sleep(Duration::from_secs(8)).await;
        (chk, link, drx, rx, h, stx)
    }

    /// Spec health-checking "Fast recheck run after a peer report".
    #[tokio::test(start_paused = true)]
    async fn fast_recheck_run_after_a_peer_report() {
        let (chk, link, mut drx, mut rx, h, _stx) = healthy_switch().await;
        let mut last = None;
        while let Ok(u) = rx.try_recv() {
            last = Some(update(u));
        }
        assert_eq!(last.unwrap().state, HealthState::Healthy);
        let before = chk.starts.lock().unwrap().len();
        chk.down.store(true, Ordering::SeqCst);
        let at = Instant::now();
        link.recheck.send(report());
        tokio::time::sleep(Duration::from_secs(10)).await;
        h.abort();
        let starts = chk.starts.lock().unwrap()[before..].to_vec();
        assert!(starts.len() >= 4, "{starts:?}");
        assert_eq!(starts[0], at);
        assert_eq!(starts[1] - starts[0], Duration::from_millis(210));
        assert_eq!(starts[2] - starts[1], Duration::from_millis(210));
        let gap = starts[3] - starts[2] - Duration::from_millis(10);
        assert!(
            gap >= Duration::from_millis(4500) && gap <= Duration::from_millis(5500),
            "{gap:?}"
        );
        let mut states = Vec::new();
        while let Ok(u) = rx.try_recv() {
            states.push(update(u).state);
        }
        assert_eq!(
            &states[..3],
            &[
                HealthState::Healthy,
                HealthState::Healthy,
                HealthState::Unhealthy
            ]
        );
        // The healthy -> unhealthy transition is announced once.
        let Ok(PeerOut::Down(d)) = drx.try_recv() else {
            panic!("the failure is announced");
        };
        assert_eq!(
            (&*d.service, d.backend.as_str(), d.reason.as_str()),
            ("a", "10.0.0.11:8080", "connection refused")
        );
        assert!(drx.try_recv().is_err());
    }

    /// A run whose checks neither pass nor make the backend unhealthy (here
    /// every result is `error`) stops after `fall` checks.
    #[tokio::test(start_paused = true)]
    async fn run_stops_after_fall_checks() {
        let (chk, link, mut drx, _rx, h, _stx) = healthy_switch().await;
        let before = chk.starts.lock().unwrap().len();
        chk.error.store(true, Ordering::SeqCst);
        let at = Instant::now();
        link.recheck.send(report());
        tokio::time::sleep(Duration::from_secs(7)).await;
        h.abort();
        let starts = chk.starts.lock().unwrap()[before..].to_vec();
        assert!(starts.len() >= 4, "{starts:?}");
        assert_eq!(starts[0], at);
        assert_eq!(starts[1] - starts[0], Duration::from_millis(210));
        assert_eq!(starts[2] - starts[1], Duration::from_millis(210));
        let gap = starts[3] - starts[2] - Duration::from_millis(10);
        assert!(
            gap >= Duration::from_millis(4500) && gap <= Duration::from_millis(5500),
            "{gap:?}"
        );
        assert!(drx.try_recv().is_err());
    }

    /// A run stops at the first `up`; a report during the cooldown that
    /// follows is ignored.
    #[tokio::test(start_paused = true)]
    async fn run_stops_on_up_and_cooldown_ignores_reports() {
        let (chk, link, mut drx, _rx, h, _stx) = healthy_switch().await;
        let before = chk.starts.lock().unwrap().len();
        let at = Instant::now();
        link.recheck.send(report());
        tokio::time::sleep(Duration::from_secs(1)).await;
        link.recheck.send(report()); // within fall × interval = 15s
        tokio::time::sleep(Duration::from_secs(1)).await;
        h.abort();
        let starts = chk.starts.lock().unwrap()[before..].to_vec();
        assert_eq!(starts, vec![at], "one check, then the normal pace");
        assert!(drx.try_recv().is_err());
    }

    /// `unknown` -> `unhealthy` (just started) is not announced.
    #[tokio::test(start_paused = true)]
    async fn startup_failures_are_not_announced() {
        let chk = Arc::new(Switch::default());
        chk.down.store(true, Ordering::SeqCst);
        let (_stx, srx) = watch::channel(Settings::default());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (dtx, mut drx) = mpsc::unbounded_channel();
        let link = PeerLink {
            recheck: Arc::new(Recheck::default()),
            peers: Some(hooks(dtx, "10.0.0.5:5302")),
        };
        let h = spawn_backend("a".into(), backend(), chk, 0, srx, tx, link);
        tokio::time::sleep(Duration::from_secs(5)).await;
        h.abort();
        let mut last = None;
        while let Ok(u) = rx.try_recv() {
            last = Some(update(u));
        }
        assert_eq!(last.unwrap().state, HealthState::Unhealthy);
        assert!(drx.try_recv().is_err());
    }
}
