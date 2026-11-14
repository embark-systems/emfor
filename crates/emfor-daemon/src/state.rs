//! Per-backend health state machine with haproxy-style `rise` / `fall`.

use std::time::Duration;

use emfor_format::{BackendRecord, HealthState};
use tokio::time::Instant;

use crate::checker::CheckResult;

/// Thresholds the state machine needs from the service settings.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    pub rise: u32,
    pub fall: u32,
    pub interval: Duration,
}

/// A health state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub from: HealthState,
    pub to: HealthState,
}

#[derive(Debug, Clone)]
pub struct BackendHealth {
    pub state: HealthState,
    pub successes: u32,
    pub failures: u32,
    /// Start of the current run of `error`-only results.
    error_since: Option<Instant>,
}

impl Default for BackendHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendHealth {
    /// Every backend starts `unknown` with both counters at zero.
    pub fn new() -> Self {
        Self {
            state: HealthState::Unknown,
            successes: 0,
            failures: 0,
            error_since: None,
        }
    }

    /// Feed one check result. Returns the transition when the state changed.
    pub fn apply(
        &mut self,
        result: &CheckResult,
        now: Instant,
        t: Thresholds,
    ) -> Option<Transition> {
        let from = self.state;
        match result {
            CheckResult::Up => {
                self.error_since = None;
                self.failures = 0;
                self.successes = self.successes.saturating_add(1);
                if self.successes >= t.rise {
                    self.state = HealthState::Healthy;
                }
            }
            CheckResult::Down(_) => {
                self.error_since = None;
                self.successes = 0;
                self.failures = self.failures.saturating_add(1);
                if self.failures >= t.fall {
                    self.state = HealthState::Unhealthy;
                }
            }
            CheckResult::Error(_) => {
                let since = *self.error_since.get_or_insert(now);
                if now.duration_since(since) > t.interval * t.fall {
                    self.state = HealthState::Unknown;
                    self.successes = 0;
                    self.failures = 0;
                }
            }
        }
        (self.state != from).then_some(Transition {
            from,
            to: self.state,
        })
    }
}

/// New smoothed check latency: the sample itself when there is none yet,
/// otherwise `round(0.05 * sample + 0.95 * previous)`, in microseconds.
pub fn smooth_latency(previous: Option<u64>, sample_us: u64) -> u64 {
    match previous {
        None => sample_us,
        Some(p) => (sample_us + 19 * p + 10) / 20,
    }
}

/// Latency after one check: an `up` result updates it; the backend turning
/// unhealthy clears it; anything else keeps it.
pub fn next_latency(
    previous: Option<u64>,
    result: &CheckResult,
    sample_us: u64,
    transition: Option<Transition>,
) -> Option<u64> {
    if matches!(transition, Some(t) if t.to == HealthState::Unhealthy) {
        None
    } else if matches!(result, CheckResult::Up) {
        Some(smooth_latency(previous, sample_us))
    } else {
        previous
    }
}

/// Whether a backend in scope is fast, given the lowest latency in scope and
/// whether it was fast before: it enters at `max(min * 3 / 2, min + 1000)`
/// and leaves above `max(min * 2, min + 2000)` (hysteresis).
pub fn is_fast(latency_us: u64, min_us: u64, fast_before: bool) -> bool {
    let limit = if fast_before {
        min_us.saturating_mul(2).max(min_us.saturating_add(2000))
    } else {
        (min_us.saturating_mul(3) / 2).max(min_us.saturating_add(1000))
    };
    latency_us <= limit
}

