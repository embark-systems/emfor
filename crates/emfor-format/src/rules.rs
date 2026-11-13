//! Selection rules shared by the daemon's DNS server and the client
//! library: which backends of a record may be chosen. How one of them is
//! then picked (turns, weights, affinity) differs between the two and stays
//! with each.

use crate::{BackendRecord, Balance, HealthState, OnAllDown};

/// The healthy entries of `list` when there are any. Otherwise every entry
/// when `on_all_down` is `open`, or `None` (all down) when it is `closed`
/// or `list` is empty. `backend` gives each entry's record; `trust_health`
/// false (a stale store) counts no backend as healthy.
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
    if !healthy.is_empty() {
        return Some(healthy);
    }
    match on_all_down {
        OnAllDown::Open if !list.is_empty() => Some(list.to_vec()),
        _ => None,
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
        ];
        for (list, mode, trust, want) in cases {
            let got = ids(healthy_or_all(*mode, list, *trust, |r| r));
            let want = want
                .as_ref()
                .map(|w| w.iter().map(|s| s.to_string()).collect());
            assert_eq!(got, want, "{mode:?} trust={trust}");
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

    #[test]
    fn subset_falls_back_to_all() {
        assert_eq!(subset_or_all(&[1, 2, 3], |x| x > 1), [2, 3]);
        assert_eq!(subset_or_all(&[1, 2, 3], |x| x > 5), [1, 2, 3]);
    }
}
