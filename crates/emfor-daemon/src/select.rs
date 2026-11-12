//! Choosing one backend of a service without an affinity key.
//!
//! This is the reference for the selection rules that every language client
//! must follow: candidates per address family, per-family all-down handling,
//! then `balance`. No I/O.

use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};

use emfor_format::{Balance, HealthState, OnAllDown, ServiceRecord};

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
pub fn candidates(record: &ServiceRecord, family: Family) -> Result<Vec<IpAddr>, Selection> {
    let of_family: Vec<(IpAddr, HealthState)> = record
        .backends
        .iter()
        .filter_map(|b| b.ip.parse::<IpAddr>().ok().map(|ip| (ip, b.state)))
        .filter(|(ip, _)| family.matches(ip))
        .collect();
    if of_family.is_empty() {
        return Err(Selection::NoFamily);
    }
    let healthy: Vec<IpAddr> = of_family
        .iter()
        .filter(|(_, s)| *s == HealthState::Healthy)
        .map(|(ip, _)| *ip)
        .collect();
    if !healthy.is_empty() {
        return Ok(healthy);
    }
    match record.on_all_down {
        OnAllDown::Open => Ok(of_family.into_iter().map(|(ip, _)| ip).collect()),
        OnAllDown::Closed => Err(Selection::AllDown),
    }
}

/// Choose one backend. `counter` is the round-robin position for this
/// service and family; it advances only for `round-robin` and `sticky`.
pub fn select(record: &ServiceRecord, family: Family, counter: &AtomicU64) -> Selection {
    let c = match candidates(record, family) {
        Ok(c) => c,
        Err(s) => return s,
    };
    let i = match record.balance {
        Balance::Failover => 0,
        // No affinity key on this path: sticky behaves as round-robin.
        Balance::RoundRobin | Balance::Sticky => {
            (counter.fetch_add(1, Ordering::Relaxed) % c.len() as u64) as usize
        }
    };
    Selection::Chosen(c[i])
}

#[cfg(test)]
mod tests {
    use super::*;
    use HealthState::*;
    use emfor_format::BackendRecord;

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
        let c = AtomicU64::new(0);
        for _ in 0..4 {
            assert_eq!(
                select(&r, Family::V4, &c),
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
        let c = AtomicU64::new(0);
        let picks: Vec<_> = (0..4).map(|_| select(&r, Family::V4, &c)).collect();
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
            let c = AtomicU64::new(0);
            let want = match want {
                Some(ip) => Selection::Chosen(ip.parse().unwrap()),
                None => Selection::AllDown,
            };
            for _ in 0..3 {
                assert_eq!(select(&r, Family::V4, &c), want, "{states:?} {oad:?}");
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
        let c = AtomicU64::new(0);
        assert_ne!(select(&r, Family::V4, &c), select(&r, Family::V4, &c));
    }
}