/// Pool of a `fastest` service: ids of its fast backends in config order.
/// `previous` is the pool before this recomputation. Only healthy backends
/// with a latency are in scope; the backend with the lowest latency is
/// always fast, so the pool is empty exactly when nothing is in scope.
pub fn fast_pool(backends: &[BackendRecord], previous: &[String]) -> Vec<String> {
    let in_scope = |b: &BackendRecord| {
        if b.state == HealthState::Healthy {
            b.latency_us
        } else {
            None
        }
    };
    let Some(min) = backends.iter().filter_map(in_scope).min() else {
        return Vec::new();
    };
    backends
        .iter()
        .filter(|b| in_scope(b).is_some_and(|l| is_fast(l, min, previous.contains(&b.id))))
        .map(|b| b.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use HealthState::*;

    const T: Thresholds = Thresholds {
        rise: 2,
        fall: 3,
        interval: Duration::from_secs(2),
    };

    fn parse(s: &str) -> CheckResult {
        match s {
            "up" => CheckResult::Up,
            "down" => CheckResult::Down("x".into()),
            "error" => CheckResult::Error("x".into()),
            _ => unreachable!(),
        }
    }

    #[test]
    fn transitions_table() {
        // Rows of the "state transitions with rise = 2, fall = 3" example.
        let rows: &[(&str, HealthState)] = &[
            ("up", Unknown),
            ("up,up", Healthy),
            ("up,up,down,down", Healthy),
            ("up,up,down,down,down", Unhealthy),
            ("up,up,down,down,up,down,down", Healthy),
            ("down,down,down", Unhealthy),
            ("down,down,down,up,up", Healthy),
            ("up,up,error,error", Healthy),
        ];
        let now = Instant::now();
        for (seq, expected) in rows {
            let mut h = BackendHealth::new();
            for r in seq.split(',') {
                h.apply(&parse(r), now, T);
            }
            assert_eq!(h.state, *expected, "sequence {seq}");
        }
    }

    #[test]
    fn transition_reported_once() {
        let now = Instant::now();
        let mut h = BackendHealth::new();
        assert_eq!(h.apply(&CheckResult::Up, now, T), None);
        assert_eq!(
            h.apply(&CheckResult::Up, now, T),
            Some(Transition {
                from: Unknown,
                to: Healthy
            })
        );
        assert_eq!(h.apply(&CheckResult::Up, now, T), None);
    }

    #[tokio::test(start_paused = true)]
    async fn persistent_errors_become_unknown() {
        let mut h = BackendHealth::new();
        h.apply(&CheckResult::Up, Instant::now(), T);
        h.apply(&CheckResult::Up, Instant::now(), T);
        assert_eq!(h.state, Healthy);
        let err = CheckResult::Error("plugin gone".into());
        h.apply(&err, Instant::now(), T);
        tokio::time::advance(Duration::from_secs(6)).await;
        // Exactly fall × interval: not yet "longer than".
        assert_eq!(h.apply(&err, Instant::now(), T), None);
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(
            h.apply(&err, Instant::now(), T),
            Some(Transition {
                from: Healthy,
                to: Unknown
            })
        );
        assert_eq!((h.successes, h.failures), (0, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn up_between_errors_restarts_error_window() {
        let mut h = BackendHealth::new();
        let err = CheckResult::Error("e".into());
        h.apply(&err, Instant::now(), T);
        tokio::time::advance(Duration::from_secs(5)).await;
        h.apply(&CheckResult::Up, Instant::now(), T);
        h.apply(&err, Instant::now(), T);
        tokio::time::advance(Duration::from_secs(5)).await;
        h.apply(&err, Instant::now(), T);
        assert_eq!(h.successes, 1, "window restarted, counters kept");
    }

    /// Rows of the "smoothing" example in the state-store spec.
    #[test]
    fn smoothing_table() {
        let down = CheckResult::Down("x".into());
        let error = CheckResult::Error("x".into());
        let rows: &[(Option<u64>, u64, &CheckResult, Option<u64>)] = &[
            (None, 1000, &CheckResult::Up, Some(1000)),
            (Some(1000), 2000, &CheckResult::Up, Some(1050)),
            (Some(1050), 2000, &CheckResult::Up, Some(1098)),
            (Some(1098), 900, &down, Some(1098)),
            (Some(1098), 900, &error, Some(1098)),
        ];
        for (prev, sample, result, want) in rows {
            assert_eq!(
                next_latency(*prev, result, *sample, None),
                *want,
                "{prev:?} {sample}"
            );
        }
    }

    /// Scenario "Cleared on unhealthy": `fall` downs empty the latency, and the
    /// first up check after recovery starts afresh.
    #[test]
    fn latency_cleared_on_unhealthy() {
        let t = Thresholds {
            rise: 1,
            fall: 2,
            interval: Duration::from_secs(1),
        };
        let now = Instant::now();
        let mut h = BackendHealth::new();
        let mut lat = None;
        let up = CheckResult::Up;
        let down = CheckResult::Down("refused".into());
        let tr = h.apply(&up, now, t);
        lat = next_latency(lat, &up, 1200, tr);
        assert_eq!((h.state, lat), (Healthy, Some(1200)));
        let tr = h.apply(&down, now, t);
        lat = next_latency(lat, &down, 900, tr);
        assert_eq!((h.state, lat), (Healthy, Some(1200)), "one down keeps it");
        let tr = h.apply(&down, now, t);
        lat = next_latency(lat, &down, 900, tr);
        assert_eq!((h.state, lat), (Unhealthy, None));
        let tr = h.apply(&up, now, t);
        lat = next_latency(lat, &up, 3000, tr);
        assert_eq!((h.state, lat), (Healthy, Some(3000)));
    }

    fn backend(id: &str, state: HealthState, latency_us: Option<u64>) -> BackendRecord {
        BackendRecord {
            id: id.into(),
            ip: "10.0.0.1".into(),
            port: 80,
            weight: 1,
            site: None,
            state,
            checked_at: None,
            reason: None,
            latency_us,
            recovered_at: None,
        }
    }

    #[test]
    fn fast_membership_table() {
        // Rows of the "fast membership" example: lowest latency in scope,
        // the backend's state, latency and fast flag before, fast after.
        let rows: &[(u64, HealthState, Option<u64>, bool, bool)] = &[
            (2000, Healthy, Some(2000), false, true),
            (2000, Healthy, Some(3000), false, true),
            (2000, Healthy, Some(3500), false, false),
            (2000, Healthy, Some(3500), true, true),
            (2000, Healthy, Some(4000), true, true),
            (2000, Healthy, Some(4001), true, false),
            (2000, Unknown, Some(100), false, false),
            (2000, Healthy, None, true, false),
            (200, Healthy, Some(1200), false, true),
            (200, Healthy, Some(1201), false, false),
            (200, Healthy, Some(2200), true, true),
            (200, Healthy, Some(2201), true, false),
        ];
        for &(min, state, latency, before, after) in rows {
            let backends = [
                backend("ref", Healthy, Some(min)),
                backend("b", state, latency),
            ];
            let previous: Vec<String> = if before { vec!["b".into()] } else { vec![] };
            let pool = fast_pool(&backends, &previous);
            assert_eq!(
                pool.contains(&"b".to_string()),
                after,
                "{min} {state:?} {latency:?} {before}"
            );
            assert!(pool.contains(&"ref".to_string()));
        }
    }

    #[test]
    fn pool_in_config_order_and_empty_without_scope() {
        let backends = [
            backend("a", Healthy, Some(2800)),
            backend("b", Healthy, Some(5000)),
            backend("c", Healthy, Some(2000)),
        ];
        assert_eq!(fast_pool(&backends, &[]), ["a", "c"]);
        let none = [backend("a", Unhealthy, None), backend("b", Healthy, None)];
        assert!(fast_pool(&none, &["a".into()]).is_empty());
    }
}
