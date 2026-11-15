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
/// A `state` this reader does not know ([`HealthState::Unrecognized`])
/// counts as `unhealthy`, an `on_all_down` it does not know as `open`
/// ([`OnAllDown::is_open`]).
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
    let found = explain_tiers(on_all_down, list, trust_health, backend);
    (found.tier != Tier::None).then_some(found.candidates)
}

/// Which set of backends [`explain_tiers`] chose (spec lookup-command,
/// "Explain shows the tier and the reason of every backend"). The order is
/// the order of the tiers in [`healthy_or_all`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Healthy primary backends whose last check passed.
    PrimaryPassing,
    /// Healthy backup backends whose last check passed.
    BackupPassing,
    /// Healthy primary backends, failing ones included.
    PrimaryFailing,
    /// Healthy backup backends, failing ones included.
    BackupFailing,
    /// `unknown` backends whose last check passed (`on_all_down` `open`).
    UnknownPassing,
    /// Every `unknown` backend (`on_all_down` `open`).
    Unknown,
    /// Every backend (`on_all_down` `open`).
    All,
    /// No backend may be chosen: `closed`, or an empty list.
    None,
}

impl Tier {
    /// The name used in `--json` output.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::PrimaryPassing => "primary-passing",
            Tier::BackupPassing => "backup-passing",
            Tier::PrimaryFailing => "primary-failing",
            Tier::BackupFailing => "backup-failing",
            Tier::UnknownPassing => "unknown-passing",
            Tier::Unknown => "unknown",
            Tier::All => "all",
            Tier::None => "none",
        }
    }

    /// The tier in words, for the text output.
    pub fn describe(self) -> &'static str {
        match self {
            Tier::PrimaryPassing => "healthy primary backends whose last check passed",
            Tier::BackupPassing => "healthy backup backends whose last check passed",
            Tier::PrimaryFailing => {
                "healthy primary backends including those whose last check failed"
            }
            Tier::BackupFailing => {
                "healthy backup backends including those whose last check failed"
            }
            Tier::UnknownPassing => "unknown backends whose last check passed",
            Tier::Unknown => "every unknown backend",
            Tier::All => "every backend",
            Tier::None => "none",
        }
    }
}

/// Why an entry is not among the candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exclusion {
    /// The store is stale, so no backend counts as healthy or unknown.
    Stale,
    /// `unhealthy` (or a state this reader does not know).
    Unhealthy,
    /// `unknown`, and a better tier has candidates.
    Unknown,
    /// Healthy (or unknown) with a `reason`: its last check failed, and a
    /// passing backend of the tier above exists.
    LastCheckFailed,
    /// A backup backend while a primary backend is a candidate.
    BackupHeldBack,
    /// Drained in the config (`drain = true`): never chosen, in any tier.
    Drained,
}

impl Exclusion {
    /// The words used in the output and in `--json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Exclusion::Stale => "stale",
            Exclusion::Unhealthy => "unhealthy",
            Exclusion::Unknown => "unknown",
            Exclusion::LastCheckFailed => "last check failed",
            Exclusion::BackupHeldBack => "backup held back",
            Exclusion::Drained => "drained",
        }
    }
}

/// What [`explain_tiers`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct TierExplanation<T> {
    pub tier: Tier,
    /// The entries that may be chosen, in the order of `list`.
    pub candidates: Vec<T>,
    /// Every other entry, in the order of `list`, with the reason.
    pub excluded: Vec<(T, Exclusion)>,
}

