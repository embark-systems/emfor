//! Choosing one backend of a service without an affinity key.
//!
//! This is the reference for the selection rules that every language client
//! must follow: candidates per address family, per-family all-down handling,
//! then `balance`. No I/O.

use std::net::IpAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use emfor_format::{BackendRecord, Balance, ServiceRecord, rules};

use crate::live::{FamilyChoice, Swrr};

/// Address family asked for: IPv4 for A, IPv6 for AAAA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    fn matches(self, ip: &IpAddr) -> bool {
        matches!(
            (self, ip),
            (Family::V4, IpAddr::V4(_)) | (Family::V6, IpAddr::V6(_))
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Chosen(IpAddr),
    /// The service has no backend of this family.
    NoFamily,
    /// No backend of this family is healthy and `on_all_down` is `closed`.
    AllDown,
}

/// Backends that may be chosen for `family`, in config order, or why none may.
fn candidate_backends(
    record: &ServiceRecord,
    family: Family,
) -> Result<Vec<(IpAddr, &BackendRecord)>, Selection> {
    let of_family: Vec<(IpAddr, &BackendRecord)> = parsed(record)
        .filter(|(ip, _)| family.matches(ip))
        .collect();
    if of_family.is_empty() {
        return Err(Selection::NoFamily);
    }
    healthy_or_all(record, of_family)
}

/// Backends whose address parses, in config order.
fn parsed(record: &ServiceRecord) -> impl Iterator<Item = (IpAddr, &BackendRecord)> {
    record
        .backends
        .iter()
        .filter_map(|b| b.ip.parse::<IpAddr>().ok().map(|ip| (ip, b)))
}

/// The candidates of `list` by health and `on_all_down`, or `AllDown`; the
/// daemon's store is always fresh. The tiers are in [`rules::healthy_or_all`].
fn healthy_or_all<'a>(
    record: &ServiceRecord,
    list: Vec<(IpAddr, &'a BackendRecord)>,
) -> Result<Vec<(IpAddr, &'a BackendRecord)>, Selection> {
    rules::healthy_or_all(record.on_all_down, &list, true, |(_, b)| b).ok_or(Selection::AllDown)
}

/// Addresses that may be chosen for `family`, in config order, or why none may.
pub fn candidates(record: &ServiceRecord, family: Family) -> Result<Vec<IpAddr>, Selection> {
    candidate_backends(record, family).map(|c| c.into_iter().map(|(ip, _)| ip).collect())
}

/// Smooth weighted round-robin: add each weight to its current value, take
/// the largest (earlier on a tie), subtract the total from it. The state is
/// reset when the candidate list differs from the previous call's.
fn smooth_weighted(state: &Mutex<Swrr>, list: Vec<(IpAddr, u32)>) -> usize {
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    if s.candidates != list {
        s.current = vec![0; list.len()];
        s.candidates = list;
    }
    let total: i64 = s.candidates.iter().map(|(_, w)| i64::from(*w)).sum();
    let mut best = 0;
    for i in 0..s.candidates.len() {
        s.current[i] += i64::from(s.candidates[i].1);
        if s.current[i] > s.current[best] {
            best = i;
        }
    }
    s.current[best] -= total;
    best
}

/// Choose one backend. `choice` holds the round-robin position and the
/// weighted state for this service and family; `site` is the host's site.
pub fn select(
    record: &ServiceRecord,
    family: Family,
    choice: &FamilyChoice,
    site: Option<&str>,
) -> Selection {
    select_at(record, family, choice, site, crate::schedule::unix_ms())
}

/// [`select`] at Unix time `now_ms`, which decides slow start.
pub fn select_at(
    record: &ServiceRecord,
    family: Family,
    choice: &FamilyChoice,
    site: Option<&str>,
    now_ms: u64,
) -> Selection {
    let c = match candidate_backends(record, family) {
        Ok(c) => c,
        Err(s) => return s,
    };
    let next = |len: usize| (choice.counter.fetch_add(1, Ordering::Relaxed) % len as u64) as usize;
    // Candidates in the host's site, or in the daemon's pool; all
    // candidates when none of them qualifies.
    let c = match record.balance {
        Balance::Locality | Balance::Fastest => {
            let pool = record.pool.as_deref();
            rules::subset_or_all(&c, |(_, b)| rules::preferred(record.balance, b, site, pool))
        }
        _ => c,
    };
    let slow = record.slow_start_ms;
    if record.balance != Balance::Failover && rules::any_ramping(&c, slow, now_ms, |(_, b)| b) {
        // Slow start: every method but failover goes by effective weight.
        let list = c
            .iter()
            .map(|(ip, b)| {
                let base = rules::base_weight(record.balance, b);
                (*ip, rules::effective_weight(b, base, slow, now_ms))
            })
            .collect();
        return Selection::Chosen(c[smooth_weighted(&choice.swrr, list)].0);
    }
    let ip = match record.balance {
        Balance::Failover => c[0].0,
        Balance::Weighted => {
            let list = c.iter().map(|(ip, b)| (*ip, b.weight)).collect();
            c[smooth_weighted(&choice.swrr, list)].0
        }
        // Locality and fastest take turns over the candidates kept above;
        // with no affinity key on this path, sticky behaves as round-robin.
        Balance::Locality
        | Balance::Fastest
        | Balance::RoundRobin
        | Balance::Sticky
        | Balance::Unknown => c[next(c.len())].0,
    };
    Selection::Chosen(ip)
}

/// One SRV record's data, before it becomes a DNS record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SrvTarget {
    pub ip: IpAddr,
    pub port: u16,
    pub priority: u16,
    pub weight: u16,
}

/// Every SRV candidate of a service, both families, with priority, weight
/// and order from `balance`. `rotation` is the service's SRV counter; it
/// advances once per call that returns candidates. `site` is the host's site.
pub fn srv_targets(
    record: &ServiceRecord,
    rotation: &AtomicU64,
    site: Option<&str>,
) -> Result<Vec<SrvTarget>, Selection> {
    srv_targets_at(record, rotation, site, crate::schedule::unix_ms())
}

