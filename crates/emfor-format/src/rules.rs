//! Selection rules shared by the daemon's DNS server and the client
//! library: which backends of a record may be chosen. How one of them is
//! then picked (turns, weights, affinity) differs between the two and stays
//! with each.

use crate::{BackendRecord, Balance, HealthState, OnAllDown};

/// The entries of `list` that may be chosen, in their order:
///
/// 1. the passing primary ones (`healthy` with no `reason`: the last check
///    was up; `backup` false), when there are any;
/// 2. otherwise the passing backup ones (`backup` true);
/// 3. otherwise the healthy primary ones, failing ones (`healthy` with a
///    `reason`) included;
/// 4. otherwise the healthy backup ones, failing ones included;
/// 5. otherwise, when `on_all_down` is `open`, the passing unknown ones
///    (`unknown` with no `reason`), primary and backup alike;
/// 6. otherwise, when `open`, every `unknown` one;
/// 7. otherwise every entry when `open`, or `None` (all down) when
///    `on_all_down` is `closed` or `list` is empty.
///
/// `backend` gives each entry's record; `trust_health` false (a stale
/// store) counts no backend as healthy or unknown, so neither `state` nor
/// `reason` plays a part and an open service uses every entry.
pub fn healthy_or_all<'a, T: Copy>(
    on_all_down: OnAllDown,
    list: &[T],
    trust_health: bool,
    backend: impl Fn(T) -> &'a BackendRecord,
) -> Option<Vec<T>> {
    let healthy: Vec<T> = list
        .iter()
        .copied()
        .filter(|&t| trust_health && backend(t).state == HealthState::Healthy)
        .collect();
    let tiers: [(bool, bool); 4] = [(true, false), (true, true), (false, false), (false, true)];
    for (passing_only, backup) in tiers {
        let tier: Vec<T> = healthy
            .iter()
            .copied()
            .filter(|&t| {
                let b = backend(t);
                b.backup == backup && (!passing_only || b.reason.is_none())
            })
            .collect();
        if !tier.is_empty() {
            return Some(tier);
        }
    }
    if on_all_down != OnAllDown::Open || list.is_empty() {
        return None;
    }
    let unknown: Vec<T> = list
        .iter()
        .copied()
        .filter(|&t| trust_health && backend(t).state == HealthState::Unknown)
        .collect();
    let unknown_passing: Vec<T> = unknown
        .iter()
        .copied()
        .filter(|&t| backend(t).reason.is_none())
        .collect();
    if !unknown_passing.is_empty() {
        return Some(unknown_passing);
    }
    if !unknown.is_empty() {
        return Some(unknown);
    }
    Some(list.to_vec())
}

/// Steps of slow start: a ramping backend gets 1 to 9 tenths of its base
/// weight, one that is not ramping all ten.
pub const SLOW_START_STEPS: u32 = 10;

/// The slow start step of `b`, from 1 to [`SLOW_START_STEPS`]: the tenths of
/// `slow_start_ms` passed since `b.recovered_at`, rounded up and at least 1,
/// or all of them when `b` has not recovered, slow start is off, the clock
/// is earlier than `recovered_at`, or `slow_start_ms` has passed.
pub fn slow_start_step(b: &BackendRecord, slow_start_ms: u64, now_ms: u64) -> u32 {
    let full = SLOW_START_STEPS;
    let Some(at) = b.recovered_at else {
        return full;
    };
    if slow_start_ms == 0 || now_ms < at {
        return full;
    }
    let elapsed = now_ms - at;
    if elapsed >= slow_start_ms {
        return full;
    }
    // elapsed < slow_start_ms, so the quotient is below `full`.
    let step = (u64::from(full) * elapsed).div_ceil(slow_start_ms);
    u32::try_from(step).unwrap_or(full).max(1)
}

/// `base` (the backend's weight for `weighted`, 1 otherwise) times the slow
/// start step of `b`.
pub fn effective_weight(b: &BackendRecord, base: u32, slow_start_ms: u64, now_ms: u64) -> u32 {
    base.saturating_mul(slow_start_step(b, slow_start_ms, now_ms))
}