/// The tiers of [`healthy_or_all`], with the reason for every entry that is
/// left out. `candidates` is what [`healthy_or_all`] returns, and `tier` is
/// [`Tier::None`] exactly when it returns `None`.
pub fn explain_tiers<'a, T: Copy>(
    on_all_down: OnAllDown,
    list: &[T],
    trust_health: bool,
    backend: impl Fn(T) -> &'a BackendRecord,
) -> TierExplanation<T> {
    let is_healthy = |t: T| trust_health && backend(t).state == HealthState::Healthy;
    let is_unknown = |t: T| trust_health && backend(t).state == HealthState::Unknown;
    // A drained backend is in no tier, the last one included.
    let pick = |keep: &dyn Fn(T) -> bool| -> Vec<usize> {
        (0..list.len())
            .filter(|&i| !backend(list[i]).drain && keep(list[i]))
            .collect()
    };
    let eligible = pick(&|_| true);
    let mut chosen: Option<(Tier, Vec<usize>)> = None;
    let healthy_tiers = [
        (Tier::PrimaryPassing, true, false),
        (Tier::BackupPassing, true, true),
        (Tier::PrimaryFailing, false, false),
        (Tier::BackupFailing, false, true),
    ];
    for (tier, passing_only, backup) in healthy_tiers {
        let idx = pick(&|t| {
            let b = backend(t);
            is_healthy(t) && b.backup == backup && (!passing_only || b.reason.is_none())
        });
        if !idx.is_empty() {
            chosen = Some((tier, idx));
            break;
        }
    }
    if chosen.is_none() && on_all_down.is_open() && !eligible.is_empty() {
        let unknown_passing = pick(&|t| is_unknown(t) && backend(t).reason.is_none());
        let unknown = pick(&|t| is_unknown(t));
        chosen = Some(if !unknown_passing.is_empty() {
            (Tier::UnknownPassing, unknown_passing)
        } else if !unknown.is_empty() {
            (Tier::Unknown, unknown)
        } else {
            (Tier::All, eligible)
        });
    }
    let (tier, idx) = chosen.unwrap_or((Tier::None, Vec::new()));
    let why = |t: T| -> Exclusion {
        if backend(t).drain {
            return Exclusion::Drained;
        }
        if !trust_health {
            return Exclusion::Stale;
        }
        let b = backend(t);
        match b.state {
            HealthState::Healthy => {
                if b.backup && matches!(tier, Tier::PrimaryPassing | Tier::PrimaryFailing) {
                    Exclusion::BackupHeldBack
                } else {
                    Exclusion::LastCheckFailed
                }
            }
            HealthState::Unknown => {
                if tier == Tier::UnknownPassing && b.reason.is_some() {
                    Exclusion::LastCheckFailed
                } else {
                    Exclusion::Unknown
                }
            }
            HealthState::Unhealthy | HealthState::Unrecognized => Exclusion::Unhealthy,
        }
    };
    TierExplanation {
        tier,
        candidates: idx.iter().map(|&i| list[i]).collect(),
        excluded: (0..list.len())
            .filter(|i| !idx.contains(i))
            .map(|i| (list[i], why(list[i])))
            .collect(),
    }
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
            drain: false,
            checked_state: None,
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
        let future = b("10.0.0.1:80", Unrecognized, None);
        let future_unknown = [&future, &ub];
        let future_down = [&future, &db];
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
            // Values from a newer daemon: an unrecognized state is
            // unhealthy, an unknown on_all_down is open.
            (
                &future_unknown,
                OnAllDown::Open,
                true,
                Some(vec!["10.0.0.2:80"]),
            ),
            (
                &future_down,
                OnAllDown::Unknown,
                true,
                Some(vec!["10.0.0.1:80", "10.0.0.2:80"]),
            ),
            (&future_down, OnAllDown::Closed, true, None),
            (
                &only_down,
                OnAllDown::Unknown,
                true,
                Some(vec!["10.0.0.2:80"]),
            ),
            (&[], OnAllDown::Unknown, true, None),
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

    /// The original `healthy_or_all`, kept to guard the rewrite that calls
    /// [`explain_tiers`] (task 1.2): same candidates for every input.
    fn reference_healthy_or_all<'a, T: Copy>(
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
        if !on_all_down.is_open() || list.is_empty() {
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

    fn rec(id: &str, state: HealthState, reason: bool, backup: bool) -> BackendRecord {
        let mut r = b(id, state, None);
        r.reason = reason.then(|| "connect: Connection refused".to_string());
        r.backup = backup;
        r
    }

    /// Every list of up to three backends over every state, reason and
    /// backup combination, with fresh and stale stores and both
    /// `on_all_down` values: `explain_tiers` gives the candidates
    /// `healthy_or_all` gave, `tier` is `None` exactly when there are none,
    /// and every entry is a candidate or has a reason.
    #[test]
    fn explain_tiers_matches_the_candidates_of_healthy_or_all() {
        use HealthState::*;
        let kinds: Vec<(HealthState, bool, bool)> = [Healthy, Unhealthy, Unknown, Unrecognized]
            .into_iter()
            .flat_map(|s| [false, true].map(move |r| (s, r)))
            .flat_map(|(s, r)| [false, true].map(move |bk| (s, r, bk)))
            .collect();
        let mut lists: Vec<Vec<BackendRecord>> = vec![vec![]];
        for n in 1..=3usize {
            let mut idx = vec![0usize; n];
            loop {
                lists.push(
                    idx.iter()
                        .enumerate()
                        .map(|(i, &k)| {
                            let (s, r, bk) = kinds[k];
                            rec(&format!("10.0.0.{}:80", i + 1), s, r, bk)
                        })
                        .collect(),
                );
                let mut pos = 0;
                while pos < n {
                    idx[pos] += 1;
                    if idx[pos] < kinds.len() {
                        break;
                    }
                    idx[pos] = 0;
                    pos += 1;
                }
                if pos == n {
                    break;
                }
            }
        }
        let mut checked = 0;
        for list in &lists {
            let refs: Vec<&BackendRecord> = list.iter().collect();
            for trust in [true, false] {
                for oad in [OnAllDown::Open, OnAllDown::Closed, OnAllDown::Unknown] {
                    let want = reference_healthy_or_all(oad, &refs, trust, |b| b);
                    let got = explain_tiers(oad, &refs, trust, |b| b);
                    let ids =
                        |v: &[&BackendRecord]| v.iter().map(|b| b.id.clone()).collect::<Vec<_>>();
                    assert_eq!(
                        want.as_deref().map(ids),
                        (got.tier != Tier::None).then(|| ids(&got.candidates)),
                        "{list:?} trust={trust} {oad:?}"
                    );
                    assert_eq!(
                        got.candidates.len() + got.excluded.len(),
                        refs.len(),
                        "{list:?}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 1000, "{checked}");
    }

    /// Spec backend-selection "Drained backends are never selected": a
    /// drained backend is in no tier, whatever its state, with a fresh or a
    /// stale store and either `on_all_down`; it is excluded as `drained`.
    #[test]
    fn drained_backends_are_in_no_tier() {
        use HealthState::*;
        for state in [Healthy, Unhealthy, Unknown, Unrecognized] {
            for reason in [false, true] {
                let mut x = rec("10.0.0.1:80", state, reason, false);
                x.drain = true;
                let others = [
                    rec("10.0.0.2:80", Healthy, false, false),
                    rec("10.0.0.2:80", Unhealthy, true, false),
                    rec("10.0.0.2:80", Unknown, false, true),
                ];
                for y in &others {
                    for trust in [true, false] {
                        for oad in [OnAllDown::Open, OnAllDown::Closed] {
                            let list = [&x, y];
                            let got = explain_tiers(oad, &list, trust, |b| b);
                            assert!(
                                got.candidates.iter().all(|b| b.id != x.id),
                                "{state:?} {reason} {trust} {oad:?}: {got:?}"
                            );
                            assert!(
                                got.excluded
                                    .iter()
                                    .any(|(b, why)| b.id == x.id && *why == Exclusion::Drained),
                                "{got:?}"
                            );
                        }
                    }
                }
                // Everything drained: nothing to choose, `open` included.
                let mut only = [x.clone(), x.clone()];
                only[1].id = "10.0.0.2:80".into();
                let refs: Vec<&BackendRecord> = only.iter().collect();
                for oad in [OnAllDown::Open, OnAllDown::Closed] {
                    let got = explain_tiers(oad, &refs, true, |b| b);
                    assert_eq!(got.tier, Tier::None, "{oad:?}");
                    assert!(got.candidates.is_empty());
                    assert_eq!(got.excluded.len(), 2);
                    assert!(healthy_or_all(oad, &refs, true, |b| b).is_none());
                }
            }
        }
    }

    /// The last tier of an open service never offers a drained backend.
    #[test]
    fn last_tier_skips_the_drained_backend() {
        use HealthState::*;
        let mut a = rec("10.0.0.1:80", Unhealthy, true, false);
        a.drain = true;
        let b = rec("10.0.0.2:80", Unhealthy, true, false);
        let refs = [&a, &b];
        let got = explain_tiers(OnAllDown::Open, &refs, true, |x| x);
        assert_eq!(got.tier, Tier::All);
        assert_eq!(got.candidates.len(), 1);
        assert_eq!(got.candidates[0].id, "10.0.0.2:80");
    }

    /// Spec lookup-command, example "tier of the candidates".
    #[test]
    fn tier_of_the_candidates_table() {
        use HealthState::*;
        let primary_ok = rec("10.0.0.1:80", Healthy, false, false);
        let primary_failing = rec("10.0.0.2:80", Healthy, true, false);
        let backup_ok = rec("10.0.0.3:80", Healthy, false, true);
        let down = rec("10.0.0.4:80", Unhealthy, true, false);
        let unknown_ok = rec("10.0.0.5:80", Unknown, false, false);
        let rows: [(Vec<&BackendRecord>, OnAllDown, Tier); 6] = [
            (
                vec![&primary_ok, &down],
                OnAllDown::Open,
                Tier::PrimaryPassing,
            ),
            (
                vec![&primary_failing, &backup_ok],
                OnAllDown::Open,
                Tier::BackupPassing,
            ),
            (
                vec![&primary_failing, &down],
                OnAllDown::Open,
                Tier::PrimaryFailing,
            ),
            (
                vec![&down, &unknown_ok],
                OnAllDown::Open,
                Tier::UnknownPassing,
            ),
            (vec![&down], OnAllDown::Open, Tier::All),
            (vec![&down], OnAllDown::Closed, Tier::None),
        ];
        for (list, oad, want) in rows {
            let got = explain_tiers(oad, &list, true, |b| b);
            assert_eq!(got.tier, want, "{list:?} {oad:?}");
        }
    }

    /// Spec scenario "Primary and backup backends" and "Stale store".
    #[test]
    fn explain_tiers_gives_the_reason_of_every_excluded_entry() {
        use HealthState::*;
        let a = rec("10.0.0.11:8080", Healthy, false, false);
        let b2 = rec("10.0.0.12:8080", Healthy, true, false);
        let c = rec("10.0.0.13:8080", Healthy, false, true);
        let d = rec("10.0.0.14:8080", Unhealthy, true, false);
        let list = [&a, &b2, &c, &d];
        let got = explain_tiers(OnAllDown::Open, &list, true, |b| b);
        assert_eq!(got.tier, Tier::PrimaryPassing);
        assert_eq!(got.candidates.len(), 1);
        assert_eq!(got.candidates[0].id, "10.0.0.11:8080");
        let why: Vec<(&str, &str)> = got
            .excluded
            .iter()
            .map(|(b, e)| (b.id.as_str(), e.as_str()))
            .collect();
        assert_eq!(
            why,
            [
                ("10.0.0.12:8080", "last check failed"),
                ("10.0.0.13:8080", "backup held back"),
                ("10.0.0.14:8080", "unhealthy"),
            ]
        );
        // A stale store: nothing counts as healthy; open uses every backend,
        // closed has none and every entry is excluded as stale.
        let stale_open = explain_tiers(OnAllDown::Open, &list, false, |b| b);
        assert_eq!(stale_open.tier, Tier::All);
        assert_eq!(stale_open.candidates.len(), 4);
        let stale_closed = explain_tiers(OnAllDown::Closed, &list, false, |b| b);
        assert_eq!(stale_closed.tier, Tier::None);
        assert!(
            stale_closed
                .excluded
                .iter()
                .all(|(_, e)| *e == Exclusion::Stale)
        );
        // A passing unknown beats a failing unknown.
        let u1 = rec("10.0.0.21:80", Unknown, true, false);
        let u2 = rec("10.0.0.22:80", Unknown, false, false);
        let got = explain_tiers(OnAllDown::Open, &[&u1, &u2], true, |b| b);
        assert_eq!(got.tier, Tier::UnknownPassing);
        assert_eq!(got.excluded[0].1, Exclusion::LastCheckFailed);
    }
}