/// [`srv_targets`] at Unix time `now_ms`, which decides slow start: while a
/// candidate ramps, every method but failover gives each record the
/// candidate's effective weight.
pub fn srv_targets_at(
    record: &ServiceRecord,
    rotation: &AtomicU64,
    site: Option<&str>,
    now_ms: u64,
) -> Result<Vec<SrvTarget>, Selection> {
    let all: Vec<(IpAddr, &BackendRecord)> = parsed(record).collect();
    if all.is_empty() {
        return Err(Selection::NoFamily);
    }
    let c = healthy_or_all(record, all)?;
    let slow = record.slow_start_ms;
    let ramping =
        record.balance != Balance::Failover && rules::any_ramping(&c, slow, now_ms, |(_, b)| b);
    let target = |(ip, b): (IpAddr, &BackendRecord), priority: u16, weight: u16| SrvTarget {
        ip,
        port: b.port,
        priority,
        weight: if ramping {
            let base = rules::base_weight(record.balance, b);
            u16::try_from(rules::effective_weight(b, base, slow, now_ms)).unwrap_or(u16::MAX)
        } else {
            weight
        },
    };
    let rotated = || {
        let start = (rotation.fetch_add(1, Ordering::Relaxed) % c.len() as u64) as usize;
        c[start..]
            .iter()
            .chain(&c[..start])
            .copied()
            .collect::<Vec<_>>()
    };
    let out = match record.balance {
        Balance::Failover => {
            rotation.fetch_add(1, Ordering::Relaxed);
            c.iter()
                .enumerate()
                .map(|(i, b)| target(*b, u16::try_from(i).unwrap_or(u16::MAX), 1))
                .collect()
        }
        Balance::Weighted => {
            rotation.fetch_add(1, Ordering::Relaxed);
            c.iter()
                .map(|b| target(*b, 0, u16::try_from(b.1.weight).unwrap_or(u16::MAX)))
                .collect()
        }
        // Local (or pool) candidates priority 0, others 1; all 0 when none
        // qualifies. Stable sort keeps the rotation within each priority.
        Balance::Locality | Balance::Fastest => {
            let pool = record.pool.as_deref().unwrap_or_default();
            let preferred = |b: &BackendRecord| match record.balance {
                Balance::Locality => site.is_some() && b.site.as_deref() == site,
                _ => pool.contains(&b.id),
            };
            let any = c.iter().any(|(_, b)| preferred(b));
            let mut list: Vec<SrvTarget> = rotated()
                .into_iter()
                .map(|b| target(b, u16::from(any && !preferred(b.1)), 1))
                .collect();
            list.sort_by_key(|t| t.priority);
            list
        }
        Balance::RoundRobin | Balance::Sticky | Balance::Unknown => {
            rotated().into_iter().map(|b| target(b, 0, 1)).collect()
        }
    };
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use HealthState::*;
    use emfor_format::{HealthState, OnAllDown};
    use std::collections::BTreeSet;

    fn rec(
        backends: &[(&str, HealthState)],
        on_all_down: OnAllDown,
        balance: Balance,
    ) -> ServiceRecord {
        let mut r = ServiceRecord {
            name: "api.svc.emfor".into(),
            on_all_down,
            balance,
            all_down: true,
            slow_start_ms: 0,
            pool: None,
            backends: backends
                .iter()
                .map(|(ip, state)| BackendRecord {
                    id: format!("{ip}:80"),
                    ip: ip.to_string(),
                    port: 80,
                    weight: 1,
                    site: None,
                    state: *state,
                    checked_at: None,
                    reason: None,
                    latency_us: None,
                    backup: false,
                    recovered_at: None,
                })
                .collect(),
        };
        r.refresh_all_down();
        r
    }

    /// `r` with backend `ip` given `reason` (failing while it is healthy).
    fn failing(mut r: ServiceRecord, ip: &str, reason: &str) -> ServiceRecord {
        let b = r
            .backends
            .iter_mut()
            .find(|b| b.ip == ip)
            .expect("the backend is in the record");
        b.reason = Some(reason.into());
        r
    }

    /// `r` with backend `ip` marked `backup`.
    fn backup(mut r: ServiceRecord, ip: &str) -> ServiceRecord {
        let b = r
            .backends
            .iter_mut()
            .find(|b| b.ip == ip)
            .expect("the backend is in the record");
        b.backup = true;
        r
    }

    fn ips(list: &[&str]) -> Vec<IpAddr> {
        list.iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn candidate_sets_example() {
        use OnAllDown::{Closed, Open};
        type Row<'a> = (
            &'a [(&'a str, HealthState)],
            OnAllDown,
            Family,
            Result<&'a [&'a str], Selection>,
        );
        let rows: &[Row] = &[
            (
                &[("10.0.0.1", Healthy), ("10.0.0.2", Unhealthy)],
                Open,
                Family::V4,
                Ok(&["10.0.0.1"]),
            ),
            (
                &[("10.0.0.1", Unhealthy), ("10.0.0.2", Unknown)],
                Open,
                Family::V4,
                Ok(&["10.0.0.2"]),
            ),
            (
                &[
                    ("10.0.0.1", Unhealthy),
                    ("10.0.0.2", Unhealthy),
                    ("2001:db8::1", Unknown),
                ],
                Open,
                Family::V4,
                Ok(&["10.0.0.1", "10.0.0.2"]),
            ),
            (
                &[("10.0.0.1", Unhealthy), ("10.0.0.2", Unknown)],
                Closed,
                Family::V4,
                Err(Selection::AllDown),
            ),
            (
                &[("10.0.0.1", Healthy)],
                Open,
                Family::V6,
                Err(Selection::NoFamily),
            ),
            (
                &[("10.0.0.1", Unhealthy), ("2001:db8::1", Healthy)],
                Closed,
                Family::V4,
                Err(Selection::AllDown),
            ),
            (
                &[("10.0.0.1", Unhealthy), ("2001:db8::1", Healthy)],
                Closed,
                Family::V6,
                Ok(&["2001:db8::1"]),
            ),
        ];
        for (backends, oad, family, expected) in rows {
            let r = rec(backends, *oad, Balance::RoundRobin);
            let got = candidates(&r, *family);
            let want = expected.map(ips);
            assert_eq!(got, want, "{backends:?} {oad:?} {family:?}");
        }
        // Rows with a failing backend (healthy, with a reason).
        let r = failing(
            rec(
                &[("10.0.0.1", Healthy), ("10.0.0.2", Healthy)],
                Closed,
                Balance::RoundRobin,
            ),
            "10.0.0.1",
            "refused",
        );
        assert_eq!(candidates(&r, Family::V4), Ok(ips(&["10.0.0.2"])));
        let r = failing(
            rec(
                &[
                    ("10.0.0.1", Healthy),
                    ("10.0.0.2", Unhealthy),
                    ("2001:db8::1", Healthy),
                ],
                Closed,
                Balance::RoundRobin,
            ),
            "10.0.0.1",
            "refused",
        );
        assert_eq!(candidates(&r, Family::V4), Ok(ips(&["10.0.0.1"])));
        // Open with every unknown backend failing: all unknown ones.
        let r = rec(
            &[
                ("10.0.0.1", Unhealthy),
                ("10.0.0.2", Unknown),
                ("10.0.0.3", Unknown),
            ],
            Open,
            Balance::RoundRobin,
        );
        let r = failing(failing(r, "10.0.0.2", "timeout"), "10.0.0.3", "refused");
        assert_eq!(
            candidates(&r, Family::V4),
            Ok(ips(&["10.0.0.2", "10.0.0.3"]))
        );
    }

    /// Spec "Candidate backends per address family", scenario "Backup used
    /// while no primary passes".
    #[test]
    fn backup_used_while_no_primary_passes() {
        let r = rec(
            &[
                ("10.0.0.1", Healthy),
                ("10.0.0.2", Unhealthy),
                ("10.0.1.1", Healthy),
            ],
            OnAllDown::Closed,
            Balance::RoundRobin,
        );
        let r = backup(failing(r, "10.0.0.1", "timeout"), "10.0.1.1");
        assert_eq!(candidates(&r, Family::V4), Ok(ips(&["10.0.1.1"])));
    }

    /// Spec "Candidate backends per address family", scenario "Passing
    /// primary wins over a backup".
    #[test]
    fn passing_primary_wins_over_a_backup() {
        let r = rec(
            &[("10.0.0.1", Healthy), ("10.0.1.1", Healthy)],
            OnAllDown::Closed,
            Balance::RoundRobin,
        );
        let r = backup(r, "10.0.1.1");
        assert_eq!(candidates(&r, Family::V4), Ok(ips(&["10.0.0.1"])));
    }

    /// Spec "Candidate backends per address family", example "candidate
    /// sets", the rows with a backup (10.0.1.1).
    #[test]
    fn backup_candidate_sets() {
        use OnAllDown::{Closed, Open};
        let two = |a: HealthState, b: HealthState, oad| {
            backup(
                rec(
                    &[("10.0.0.1", a), ("10.0.1.1", b)],
                    oad,
                    Balance::RoundRobin,
                ),
                "10.0.1.1",
            )
        };
        let r = failing(
            failing(two(Healthy, Healthy, Closed), "10.0.0.1", "timeout"),
            "10.0.1.1",
            "timeout",
        );
        assert_eq!(candidates(&r, Family::V4), Ok(ips(&["10.0.0.1"])));
        let r = failing(two(Unhealthy, Healthy, Closed), "10.0.1.1", "timeout");
        assert_eq!(candidates(&r, Family::V4), Ok(ips(&["10.0.1.1"])));
        let r = two(Unhealthy, Unknown, Open);
        assert_eq!(candidates(&r, Family::V4), Ok(ips(&["10.0.1.1"])));
        let r = two(Unhealthy, Unhealthy, Open);
        assert_eq!(
            candidates(&r, Family::V4),
            Ok(ips(&["10.0.0.1", "10.0.1.1"]))
        );
    }

    /// Spec "Candidate backends per address family", scenario "Open
    /// service prefers unknown backends".
    #[test]
    fn open_prefers_unknown_backends() {
        let r = rec(
            &[
                ("10.0.0.1", Unhealthy),
                ("10.0.0.2", Unknown),
                ("10.0.0.3", Unknown),
            ],
            OnAllDown::Open,
            Balance::RoundRobin,
        );
        let r = failing(r, "10.0.0.2", "timeout");
        assert_eq!(candidates(&r, Family::V4), Ok(ips(&["10.0.0.3"])));
    }

    /// Spec "Candidate backends per address family", scenario "Failing
    /// backends are skipped while another passes".
    #[test]
    fn failing_backends_are_skipped() {
        let r = failing(
            rec(
                &[("10.0.0.1", Healthy), ("10.0.0.2", Healthy)],
                OnAllDown::Open,
                Balance::RoundRobin,
            ),
            "10.0.0.1",
            "connect: Connection refused",
        );
        let c = FamilyChoice::default();
        for _ in 0..4 {
            assert_eq!(
                select(&r, Family::V4, &c, None),
                Selection::Chosen("10.0.0.2".parse().unwrap())
            );
        }
    }

    /// Spec "Candidate backends per address family", scenario "Every
    /// healthy backend failing".
    #[test]
    fn every_healthy_backend_failing() {
        let r = rec(
            &[
                ("10.0.0.1", Healthy),
                ("10.0.0.2", Healthy),
                ("10.0.0.3", Unhealthy),
            ],
            OnAllDown::Closed,
            Balance::RoundRobin,
        );
        let r = failing(failing(r, "10.0.0.1", "timeout"), "10.0.0.2", "timeout");
        assert_eq!(
            candidates(&r, Family::V4),
            Ok(ips(&["10.0.0.1", "10.0.0.2"]))
        );
        let c = FamilyChoice::default();
        let picks: Vec<_> = (0..4).map(|_| select(&r, Family::V4, &c, None)).collect();
        let a = Selection::Chosen("10.0.0.1".parse().unwrap());
        let b = Selection::Chosen("10.0.0.2".parse().unwrap());
        assert_eq!(picks, [a, b, a, b]);
    }

    #[test]
    fn unhealthy_backends_are_skipped() {
        let r = rec(
            &[("10.0.0.1", Healthy), ("10.0.0.2", Unhealthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
        );
        let c = FamilyChoice::default();
        for _ in 0..4 {
            assert_eq!(
                select(&r, Family::V4, &c, None),
                Selection::Chosen("10.0.0.1".parse().unwrap())
            );
        }
    }

    #[test]
    fn round_robin_alternates() {
        let r = rec(
            &[("10.0.0.1", Healthy), ("10.0.0.2", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
        );
        let c = FamilyChoice::default();
        let picks: Vec<_> = (0..4).map(|_| select(&r, Family::V4, &c, None)).collect();
        let a = Selection::Chosen("10.0.0.1".parse().unwrap());
        let b = Selection::Chosen("10.0.0.2".parse().unwrap());
        assert_eq!(picks, [a, b, a, b]);
    }

    #[test]
    fn failover_order_example() {
        use OnAllDown::{Closed, Open};
        let rows: &[([HealthState; 3], OnAllDown, Option<&str>)] = &[
            ([Healthy, Healthy, Healthy], Open, Some("10.0.0.1")),
            ([Unhealthy, Healthy, Healthy], Open, Some("10.0.0.2")),
            ([Unhealthy, Unhealthy, Healthy], Open, Some("10.0.0.3")),
            ([Unhealthy, Unhealthy, Unhealthy], Open, Some("10.0.0.1")),
            ([Unhealthy, Unhealthy, Unhealthy], Closed, None),
        ];
        for (states, oad, want) in rows {
            let r = rec(
                &[
                    ("10.0.0.1", states[0]),
                    ("10.0.0.2", states[1]),
                    ("10.0.0.3", states[2]),
                ],
                *oad,
                Balance::Failover,
            );
            let c = FamilyChoice::default();
            let want = match want {
                Some(ip) => Selection::Chosen(ip.parse().unwrap()),
                None => Selection::AllDown,
            };
            for _ in 0..3 {
                assert_eq!(select(&r, Family::V4, &c, None), want, "{states:?} {oad:?}");
            }
        }
    }

    #[test]
    fn sticky_without_key_rotates() {
        let r = rec(
            &[("10.0.0.1", Healthy), ("10.0.0.2", Healthy)],
            OnAllDown::Open,
            Balance::Sticky,
        );
        let c = FamilyChoice::default();
        assert_ne!(
            select(&r, Family::V4, &c, None),
            select(&r, Family::V4, &c, None)
        );
    }

    fn chosen(ip: &str) -> Selection {
        Selection::Chosen(ip.parse().unwrap())
    }

    /// Backends as (ip, weight, site, state).
    fn rec_ws(
        backends: &[(&str, u32, Option<&str>, HealthState)],
        on_all_down: OnAllDown,
        balance: Balance,
    ) -> ServiceRecord {
        let plain: Vec<(&str, HealthState)> = backends.iter().map(|b| (b.0, b.3)).collect();
        let mut r = rec(&plain, on_all_down, balance);
        for (b, (_, w, site, _)) in r.backends.iter_mut().zip(backends) {
            b.weight = *w;
            b.site = site.map(str::to_string);
        }
        r
    }

    #[test]
    fn weighted_answer_order_example() {
        /// (ip, weight) in config order, then the expected answers.
        type Row<'a> = (&'a [(&'a str, u32)], &'a [&'a str]);
        let rows: &[Row] = &[
            (
                &[("10.0.0.1", 3), ("10.0.0.2", 1)],
                &["10.0.0.1", "10.0.0.1", "10.0.0.2", "10.0.0.1"],
            ),
            (
                &[("10.0.0.1", 1), ("10.0.0.2", 3), ("10.0.0.3", 1)],
                &["10.0.0.2", "10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.2"],
            ),
            (
                &[("10.0.0.1", 1), ("10.0.0.2", 1)],
                &["10.0.0.1", "10.0.0.2", "10.0.0.1", "10.0.0.2"],
            ),
        ];
        for (weights, want) in rows {
            let b: Vec<_> = weights
                .iter()
                .map(|(ip, w)| (*ip, *w, None, Healthy))
                .collect();
            let r = rec_ws(&b, OnAllDown::Open, Balance::Weighted);
            let c = FamilyChoice::default();
            // Two cycles: the second repeats the first.
            for round in 0..2 {
                let got: Vec<_> = want
                    .iter()
                    .map(|_| select(&r, Family::V4, &c, None))
                    .collect();
                let want: Vec<_> = want.iter().map(|ip| chosen(ip)).collect();
                assert_eq!(got, want, "{weights:?} round {round}");
            }
        }
    }

    #[test]
    fn weighted_skips_unhealthy_heavy_backend() {
        let r = rec_ws(
            &[
                ("10.0.0.1", 3, None, Unhealthy),
                ("10.0.0.2", 1, None, Healthy),
            ],
            OnAllDown::Open,
            Balance::Weighted,
        );
        let c = FamilyChoice::default();
        for _ in 0..4 {
            assert_eq!(select(&r, Family::V4, &c, None), chosen("10.0.0.2"));
        }
    }

    #[test]
    fn weighted_state_resets_when_candidates_change() {
        let both = rec_ws(
            &[
                ("10.0.0.1", 3, None, Healthy),
                ("10.0.0.2", 1, None, Healthy),
            ],
            OnAllDown::Open,
            Balance::Weighted,
        );
        let c = FamilyChoice::default();
        assert_eq!(select(&both, Family::V4, &c, None), chosen("10.0.0.1"));
        let one = rec_ws(
            &[
                ("10.0.0.1", 3, None, Unhealthy),
                ("10.0.0.2", 1, None, Healthy),
            ],
            OnAllDown::Open,
            Balance::Weighted,
        );
        assert_eq!(select(&one, Family::V4, &c, None), chosen("10.0.0.2"));
        // Back to both: a reset state starts the cycle again.
        let got: Vec<_> = (0..4)
            .map(|_| select(&both, Family::V4, &c, None))
            .collect();
        let a = chosen("10.0.0.1");
        let b = chosen("10.0.0.2");
        assert_eq!(got, [a, a, b, a]);
    }

    #[test]
    fn locality_outcomes_example() {
        use OnAllDown::{Closed, Open};
        type Row<'a> = (
            Option<&'a str>,
            [(&'a str, &'a str, HealthState); 2],
            OnAllDown,
            &'a [&'a str],
        );
        let rows: &[Row] = &[
            (
                Some("dc1"),
                [("10.0.0.1", "dc1", Healthy), ("10.0.0.3", "dc2", Healthy)],
                Open,
                &["10.0.0.1"],
            ),
            (
                Some("dc1"),
                [("10.0.0.1", "dc1", Unhealthy), ("10.0.0.3", "dc2", Healthy)],
                Open,
                &["10.0.0.3"],
            ),
            (
                Some("dc1"),
                [
                    ("10.0.0.1", "dc1", Unhealthy),
                    ("10.0.0.3", "dc2", Unhealthy),
                ],
                Open,
                &["10.0.0.1"],
            ),
            (
                Some("dc1"),
                [
                    ("10.0.0.1", "dc1", Unhealthy),
                    ("10.0.0.3", "dc2", Unhealthy),
                ],
                Closed,
                &[],
            ),
            (
                None,
                [("10.0.0.1", "dc1", Healthy), ("10.0.0.3", "dc2", Healthy)],
                Open,
                &["10.0.0.1", "10.0.0.3"],
            ),
            (
                Some("dc3"),
                [("10.0.0.1", "dc1", Healthy), ("10.0.0.3", "dc2", Healthy)],
                Open,
                &["10.0.0.1", "10.0.0.3"],
            ),
        ];
        for (site, backends, oad, want) in rows {
            let b: Vec<_> = backends
                .iter()
                .map(|(ip, s, st)| (*ip, 1, Some(*s), *st))
                .collect();
            let r = rec_ws(&b, *oad, Balance::Locality);
            let c = FamilyChoice::default();
            let got: Vec<_> = (0..4).map(|_| select(&r, Family::V4, &c, *site)).collect();
            let expected: Vec<Selection> = if want.is_empty() {
                vec![Selection::AllDown; 4]
            } else {
                (0..4).map(|i| chosen(want[i % want.len()])).collect()
            };
            assert_eq!(got, expected, "{site:?} {backends:?} {oad:?}");
        }
    }

    #[test]
    fn local_backends_preferred() {
        let r = rec_ws(
            &[
                ("10.0.0.1", 1, Some("dc1"), Healthy),
                ("10.0.0.2", 1, Some("dc1"), Healthy),
                ("10.0.0.3", 1, Some("dc2"), Healthy),
            ],
            OnAllDown::Open,
            Balance::Locality,
        );
        let c = FamilyChoice::default();
        let got: Vec<_> = (0..4)
            .map(|_| select(&r, Family::V4, &c, Some("dc1")))
            .collect();
        let (a, b) = (chosen("10.0.0.1"), chosen("10.0.0.2"));
        assert_eq!(got, [a, b, a, b]);
    }

    /// Backend `ip:port` with state, weight and site.
    fn srv_rec(
        backends: &[(&str, u16, HealthState, u32, Option<&str>)],
        on_all_down: OnAllDown,
        balance: Balance,
    ) -> ServiceRecord {
        let mut r = rec(&[], on_all_down, balance);
        r.backends = backends
            .iter()
            .map(|(ip, port, state, weight, site)| BackendRecord {
                id: if ip.contains(':') {
                    format!("[{ip}]:{port}")
                } else {
                    format!("{ip}:{port}")
                },
                ip: ip.to_string(),
                port: *port,
                weight: *weight,
                site: site.map(str::to_string),
                state: *state,
                checked_at: None,
                reason: None,
                latency_us: None,
                backup: false,
                recovered_at: None,
            })
            .collect();
        r.refresh_all_down();
        r
    }

    /// `(ip, port, priority, weight)` per record.
    fn srv(
        r: &ServiceRecord,
        n: &AtomicU64,
        site: Option<&str>,
    ) -> Result<Vec<(String, u16, u16, u16)>, Selection> {
        srv_targets(r, n, site).map(|l| {
            l.into_iter()
                .map(|t| (t.ip.to_string(), t.port, t.priority, t.weight))
                .collect()
        })
    }

    fn row(ip: &str, port: u16, priority: u16, weight: u16) -> (String, u16, u16, u16) {
        (ip.to_string(), port, priority, weight)
    }

    #[test]
    fn srv_failover_priorities() {
        let r = srv_rec(
            &[
                ("10.0.0.11", 8080, Unhealthy, 1, None),
                ("10.0.0.12", 8080, Healthy, 1, None),
                ("10.0.0.13", 8080, Healthy, 1, None),
            ],
            OnAllDown::Open,
            Balance::Failover,
        );
        let n = AtomicU64::new(0);
        for _ in 0..2 {
            assert_eq!(
                srv(&r, &n, None).unwrap(),
                vec![row("10.0.0.12", 8080, 0, 1), row("10.0.0.13", 8080, 1, 1)]
            );
        }
    }

    #[test]
    fn srv_weighted_weights() {
        let r = srv_rec(
            &[
                ("10.0.0.11", 8080, Healthy, 1, None),
                ("10.0.0.12", 8080, Healthy, 3, None),
                ("10.0.0.13", 8080, Healthy, 1, None),
            ],
            OnAllDown::Open,
            Balance::Weighted,
        );
        let n = AtomicU64::new(0);
        for _ in 0..3 {
            assert_eq!(
                srv(&r, &n, None).unwrap(),
                vec![
                    row("10.0.0.11", 8080, 0, 1),
                    row("10.0.0.12", 8080, 0, 3),
                    row("10.0.0.13", 8080, 0, 1)
                ]
            );
        }
    }

    #[test]
    fn srv_round_robin_rotation() {
        for balance in [Balance::RoundRobin, Balance::Sticky, Balance::Unknown] {
            let r = srv_rec(
                &[
                    ("10.0.0.1", 80, Healthy, 1, None),
                    ("10.0.0.2", 80, Healthy, 1, None),
                    ("10.0.0.3", 80, Healthy, 1, None),
                ],
                OnAllDown::Open,
                balance,
            );
            let n = AtomicU64::new(0);
            let order = |l: Vec<(String, u16, u16, u16)>| {
                assert!(l.iter().all(|t| t.2 == 0 && t.3 == 1));
                l.into_iter().map(|t| t.0).collect::<Vec<_>>()
            };
            assert_eq!(
                order(srv(&r, &n, None).unwrap()),
                ["10.0.0.1", "10.0.0.2", "10.0.0.3"]
            );
            assert_eq!(
                order(srv(&r, &n, None).unwrap()),
                ["10.0.0.2", "10.0.0.3", "10.0.0.1"]
            );
            assert_eq!(
                order(srv(&r, &n, None).unwrap()),
                ["10.0.0.3", "10.0.0.1", "10.0.0.2"]
            );
        }
    }

    #[test]
    fn srv_locality_priorities() {
        let r = srv_rec(
            &[
                ("10.0.0.11", 8080, Healthy, 1, Some("dc1")),
                ("10.0.1.11", 8080, Healthy, 1, Some("dc2")),
                ("10.0.0.12", 8080, Healthy, 1, Some("dc1")),
            ],
            OnAllDown::Open,
            Balance::Locality,
        );
        let n = AtomicU64::new(0);
        for _ in 0..3 {
            let l = srv(&r, &n, Some("dc1")).unwrap();
            assert_eq!(l.len(), 3);
            assert!(l[0].0.starts_with("10.0.0.") && l[0].2 == 0);
            assert!(l[1].0.starts_with("10.0.0.") && l[1].2 == 0);
            assert_eq!(l[2], row("10.0.1.11", 8080, 1, 1));
        }
        // Rotation still spreads the local backends.
        let firsts: BTreeSet<String> = (0..2)
            .map(|_| srv(&r, &n, Some("dc1")).unwrap()[0].0.clone())
            .collect();
        assert_eq!(firsts.len(), 2);

        // Example: locality priorities by host site (priorities in config order).
        let two = srv_rec(
            &[
                ("10.0.0.11", 8080, Healthy, 1, Some("dc1")),
                ("10.0.1.11", 8080, Healthy, 1, Some("dc2")),
            ],
            OnAllDown::Open,
            Balance::Locality,
        );
        for site in [Some("dc3"), None] {
            let l = srv(&two, &AtomicU64::new(0), site).unwrap();
            assert!(l.iter().all(|t| t.2 == 0), "{site:?}: {l:?}");
        }
        let l = srv(&r, &AtomicU64::new(0), Some("dc1")).unwrap();
        let prio = |ip: &str| l.iter().find(|t| t.0 == ip).unwrap().2;
        assert_eq!(
            [prio("10.0.0.11"), prio("10.0.1.11"), prio("10.0.0.12")],
            [0, 1, 0]
        );
    }

    #[test]
    fn srv_health_across_families_and_all_down() {
        let mixed = srv_rec(
            &[
                ("10.0.0.11", 8080, Unhealthy, 1, None),
                ("2001:db8::11", 8080, Healthy, 1, None),
            ],
            OnAllDown::Closed,
            Balance::RoundRobin,
        );
        assert_eq!(
            srv(&mixed, &AtomicU64::new(0), None).unwrap(),
            vec![row("2001:db8::11", 8080, 0, 1)]
        );

        let down = [
            ("10.0.0.11", 8080, Unhealthy, 1, None),
            ("2001:db8::11", 8080, Unknown, 1, None),
        ];
        let closed = srv_rec(&down, OnAllDown::Closed, Balance::RoundRobin);
        let n = AtomicU64::new(0);
        assert_eq!(srv(&closed, &n, None), Err(Selection::AllDown));
        assert_eq!(n.load(Ordering::Relaxed), 0, "no answer, no rotation");

        // Scenario "All down open with an unknown backend".
        let open = srv_rec(&down, OnAllDown::Open, Balance::RoundRobin);
        assert_eq!(
            srv(&open, &AtomicU64::new(0), None).unwrap(),
            vec![row("2001:db8::11", 8080, 0, 1)]
        );

        // Scenario "All down open": every backend unhealthy lists them all.
        let all_down = [
            ("10.0.0.11", 8080, Unhealthy, 1, None),
            ("2001:db8::11", 8080, Unhealthy, 1, None),
        ];
        let open = srv_rec(&all_down, OnAllDown::Open, Balance::RoundRobin);
        assert_eq!(
            srv(&open, &AtomicU64::new(0), None).unwrap(),
            vec![
                row("10.0.0.11", 8080, 0, 1),
                row("2001:db8::11", 8080, 0, 1)
            ]
        );
    }

    /// Spec "SRV answers", scenario "Failing backend left out".
    #[test]
    fn srv_failing_backend_left_out() {
        let r = srv_rec(
            &[
                ("10.0.0.11", 8080, Healthy, 1, None),
                ("2001:db8::11", 8080, Healthy, 1, None),
            ],
            OnAllDown::Open,
            Balance::RoundRobin,
        );
        let r = failing(r, "10.0.0.11", "connect: Connection refused");
        let n = AtomicU64::new(0);
        for _ in 0..2 {
            assert_eq!(
                srv(&r, &n, None).unwrap(),
                vec![row("2001:db8::11", 8080, 0, 1)]
            );
        }
    }

    /// Spec "SRV answers", scenario "Every healthy backend failing in SRV".
    #[test]
    fn srv_every_healthy_backend_failing() {
        let r = srv_rec(
            &[
                ("10.0.0.11", 8080, Unhealthy, 1, None),
                ("10.0.0.12", 8080, Healthy, 1, None),
                ("10.0.0.13", 8080, Healthy, 1, None),
            ],
            OnAllDown::Open,
            Balance::Failover,
        );
        let r = failing(failing(r, "10.0.0.12", "timeout"), "10.0.0.13", "timeout");
        assert_eq!(
            srv(&r, &AtomicU64::new(0), None).unwrap(),
            vec![row("10.0.0.12", 8080, 0, 1), row("10.0.0.13", 8080, 1, 1)]
        );
    }

    /// Spec "SRV answers", scenario "Backups listed only while no primary
    /// passes".
    #[test]
    fn srv_backups_listed_only_while_no_primary_passes() {
        let r = srv_rec(
            &[
                ("10.0.0.11", 8080, Healthy, 1, None),
                ("10.0.1.11", 8080, Healthy, 1, None),
                ("10.0.1.12", 8080, Healthy, 1, None),
            ],
            OnAllDown::Closed,
            Balance::Failover,
        );
        let r = backup(backup(r, "10.0.1.11"), "10.0.1.12");
        let down = failing(r.clone(), "10.0.0.11", "timeout");
        assert_eq!(
            srv(&down, &AtomicU64::new(0), None).unwrap(),
            vec![row("10.0.1.11", 8080, 0, 1), row("10.0.1.12", 8080, 1, 1)]
        );
        assert_eq!(
            srv(&r, &AtomicU64::new(0), None).unwrap(),
            vec![row("10.0.0.11", 8080, 0, 1)]
        );
    }

    /// Unix time the slow start tests run at.
    const NOW: u64 = 1_762_905_700_000;

    /// `r` with a 100 s slow start and backend `ip` recovered so that its
    /// step at [`NOW`] is `step` (1 to 9).
    fn ramping(mut r: ServiceRecord, ip: &str, step: u64) -> ServiceRecord {
        r.slow_start_ms = 100_000;
        let b = r
            .backends
            .iter_mut()
            .find(|b| b.ip == ip)
            .expect("the backend is in the record");
        b.recovered_at = Some(NOW - (step - 1) * 10_000 - 5_000);
        r
    }

    /// How often each address is chosen in `n` IPv4 answers at [`NOW`].
    fn counts(r: &ServiceRecord, n: usize) -> std::collections::BTreeMap<String, usize> {
        counts_at_site(r, None, n)
    }

    /// [`counts`] on a host whose site is `site`.
    fn counts_at_site(
        r: &ServiceRecord,
        site: Option<&str>,
        n: usize,
    ) -> std::collections::BTreeMap<String, usize> {
        let c = FamilyChoice::default();
        let mut out = std::collections::BTreeMap::new();
        for _ in 0..n {
            let Selection::Chosen(ip) = select_at(r, Family::V4, &c, site, NOW) else {
                panic!("no answer");
            };
            *out.entry(ip.to_string()).or_insert(0) += 1;
        }
        out
    }

    fn count_map(list: &[(&str, usize)]) -> std::collections::BTreeMap<String, usize> {
        list.iter().map(|(ip, n)| (ip.to_string(), *n)).collect()
    }

    /// `locality` narrows to the host's site first, then uses the reduced
    /// weights there; the backend at another site gets nothing.
    #[test]
    fn slow_start_locality_within_the_site() {
        let mut r = rec(
            &[
                ("10.0.0.1", Healthy),
                ("10.0.0.2", Healthy),
                ("10.0.0.3", Healthy),
            ],
            OnAllDown::Open,
            Balance::Locality,
        );
        for b in &mut r.backends[..2] {
            b.site = Some("tpe".into());
        }
        let r = ramping(r, "10.0.0.1", 3);
        assert_eq!(
            counts_at_site(&r, Some("tpe"), 13),
            count_map(&[("10.0.0.1", 3), ("10.0.0.2", 10)])
        );
    }

    /// Spec "Slow start choice in the daemon", scenario "Ramping backend
    /// gets a smaller share".
    #[test]
    fn slow_start_ramping_share() {
        let r = rec(
            &[("10.0.0.1", Healthy), ("10.0.0.2", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
        );
        let r = ramping(r, "10.0.0.2", 3);
        assert_eq!(
            counts(&r, 13),
            count_map(&[("10.0.0.1", 10), ("10.0.0.2", 3)])
        );
        // Once the slow start is over, plain turns again.
        let c = FamilyChoice::default();
        let later: Vec<_> = (0..4)
            .map(|_| select_at(&r, Family::V4, &c, None, NOW + 100_000))
            .collect();
        let (a, b) = (chosen("10.0.0.1"), chosen("10.0.0.2"));
        assert_eq!(later, [a, b, a, b]);
    }

    /// Spec "Slow start choice in the daemon", scenario "Failover ignores
    /// slow start".
    #[test]
    fn slow_start_failover_unchanged() {
        let r = rec(
            &[("10.0.0.1", Healthy), ("10.0.0.2", Healthy)],
            OnAllDown::Open,
            Balance::Failover,
        );
        let r = ramping(r, "10.0.0.1", 1);
        assert_eq!(counts(&r, 5), count_map(&[("10.0.0.1", 5)]));
    }

    /// Spec "Slow start choice in the daemon", scenario "Weighted ramping
    /// backend".
    #[test]
    fn slow_start_weighted_ramping() {
        let mut r = rec(
            &[("10.0.0.1", Healthy), ("10.0.0.2", Healthy)],
            OnAllDown::Open,
            Balance::Weighted,
        );
        r.backends[0].weight = 3;
        let r = ramping(r, "10.0.0.1", 2);
        assert_eq!(
            counts(&r, 16),
            count_map(&[("10.0.0.1", 6), ("10.0.0.2", 10)])
        );
    }

    fn srv_at(r: &ServiceRecord) -> Vec<(String, u16, u16, u16)> {
        srv_targets_at(r, &AtomicU64::new(0), None, NOW)
            .unwrap()
            .into_iter()
            .map(|t| (t.ip.to_string(), t.port, t.priority, t.weight))
            .collect()
    }

    /// Spec "SRV weights during slow start", scenario "Round-robin with a
    /// ramping backend".
    #[test]
    fn srv_slow_start_round_robin() {
        let r = srv_rec(
            &[
                ("10.0.0.11", 8080, Healthy, 1, None),
                ("10.0.0.12", 8080, Healthy, 1, None),
            ],
            OnAllDown::Open,
            Balance::RoundRobin,
        );
        let r = ramping(r, "10.0.0.12", 3);
        assert_eq!(
            srv_at(&r),
            vec![row("10.0.0.11", 8080, 0, 10), row("10.0.0.12", 8080, 0, 3)]
        );
    }

    /// Spec "SRV weights during slow start", scenario "Weighted with a
    /// ramping backend".
    #[test]
    fn srv_slow_start_weighted() {
        let r = srv_rec(
            &[
                ("10.0.0.11", 8080, Healthy, 3, None),
                ("10.0.0.12", 8080, Healthy, 1, None),
            ],
            OnAllDown::Open,
            Balance::Weighted,
        );
        let r = ramping(r, "10.0.0.11", 2);
        let weights: Vec<u16> = srv_at(&r).iter().map(|t| t.3).collect();
        assert_eq!(weights, [6, 10]);
    }

    /// Spec "SRV weights during slow start", scenario "Failover keeps
    /// weight 1".
    #[test]
    fn srv_slow_start_failover() {
        let r = srv_rec(
            &[
                ("10.0.0.11", 8080, Healthy, 1, None),
                ("10.0.0.12", 8080, Healthy, 1, None),
            ],
            OnAllDown::Open,
            Balance::Failover,
        );
        let r = ramping(r, "10.0.0.11", 2);
        assert_eq!(
            srv_at(&r),
            vec![row("10.0.0.11", 8080, 0, 1), row("10.0.0.12", 8080, 1, 1)]
        );
    }

    #[test]
    fn srv_counter_is_separate_from_family_counters() {
        let r = srv_rec(
            &[
                ("10.0.0.1", 80, Healthy, 1, None),
                ("10.0.0.2", 80, Healthy, 1, None),
            ],
            OnAllDown::Open,
            Balance::RoundRobin,
        );
        let rr = crate::live::RoundRobin::default();
        for _ in 0..3 {
            srv_targets(&r, &rr.srv, None).unwrap();
        }
        assert_eq!(rr.srv.load(Ordering::Relaxed), 3);
        assert_eq!(rr.for_family(Family::V4).counter.load(Ordering::Relaxed), 0);
    }

    fn fastest(backends: &[(&str, HealthState)], pool: &[&str]) -> ServiceRecord {
        let mut r = rec(backends, OnAllDown::Open, Balance::Fastest);
        r.pool = Some(pool.iter().map(|ip| format!("{ip}:80")).collect());
        r
    }

    fn four_a(r: &ServiceRecord) -> Vec<Selection> {
        let c = FamilyChoice::default();
        (0..4).map(|_| select(r, Family::V4, &c, None)).collect()
    }

    #[test]
    fn fastest_backends_preferred() {
        let r = fastest(
            &[
                ("10.0.0.1", Healthy),
                ("10.0.0.2", Healthy),
                ("10.0.0.3", Healthy),
            ],
            &["10.0.0.1", "10.0.0.2"],
        );
        let (a, b) = (chosen("10.0.0.1"), chosen("10.0.0.2"));
        assert_eq!(four_a(&r), [a, b, a, b]);
    }

    #[test]
    fn fastest_empty_pool_rotates_over_all() {
        let r = fastest(&[("10.0.0.1", Healthy), ("10.0.0.2", Healthy)], &[]);
        let got = four_a(&r);
        assert_ne!(got[0], got[1]);
        let mut missing = rec(
            &[("10.0.0.1", Healthy), ("10.0.0.2", Healthy)],
            OnAllDown::Open,
            Balance::Fastest,
        );
        missing.pool = None;
        assert_eq!(four_a(&missing), got);
    }

    /// A `fastest` record whose pool the daemon computed from latencies:
    /// primaries 10.0.0.1 (20 ms) and 10.0.0.2 (100 ms), backup 10.0.1.1
    /// (2 ms), all passing.
    fn fastest_with_fast_backup() -> ServiceRecord {
        let mut r = srv_rec(
            &[
                ("10.0.0.1", 8080, Healthy, 1, None),
                ("10.0.0.2", 8080, Healthy, 1, None),
                ("10.0.1.1", 8080, Healthy, 1, None),
            ],
            OnAllDown::Open,
            Balance::Fastest,
        );
        r = backup(r, "10.0.1.1");
        for (b, l) in r.backends.iter_mut().zip([20_000, 100_000, 2_000]) {
            b.latency_us = Some(l);
        }
        r.pool = Some(crate::state::fast_pool(&r.backends, &[]));
        r
    }

    #[test]
    fn fastest_with_a_fast_backup_keeps_preferring_fast_primaries() {
        let r = fastest_with_fast_backup();
        let a = chosen("10.0.0.1");
        assert_eq!(four_a(&r), [a, a, a, a]);
        assert_eq!(
            srv(&r, &AtomicU64::new(0), None).unwrap(),
            vec![row("10.0.0.1", 8080, 0, 1), row("10.0.0.2", 8080, 1, 1)]
        );
    }

    #[test]
    fn fastest_all_down_open() {
        let r = fastest(&[("10.0.0.1", Unhealthy), ("10.0.0.2", Unhealthy)], &[]);
        let (a, b) = (chosen("10.0.0.1"), chosen("10.0.0.2"));
        assert_eq!(four_a(&r), [a, b, a, b]);
    }

    #[test]
    fn fastest_pool_without_the_family() {
        let r = fastest(
            &[("10.0.0.1", Healthy), ("2001:db8::1", Healthy)],
            &["2001:db8::1"],
        );
        // The ids built by `rec` are `<ip>:80`; the pool names the IPv6 one.
        let a = chosen("10.0.0.1");
        assert_eq!(four_a(&r)[..2], [a, a]);
    }

    #[test]
    fn srv_fastest_priorities() {
        let mut r = srv_rec(
            &[
                ("10.0.0.11", 8080, Healthy, 1, None),
                ("10.0.0.12", 8080, Healthy, 1, None),
                ("2001:db8::11", 8080, Healthy, 1, None),
            ],
            OnAllDown::Open,
            Balance::Fastest,
        );
        r.pool = Some(vec!["10.0.0.11:8080".into(), "[2001:db8::11]:8080".into()]);
        let n = AtomicU64::new(0);
        let mut firsts = BTreeSet::new();
        for _ in 0..3 {
            let l = srv(&r, &n, None).unwrap();
            assert_eq!(l.len(), 3);
            let fast: BTreeSet<&str> = l[..2].iter().map(|t| t.0.as_str()).collect();
            assert_eq!(fast, BTreeSet::from(["10.0.0.11", "2001:db8::11"]));
            assert!(l[..2].iter().all(|t| t.2 == 0 && t.3 == 1));
            assert_eq!(l[2], row("10.0.0.12", 8080, 1, 1));
            firsts.insert(l[0].0.clone());
        }
        assert_eq!(firsts.len(), 2, "rotation spreads the pool");
    }

    #[test]
    fn srv_fastest_empty_pool_all_priority_0() {
        let mut r = srv_rec(
            &[
                ("10.0.0.11", 8080, Healthy, 1, None),
                ("10.0.0.12", 8080, Healthy, 1, None),
            ],
            OnAllDown::Open,
            Balance::Fastest,
        );
        for pool in [Some(vec![]), None] {
            r.pool = pool;
            let l = srv(&r, &AtomicU64::new(0), None).unwrap();
            assert_eq!(
                l,
                [row("10.0.0.11", 8080, 0, 1), row("10.0.0.12", 8080, 0, 1)]
            );
        }
    }
}