/// Whether any of `list` is in slow start (its step is below the full one).
pub fn any_ramping<'a, T: Copy>(
    list: &[T],
    slow_start_ms: u64,
    now_ms: u64,
    backend: impl Fn(T) -> &'a BackendRecord,
) -> bool {
    list.iter()
        .any(|&t| slow_start_step(backend(t), slow_start_ms, now_ms) < SLOW_START_STEPS)
}

/// The base weight for slow start: the backend's `weight` for `weighted`,
/// 1 for every other method.
pub fn base_weight(balance: Balance, b: &BackendRecord) -> u32 {
    if balance == Balance::Weighted {
        b.weight
    } else {
        1
    }
}

/// Whether `balance` prefers backend `b`: `locality` prefers the host's
/// `site`, `fastest` the backends in the daemon's `pool`; other methods
/// prefer none.
pub fn preferred(
    balance: Balance,
    b: &BackendRecord,
    site: Option<&str>,
    pool: Option<&[String]>,
) -> bool {
    match balance {
        Balance::Locality => site.is_some() && b.site.as_deref() == site,
        Balance::Fastest => pool.is_some_and(|p| p.contains(&b.id)),
        _ => false,
    }
}

/// The entries `keep` accepts, or all of `list` when it accepts none.
pub fn subset_or_all<T: Copy>(list: &[T], keep: impl Fn(T) -> bool) -> Vec<T> {
    let kept: Vec<T> = list.iter().copied().filter(|&t| keep(t)).collect();
    if kept.is_empty() { list.to_vec() } else { kept }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(id: &str, state: HealthState, site: Option<&str>) -> BackendRecord {
        let (ip, port) = id.rsplit_once(':').unwrap();
        BackendRecord {
            id: id.into(),
            ip: ip.into(),
            port: port.parse().unwrap(),
            weight: 1,
            site: site.map(Into::into),
            state,
            checked_at: None,
            reason: None,
            latency_us: None,
            backup: false,
            recovered_at: None,
        }
    }

    #[test]
    fn healthy_or_all_table() {
        use HealthState::*;
        let up = b("10.0.0.1:80", Healthy, None);
        let down = b("10.0.0.2:80", Unhealthy, None);
        let id = |r: &BackendRecord| r.id.clone();
        let ids =
            |v: Option<Vec<&BackendRecord>>| v.map(|v| v.into_iter().map(id).collect::<Vec<_>>());
        let failing = |id: &str, why: &str| {
            let mut r = b(id, Healthy, None);
            r.reason = Some(why.into());
            r
        };
        // client-lookup example "candidates by state, reason and heartbeat"
        let (pa, fa, fb) = (
            b("10.0.0.1:80", Healthy, None),
            failing("10.0.0.1:80", "refused"),
            failing("10.0.0.2:80", "timeout"),
        );
        let (pb, fc) = (
            b("10.0.0.2:80", Healthy, None),
            failing("10.0.0.2:80", "refused"),
        );
        let mut dc = b("10.0.0.3:80", Unhealthy, None);
        dc.reason = Some("refused".into());
        let mut da = b("10.0.0.1:80", Unhealthy, None);
        da.reason = Some("refused".into());
        let ub = b("10.0.0.2:80", Unknown, None);
        let passing_first = [&pa, &fc, &dc];
        let all_failing = [&fa, &fb, &dc];
        let none_healthy = [&da, &ub];
        let mixed = [&fa, &pb];
        let unknown_with = |id: &str, why: &str| {
            let mut r = b(id, Unknown, None);
            r.reason = Some(why.into());
            r
        };
        let (ut, ur, uc) = (
            unknown_with("10.0.0.2:80", "timeout"),
            unknown_with("10.0.0.3:80", "refused"),
            b("10.0.0.3:80", Unknown, None),
        );
        let mut db = b("10.0.0.2:80", Unhealthy, None);
        db.reason = Some("timeout".into());
        let unknown_passing = [&da, &ut, &uc];
        let unknown_failing = [&da, &ut, &ur];
        let two_down = [&da, &db];
        // Backups (B is 10.0.0.2:80 marked `backup`).
        let backup = |mut r: BackendRecord| {
            r.backup = true;
            r
        };
        let (bp, bf, bu) = (
            backup(b("10.0.0.2:80", Healthy, None)),
            backup(failing("10.0.0.2:80", "refused")),
            backup(b("10.0.0.2:80", Unknown, None)),
        );
        let failing_primary_passing_backup = [&fa, &bp];
        let both_failing = [&fa, &bf];
        let down_primary_failing_backup = [&da, &bf];
        let down_primary_unknown_backup = [&da, &bu];
        let passing_both = [&pa, &bp];
        let both = [&up, &down];
        let only_down = [&down];
        type Case<'a> = (
            &'a [&'a BackendRecord],
            OnAllDown,
            bool,
            Option<Vec<&'a str>>,
        );
        let cases: &[Case] = &[
            (&both, OnAllDown::Closed, true, Some(vec!["10.0.0.1:80"])),
            (&only_down, OnAllDown::Closed, true, None),
            (&only_down, OnAllDown::Open, true, Some(vec!["10.0.0.2:80"])),
            (&both, OnAllDown::Closed, false, None),
            (
                &both,
                OnAllDown::Open,
                false,
                Some(vec!["10.0.0.1:80", "10.0.0.2:80"]),
            ),
            (&[], OnAllDown::Open, true, None),
            (
                &passing_first,
                OnAllDown::Closed,
                true,
                Some(vec!["10.0.0.1:80"]),
            ),
            (
                &all_failing,
                OnAllDown::Closed,
                true,
                Some(vec!["10.0.0.1:80", "10.0.0.2:80"]),
            ),
            (&none_healthy, OnAllDown::Closed, true, None),
            (
                &none_healthy,
                OnAllDown::Open,
                true,
                Some(vec!["10.0.0.2:80"]),
            ),
            (
                &unknown_passing,
                OnAllDown::Open,
                true,
                Some(vec!["10.0.0.3:80"]),
            ),
            (
                &unknown_failing,
                OnAllDown::Open,
                true,
                Some(vec!["10.0.0.2:80", "10.0.0.3:80"]),
            ),
            (
                &two_down,
                OnAllDown::Open,
                true,
                Some(vec!["10.0.0.1:80", "10.0.0.2:80"]),
            ),
            (&unknown_passing, OnAllDown::Closed, true, None),
            (
                &none_healthy,
                OnAllDown::Open,
                false,
                Some(vec!["10.0.0.1:80", "10.0.0.2:80"]),
            ),
            (
                &mixed,
                OnAllDown::Open,
                false,
                Some(vec!["10.0.0.1:80", "10.0.0.2:80"]),
            ),
            (&mixed, OnAllDown::Closed, false, None),
            (
                &failing_primary_passing_backup,
                OnAllDown::Closed,
                true,
                Some(vec!["10.0.0.2:80"]),
            ),
            (
                &both_failing,
                OnAllDown::Closed,
                true,
                Some(vec!["10.0.0.1:80"]),
            ),
            (
                &down_primary_failing_backup,
                OnAllDown::Closed,
                true,
                Some(vec!["10.0.0.2:80"]),
            ),
            (
                &down_primary_unknown_backup,
                OnAllDown::Open,
                true,
                Some(vec!["10.0.0.2:80"]),
            ),
            (
                &passing_both,
                OnAllDown::Open,
                false,
                Some(vec!["10.0.0.1:80", "10.0.0.2:80"]),
            ),
            (
                &passing_both,
                OnAllDown::Closed,
                true,
                Some(vec!["10.0.0.1:80"]),
            ),
        ];
        for (list, mode, trust, want) in cases {
            let got = ids(healthy_or_all(*mode, list, *trust, |r| r));
            let want = want
                .as_ref()
                .map(|w| w.iter().map(|s| s.to_string()).collect());
            assert_eq!(
                got,
                want,
                "{mode:?} trust={trust} {:?}",
                list.iter()
                    .map(|r| (&r.id, r.state, &r.reason))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn preferred_by_balance() {
        let local = b("10.0.0.1:80", HealthState::Healthy, Some("dc1"));
        let pool = vec!["10.0.0.1:80".to_string()];
        assert!(preferred(Balance::Locality, &local, Some("dc1"), None));
        assert!(!preferred(Balance::Locality, &local, Some("dc2"), None));
        assert!(!preferred(Balance::Locality, &local, None, None));
        assert!(preferred(Balance::Fastest, &local, None, Some(&pool)));
        assert!(!preferred(Balance::Fastest, &local, None, None));
        assert!(!preferred(
            Balance::RoundRobin,
            &local,
            Some("dc1"),
            Some(&pool)
        ));
    }

    /// Spec "Slow start effective weight", example "step and effective
    /// weight", every row.
    #[test]
    fn slow_start_effective_weight_table() {
        let at = 1_762_905_600_000;
        /// recovered_at, slow_start_ms, now, balance, weight, step, effective.
        type Row = (Option<u64>, u64, u64, Balance, u32, u32, u32);
        let rows: &[Row] = &[
            (Some(at), 30000, at, Balance::RoundRobin, 1, 1, 1),
            (Some(at), 30000, at + 1000, Balance::RoundRobin, 1, 1, 1),
            (Some(at), 30000, at + 7500, Balance::RoundRobin, 1, 3, 3),
            (Some(at), 30000, at + 29900, Balance::Weighted, 4, 10, 40),
            (Some(at), 30000, at + 30000, Balance::Weighted, 4, 10, 40),
            (Some(at), 30000, at + 7500, Balance::Weighted, 4, 3, 12),
            (None, 30000, at + 7500, Balance::Weighted, 4, 10, 40),
            (Some(at), 0, at + 7500, Balance::RoundRobin, 1, 10, 10),
            (
                Some(at),
                30000,
                at - 100_000,
                Balance::RoundRobin,
                1,
                10,
                10,
            ),
        ];
        for &(recovered_at, slow, now, balance, weight, step, effective) in rows {
            let mut r = b("10.0.0.1:80", HealthState::Healthy, None);
            r.recovered_at = recovered_at;
            r.weight = weight;
            let row = format!("{recovered_at:?} {slow} {now} {balance:?} {weight}");
            assert_eq!(slow_start_step(&r, slow, now), step, "{row}");
            let base = base_weight(balance, &r);
            assert_eq!(effective_weight(&r, base, slow, now), effective, "{row}");
        }
    }

    /// Spec scenario "Step during a 30 second slow start".
    #[test]
    fn slow_start_steps_over_thirty_seconds() {
        let mut r = b("10.0.0.1:80", HealthState::Healthy, None);
        r.recovered_at = Some(1_762_905_600_000);
        let steps: Vec<u32> = [0, 3000, 10000, 30000, 40000]
            .iter()
            .map(|d| slow_start_step(&r, 30000, 1_762_905_600_000 + d))
            .collect();
        assert_eq!(steps, [1, 1, 4, 10, 10]);
        let other = b("10.0.0.2:80", HealthState::Healthy, None);
        assert!(any_ramping(&[&r, &other], 30000, 1_762_905_610_000, |x| x));
        assert!(!any_ramping(&[&r, &other], 30000, 1_762_905_630_000, |x| x));
        assert!(!any_ramping(&[&other], 30000, 1_762_905_610_000, |x| x));
    }

    #[test]
    fn subset_falls_back_to_all() {
        assert_eq!(subset_or_all(&[1, 2, 3], |x| x > 1), [2, 3]);
        assert_eq!(subset_or_all(&[1, 2, 3], |x| x > 5), [1, 2, 3]);
    }
}
