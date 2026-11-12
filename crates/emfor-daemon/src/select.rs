//! Choosing one backend of a service without an affinity key.
//!
//! This is the reference for the selection rules that every language client
//! must follow: candidates per address family, per-family all-down handling,
//! then `balance`. No I/O.

use std::net::IpAddr;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use emfor_format::{BackendRecord, Balance, HealthState, OnAllDown, ServiceRecord};

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
    let of_family: Vec<(IpAddr, &BackendRecord)> = record
        .backends
        .iter()
        .filter_map(|b| b.ip.parse::<IpAddr>().ok().map(|ip| (ip, b)))
        .filter(|(ip, _)| family.matches(ip))
        .collect();
    if of_family.is_empty() {
        return Err(Selection::NoFamily);
    }
    let healthy: Vec<(IpAddr, &BackendRecord)> = of_family
        .iter()
        .filter(|(_, b)| b.state == HealthState::Healthy)
        .copied()
        .collect();
    if !healthy.is_empty() {
        return Ok(healthy);
    }
    match record.on_all_down {
        OnAllDown::Open => Ok(of_family),
        OnAllDown::Closed => Err(Selection::AllDown),
    }
}

/// Addresses that may be chosen for `family`, in config order, or why none may.
pub fn candidates(record: &ServiceRecord, family: Family) -> Result<Vec<IpAddr>, Selection> {
    candidate_backends(record, family).map(|c| c.into_iter().map(|(ip, _)| ip).collect())
}

/// Smooth weighted round-robin: add each weight to its current value, take
/// the largest (earlier on a tie), subtract the total from it. The state is
/// reset when the candidate list differs from the previous call's.
fn smooth_weighted(state: &Mutex<Swrr>, candidates: &[(IpAddr, &BackendRecord)]) -> usize {
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    let list: Vec<(IpAddr, u32)> = candidates.iter().map(|(ip, b)| (*ip, b.weight)).collect();
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
    let c = match candidate_backends(record, family) {
        Ok(c) => c,
        Err(s) => return s,
    };
    let next = |len: usize| (choice.counter.fetch_add(1, Ordering::Relaxed) % len as u64) as usize;
    let ip = match record.balance {
        Balance::Failover => c[0].0,
        Balance::Weighted => c[smooth_weighted(&choice.swrr, &c)].0,
        Balance::Locality => {
            let local: Vec<IpAddr> = c
                .iter()
                .filter(|(_, b)| site.is_some() && b.site.as_deref() == site)
                .map(|(ip, _)| *ip)
                .collect();
            if local.is_empty() {
                c[next(c.len())].0
            } else {
                local[next(local.len())]
            }
        }
        // No affinity key on this path: sticky behaves as round-robin.
        Balance::RoundRobin | Balance::Sticky | Balance::Unknown => c[next(c.len())].0,
    };
    Selection::Chosen(ip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use HealthState::*;

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
                })
                .collect(),
        };
        r.refresh_all_down();
        r
    }

    fn ips(list: &[&str]) -> Vec<IpAddr> {
        list.iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn candidate_sets_example() {
        use OnAllDown::*;
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
        use OnAllDown::*;
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
        use OnAllDown::*;
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
}
