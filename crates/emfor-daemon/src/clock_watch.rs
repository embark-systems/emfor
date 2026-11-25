//! Detects a jump of the host's wall clock between two heartbeat ticks.
//!
//! The heartbeat is a wall-clock time, so a clock that is set (not slewed)
//! shows up in the store as a stale or future heartbeat. Comparing how far
//! the wall clock moved with how far a monotonic clock moved between the
//! same two ticks tells a clock jump from a busy or late tick: a late tick
//! moves both clocks by the same amount. The caller passes both times in
//! milliseconds, so the comparison needs no system clock and tests feed it
//! plain numbers.

/// A wall clock that moved this much more or less than the monotonic clock
/// between two ticks counts as a jump. Slewing by NTP changes the rate by
/// at most about 0.5 ms per second, far below this. Chosen with a wide
/// margin over the largest difference measured under load (1 ms on
/// FreeBSD, see the CHANGELOG), not tuned.
pub const JUMP_THRESHOLD_MS: u64 = 2000;

/// At most one jump is logged in this period (by the monotonic clock);
/// jumps inside it are still counted.
pub const LOG_EVERY_MS: u64 = 10_000;

/// Which way the wall clock moved relative to the monotonic clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Forward => "forward",
            Direction::Backward => "backward",
        }
    }
}

/// One detected jump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockJump {
    pub direction: Direction,
    /// How far the wall clock moved beyond the monotonic clock, always
    /// positive.
    pub jump_ms: u64,
    /// Whether the jump is to be logged: false when a jump was logged less
    /// than [`LOG_EVERY_MS`] ago.
    pub log: bool,
}

/// State kept between ticks.
#[derive(Debug, Default)]
pub struct ClockWatch {
    /// Wall and monotonic time of the previous tick; `None` before the
    /// first tick, which only sets the baseline.
    prev: Option<(u64, u64)>,
    /// Monotonic time of the last jump that was marked for logging.
    last_logged: Option<u64>,
}

