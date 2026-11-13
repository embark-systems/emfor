//! The replication lag rule shared by the redis, mysql and pgsql plugins
//! for `role = "replica"` with `max_lag`.

use emfor_plugin::CheckOutcome;

/// Judge a replica's lag in seconds against `max_lag`.
///
/// `lag` is `None` when the server could not say how far behind it is.
/// Fractions of a second are dropped before comparing, so a lag of 30.9 s
/// passes `max_lag = 30`.
pub fn lag_outcome(lag: Option<f64>, max_lag: u64) -> CheckOutcome {
    let Some(lag) = lag else {
        return CheckOutcome::Down("replication: lag unknown".into());
    };
    // Negative values (clock skew between primary and replica) count as 0;
    // `as` saturates for values beyond u64.
    let whole = if lag > 0.0 { lag.floor() as u64 } else { 0 };
    if whole > max_lag {
        CheckOutcome::Down(format!("replication: {whole}s behind, max {max_lag}s"))
    } else {
        CheckOutcome::Up
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lag_table() {
        let rows = [
            (Some(0.0), 30, CheckOutcome::Up),
            (Some(30.0), 30, CheckOutcome::Up),
            (Some(30.9), 30, CheckOutcome::Up),
            (
                Some(31.0),
                30,
                CheckOutcome::Down("replication: 31s behind, max 30s".into()),
            ),
            (Some(-2.5), 1, CheckOutcome::Up),
            (
                None,
                30,
                CheckOutcome::Down("replication: lag unknown".into()),
            ),
        ];
        for (lag, max, want) in rows {
            assert_eq!(lag_outcome(lag, max), want, "{lag:?} vs {max}");
        }
    }
}
