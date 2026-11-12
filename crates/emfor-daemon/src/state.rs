//! Per-backend health state machine with haproxy-style `rise` / `fall`.

use std::time::Duration;

use emfor_format::HealthState;
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
}