impl ClockWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Note a tick at `wall_ms` (milliseconds since the Unix epoch) and
    /// `mono_ms` (milliseconds on a monotonic clock). Returns the jump since
    /// the previous tick, if there is one.
    pub fn observe(&mut self, wall_ms: u64, mono_ms: u64) -> Option<ClockJump> {
        let prev = self.prev.replace((wall_ms, mono_ms));
        let (prev_wall, prev_mono) = prev?;
        let wall_delta = i128::from(wall_ms) - i128::from(prev_wall);
        let mono_delta = i128::from(mono_ms) - i128::from(prev_mono);
        let jump = wall_delta - mono_delta;
        let jump_ms = u64::try_from(jump.unsigned_abs()).unwrap_or(u64::MAX);
        if jump_ms <= JUMP_THRESHOLD_MS {
            return None;
        }
        let log = self
            .last_logged
            .is_none_or(|at| mono_ms.saturating_sub(at) >= LOG_EVERY_MS);
        if log {
            self.last_logged = Some(mono_ms);
        }
        Some(ClockJump {
            direction: if jump > 0 {
                Direction::Forward
            } else {
                Direction::Backward
            },
            jump_ms,
            log,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u64 = 1_700_000_000_000;

    // spectra-scenario: state-store / First tick is only a baseline
    #[test]
    fn first_tick_is_only_a_baseline() {
        let mut w = ClockWatch::new();
        // Whatever the first values are, nothing is reported.
        assert_eq!(w.observe(W, 5), None);
        // The second tick compares against the first.
        assert_eq!(w.observe(W + 1000, 1005), None);
    }

    #[test]
    fn steady_ticks_report_nothing() {
        let mut w = ClockWatch::new();
        for i in 0..100u64 {
            assert_eq!(w.observe(W + i * 1000, i * 1000), None);
        }
    }

    // spectra-scenario: state-store / Wall clock stepped forward
    #[test]
    fn wall_clock_stepped_forward() {
        let mut w = ClockWatch::new();
        w.observe(W, 0);
        let j = w.observe(W + 1000 + 7000, 1000).unwrap();
        assert_eq!(j.direction, Direction::Forward);
        assert_eq!(j.jump_ms, 7000);
        assert!(j.log);
    }

    // spectra-scenario: state-store / Wall clock stepped backward
    #[test]
    fn wall_clock_stepped_backward() {
        let mut w = ClockWatch::new();
        w.observe(W, 0);
        // Wall clock moved back 4000 ms while the monotonic clock moved 1000.
        let j = w.observe(W - 4000, 1000).unwrap();
        assert_eq!(j.direction, Direction::Backward);
        assert_eq!(j.jump_ms, 5000);
        assert!(j.log);
    }

    // spectra-scenario: state-store / Late tick without a clock change
    #[test]
    fn late_tick_with_both_clocks_moving_is_not_a_jump() {
        let mut w = ClockWatch::new();
        w.observe(W, 0);
        assert_eq!(w.observe(W + 6000, 6000), None);
    }

    // spectra-scenario: state-store / Difference at the threshold
    #[test]
    fn threshold_boundary() {
        let mut w = ClockWatch::new();
        w.observe(W, 0);
        // Exactly 2000 ms beyond the monotonic clock: not a jump.
        assert_eq!(w.observe(W + 1000 + 2000, 1000), None);
        // 2001 ms: a jump (the baseline moved to the previous tick).
        let j = w.observe(W + 3000 + 1000 + 2001, 2000).unwrap();
        assert_eq!(j.jump_ms, 2001);
        assert_eq!(j.direction, Direction::Forward);
        // The same in the other direction.
        let mut w = ClockWatch::new();
        w.observe(W, 0);
        assert_eq!(w.observe(W + 1000 - 2000, 1000), None);
        let j = w.observe(W + 1000 - 2000 + 1000 - 2001, 2000).unwrap();
        assert_eq!(j.jump_ms, 2001);
        assert_eq!(j.direction, Direction::Backward);
    }

    // spectra-scenario: state-store / Repeated jumps are limited
    #[test]
    fn jumps_inside_the_log_period_are_reported_but_not_logged() {
        let mut w = ClockWatch::new();
        let mut wall = W;
        let mut mono = 0u64;
        w.observe(wall, mono);
        let mut tick = |w: &mut ClockWatch, step: u64| {
            wall += 1000 + step;
            mono += 1000;
            w.observe(wall, mono)
        };
        for _ in 0..19 {
            assert_eq!(tick(&mut w, 0), None);
        }
        // Tick 20: the first jump is logged.
        let first = tick(&mut w, 9000).unwrap();
        assert!(first.log);
        for _ in 0..4 {
            assert_eq!(tick(&mut w, 0), None);
        }
        // Five seconds later: reported, not logged.
        let second = tick(&mut w, 9000).unwrap();
        assert!(!second.log);
        // Ten seconds after the first logged jump: logged again.
        for _ in 0..4 {
            assert_eq!(tick(&mut w, 0), None);
        }
        let third = tick(&mut w, 9000).unwrap();
        assert!(third.log);
    }

    #[test]
    fn an_unlogged_jump_does_not_move_the_log_period() {
        let mut w = ClockWatch::new();
        w.observe(W, 0);
        // Logged at mono 1000.
        assert!(w.observe(W + 1000 + 5000, 1000).unwrap().log);
        // At mono 6000 and 10500: inside 10 s of the logged one.
        assert!(!w.observe(W + 6000 + 5000 + 5000, 6000).unwrap().log);
        assert!(!w.observe(W + 10_500 + 10_000 + 5000, 10_500).unwrap().log);
        // At mono 11000: 10 s after the logged one, not after the last jump.
        assert!(w.observe(W + 11_000 + 15_000 + 5000, 11_000).unwrap().log);
    }

    #[test]
    fn huge_difference_does_not_overflow() {
        let mut w = ClockWatch::new();
        w.observe(u64::MAX, 0);
        let j = w.observe(0, 1000).unwrap();
        assert_eq!(j.direction, Direction::Backward);
        assert_eq!(j.jump_ms, u64::MAX);
    }
}
