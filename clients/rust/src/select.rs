//! Choosing a backend from a service record: staleness, candidates and the
//! balance methods. Pure functions, so every rule is unit-tested.

use emfor_format::{BackendRecord, Balance, OnAllDown, ServiceRecord, rules};
use md5::{Digest, Md5};

use crate::{Error, ErrorCode};

/// ASCII-lowercase and drop one trailing dot, as the daemon does with
/// configured service names.
pub fn normalize(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    match lower.strip_suffix('.') {
        Some(s) => s.to_string(),
        None => lower,
    }
}

/// The store is stale when the heartbeat is 0 or older than `max_ms`. A
/// heartbeat in the future is fresh.
pub fn is_stale(heartbeat: u64, now_ms: u64, max_ms: u64) -> bool {
    heartbeat == 0 || now_ms.saturating_sub(heartbeat) > max_ms
}

/// Backends to pick from, in config order.
pub fn candidates(rec: &ServiceRecord, stale: bool) -> Result<Vec<&BackendRecord>, Error> {
    let all: Vec<&BackendRecord> = rec.backends.iter().collect();
    if let Some(c) = rules::healthy_or_all(rec.on_all_down, &all, !stale, |b| b) {
        return Ok(c);
    }
    let why = if stale {
        "the store is stale (emfor daemon not updating it)"
    } else {
        "no backend is healthy"
    };
    Err(Error::new(
        ErrorCode::AllDown,
        match rec.on_all_down {
            OnAllDown::Closed => {
                format!("service {}: {why}, and on_all_down is closed", rec.name)
            }
            OnAllDown::Open => format!("service {} has no backends", rec.name),
        },
    ))
}

/// `fastest`: the candidates listed in the daemon's `pool`, when the store
/// is fresh and the pool names at least one of them; otherwise all
/// candidates (the service then behaves as `round-robin`). The client never
/// judges backends itself.
pub fn pool_candidates<'a>(
    rec: &ServiceRecord,
    candidates: Vec<&'a BackendRecord>,
    stale: bool,
) -> Vec<&'a BackendRecord> {
    if rec.balance != Balance::Fastest || stale {
        return candidates;
    }
    let pool = rec.pool.as_deref();
    rules::subset_or_all(&candidates, |b| {
        rules::preferred(Balance::Fastest, b, None, pool)
    })
}

/// Rendezvous hash version 1: the first 4 bytes of MD5(key ++ id) as a
/// big-endian u32.
pub fn rendezvous_hash(key: &str, id: &str) -> u32 {
    let mut h = Md5::new();
    h.update(key.as_bytes());
    h.update(id.as_bytes());
    let d = h.finalize();
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}

/// Index of the id with the highest hash; ties go to the earlier one.
pub fn rendezvous_index<'a>(key: &str, ids: impl Iterator<Item = &'a str>) -> usize {
    let mut best: Option<(u32, usize)> = None;
    for (i, id) in ids.enumerate() {
        let h = rendezvous_hash(key, id);
        if best.is_none_or(|(b, _)| h > b) {
            best = Some((h, i));
        }
    }
    best.map_or(0, |(_, i)| i)
}

/// Pick one candidate by `balance`. `site` is the host's site (`meta/site`);
/// `next` gives this service's next round-robin counter value.
pub fn pick(
    balance: Balance,
    candidates: &[&BackendRecord],
    affinity_key: Option<&str>,
    site: Option<&str>,
    next: impl FnOnce() -> u64,
) -> String {
    let key = affinity_key.filter(|k| !k.is_empty());
    let i = match (balance, key) {
        (Balance::Failover, _) => 0,
        (Balance::Sticky, Some(k)) => rendezvous_index(k, candidates.iter().map(|b| b.id.as_str())),
        (Balance::Weighted, _) => weighted_index(candidates, next()),
        (Balance::Locality, _) => {
            let all: Vec<usize> = (0..candidates.len()).collect();
            let local = rules::subset_or_all(&all, |i| {
                rules::preferred(Balance::Locality, candidates[i], site, None)
            });
            local[(next() % local.len() as u64) as usize]
        }
        // round-robin, sticky without a key, and a balance this client does
        // not know (written by a newer daemon).
        _ => (next() % candidates.len() as u64) as usize,
    };
    candidates[i].id.clone()
}

/// `weighted`: the first candidate, in config order, whose cumulative weight
/// exceeds `counter % W`, W being the sum of the weights.
pub fn weighted_index(candidates: &[&BackendRecord], counter: u64) -> usize {
    let total: u64 = candidates.iter().map(|b| u64::from(b.weight)).sum();
    if total == 0 {
        return (counter % candidates.len() as u64) as usize;
    }
    let r = counter % total;
    let mut cumulative = 0;
    for (i, b) in candidates.iter().enumerate() {
        cumulative += u64::from(b.weight);
        if cumulative > r {
            return i;
        }
    }
    candidates.len() - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use emfor_format::HealthState;

    fn be(id: &str, state: HealthState) -> BackendRecord {
        let (ip, port) = id.rsplit_once(':').unwrap();
        BackendRecord {
            id: id.into(),
            ip: ip.trim_matches(['[', ']']).into(),
            port: port.parse().unwrap(),
            weight: 1,
            site: None,
            state,
            checked_at: Some(1),
            reason: None,
            latency_us: None,
        }
    }

    fn rec(
        balance: Balance,
        on_all_down: OnAllDown,
        backends: Vec<BackendRecord>,
    ) -> ServiceRecord {
        ServiceRecord {
            name: "api.svc.emfor".into(),
            on_all_down,
            balance,
            all_down: false,
            pool: None,
            backends,
        }
    }

    const API: [&str; 3] = ["10.0.0.11:8080", "10.0.0.12:8080", "10.0.0.13:8080"];
    use HealthState::{Healthy, Unhealthy, Unknown};

    #[test]
    fn heartbeat_boundaries() {
        let now = 1_700_000_010_000;
        assert!(!is_stale(1_700_000_007_000, now, 3000), "3000 ms old");
        assert!(is_stale(1_700_000_006_999, now, 3000), "3001 ms old");
        assert!(!is_stale(1_700_000_015_000, now, 3000), "future");
        assert!(is_stale(0, now, 3000), "zero");
    }

    #[test]
    fn name_normalization() {
        assert_eq!(normalize("api.svc.emfor"), "api.svc.emfor");
        assert_eq!(normalize("API.Svc.Emfor"), "api.svc.emfor");
        assert_eq!(normalize("api.svc.emfor."), "api.svc.emfor");
        assert_eq!(normalize("api.svc.emfor.."), "api.svc.emfor.");
    }

    #[test]
    fn candidates_follow_health_and_on_all_down() {
        let r = rec(
            Balance::Failover,
            OnAllDown::Open,
            vec![be("10.0.1.5:5432", Unhealthy), be("10.0.1.6:5432", Healthy)],
        );
        let c = candidates(&r, false).unwrap();
        assert_eq!(
            c.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(),
            ["10.0.1.6:5432"]
        );
        // Stale: every backend unknown; open uses all, in config order.
        let c = candidates(&r, true).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].id, "10.0.1.5:5432");
        let r = rec(
            Balance::Failover,
            OnAllDown::Open,
            vec![be("10.0.1.5:5432", Unhealthy), be("10.0.1.6:5432", Unknown)],
        );
        assert_eq!(
            pick(r.balance, &candidates(&r, false).unwrap(), None, None, || 0),
            "10.0.1.5:5432"
        );
        let r = rec(
            Balance::Failover,
            OnAllDown::Closed,
            vec![be("10.0.1.5:5432", Healthy)],
        );
        assert_eq!(candidates(&r, true).unwrap_err().code, ErrorCode::AllDown);
        let r = rec(
            Balance::Failover,
            OnAllDown::Closed,
            vec![be("10.0.1.5:5432", Unhealthy)],
        );
        assert_eq!(candidates(&r, false).unwrap_err().code, ErrorCode::AllDown);
        let r = rec(Balance::Failover, OnAllDown::Open, vec![]);
        assert_eq!(candidates(&r, false).unwrap_err().code, ErrorCode::AllDown);
    }

    /// A healthy backend whose last check failed (`reason` set) is used
    /// only when no healthy backend passes; a stale store ignores `reason`.
    #[test]
    fn candidates_prefer_passing_backends() {
        let failing = |id: &str| {
            let mut b = be(id, Healthy);
            b.reason = Some("timeout".into());
            b
        };
        let ids = |c: Vec<&BackendRecord>| c.iter().map(|b| b.id.clone()).collect::<Vec<_>>();
        let r = rec(
            Balance::Failover,
            OnAllDown::Closed,
            vec![failing("10.0.1.5:5432"), be("10.0.1.6:5432", Healthy)],
        );
        assert_eq!(ids(candidates(&r, false).unwrap()), ["10.0.1.6:5432"]);
        let r = rec(
            Balance::RoundRobin,
            OnAllDown::Closed,
            vec![
                failing("10.0.1.5:5432"),
                failing("10.0.1.6:5432"),
                be("10.0.1.7:5432", Unhealthy),
            ],
        );
        assert_eq!(
            ids(candidates(&r, false).unwrap()),
            ["10.0.1.5:5432", "10.0.1.6:5432"]
        );
        let r = rec(
            Balance::Failover,
            OnAllDown::Open,
            vec![failing("10.0.1.5:5432"), be("10.0.1.6:5432", Healthy)],
        );
        assert_eq!(
            ids(candidates(&r, true).unwrap()),
            ["10.0.1.5:5432", "10.0.1.6:5432"]
        );
    }

    #[test]
    fn rendezvous_hash_vectors() {
        for (key, id, want) in [
            ("user-42", API[0], 1022706018),
            ("user-42", API[1], 3018330109),
            ("user-42", API[2], 336531056),
            ("user-7", API[0], 4189533513),
            ("user-7", API[1], 1529156490),
            ("user-7", API[2], 3484999621),
        ] {
            assert_eq!(rendezvous_hash(key, id), want, "{key} {id}");
        }
    }

    #[test]
    fn rendezvous_picks() {
        let all: Vec<BackendRecord> = API.iter().map(|a| be(a, Healthy)).collect();
        let refs: Vec<&BackendRecord> = all.iter().collect();
        let without = |skip: usize| -> Vec<&BackendRecord> {
            refs.iter()
                .enumerate()
                .filter(|(i, _)| *i != skip)
                .map(|(_, b)| *b)
                .collect()
        };
        let never = || panic!("sticky with a key must not use the counter");
        assert_eq!(
            pick(Balance::Sticky, &refs, Some("user-42"), None, never),
            API[1]
        );
        assert_eq!(
            pick(Balance::Sticky, &without(1), Some("user-42"), None, never),
            API[0]
        );
        assert_eq!(
            pick(Balance::Sticky, &refs, Some("user-7"), None, never),
            API[0]
        );
        assert_eq!(
            pick(Balance::Sticky, &without(0), Some("user-7"), None, never),
            API[2]
        );
    }

    #[test]
    fn rendezvous_tie_goes_to_earlier() {
        assert_eq!(rendezvous_index("k", ["same", "same"].into_iter()), 0);
    }

    #[test]
    fn round_robin_and_failover() {
        let all: Vec<BackendRecord> = API.iter().map(|a| be(a, Healthy)).collect();
        let refs: Vec<&BackendRecord> = all.iter().collect();
        let got: std::collections::BTreeSet<String> = (5..8)
            .map(|n| pick(Balance::RoundRobin, &refs, Some("user-42"), None, || n))
            .collect();
        assert_eq!(got.len(), 3, "3 consecutive counter values cover all 3");
        // Sticky without a key, or with an empty key, is round-robin.
        assert_eq!(pick(Balance::Sticky, &refs, Some(""), None, || 4), API[1]);
        assert_eq!(pick(Balance::Sticky, &refs, None, None, || 5), API[2]);
        assert_eq!(
            pick(Balance::Failover, &refs, Some("user-42"), None, || panic!(
                "no counter"
            )),
            API[0]
        );
    }

    #[test]
    fn weighted_counter_to_backend_example() {
        let mut heavy = be("10.0.0.11:8080", Healthy);
        heavy.weight = 3;
        let light = be("10.0.0.12:8080", Healthy);
        let refs = [&heavy, &light];
        let got: Vec<String> = (0..8)
            .map(|n| pick(Balance::Weighted, &refs, Some("user-42"), None, || n))
            .collect();
        let (h, l) = ("10.0.0.11:8080", "10.0.0.12:8080");
        assert_eq!(got, [h, h, h, l, h, h, h, l]);
        // A weight of 0 (never written by the daemon) does not panic.
        let mut zero = light.clone();
        zero.weight = 0;
        assert_eq!(weighted_index(&[&zero], 5), 0);
    }

    #[test]
    fn weighted_skips_unhealthy_and_reads_old_records() {
        let mut heavy = be("10.0.0.11:8080", Unhealthy);
        heavy.weight = 3;
        let r = rec(
            Balance::Weighted,
            OnAllDown::Open,
            vec![heavy, be("10.0.0.12:8080", Healthy)],
        );
        let c = candidates(&r, false).unwrap();
        for n in 0..3 {
            assert_eq!(pick(r.balance, &c, None, None, || n), "10.0.0.12:8080");
        }
        // Records from an older daemon have no weight: each counts as 1.
        let json = r#"{"name":"a","on_all_down":"open","balance":"weighted","all_down":false,"backends":[
            {"id":"10.0.0.11:8080","ip":"10.0.0.11","port":8080,"state":"healthy","checked_at":1,"reason":null},
            {"id":"10.0.0.12:8080","ip":"10.0.0.12","port":8080,"state":"healthy","checked_at":1,"reason":null}]}"#;
        let r: ServiceRecord = serde_json::from_str(json).unwrap();
        let c = candidates(&r, false).unwrap();
        let got: std::collections::BTreeSet<String> = (0..2)
            .map(|n| pick(r.balance, &c, None, None, || n))
            .collect();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn unknown_balance_is_round_robin_and_ignores_the_key() {
        let all: Vec<BackendRecord> = API[..2].iter().map(|a| be(a, Healthy)).collect();
        let refs: Vec<&BackendRecord> = all.iter().collect();
        let b: Balance = serde_json::from_str("\"some-future-method\"").unwrap();
        let got: std::collections::BTreeSet<String> = (0..2)
            .map(|n| pick(b, &refs, Some("user-42"), None, || n))
            .collect();
        assert_eq!(got.len(), 2);
    }

    /// Four lookups with counter values 0..4 through the client's path.
    fn fastest_picks(r: &ServiceRecord, stale: bool, n: u64) -> Vec<String> {
        (0..n)
            .map(|i| {
                let c = pool_candidates(r, candidates(r, stale).unwrap(), stale);
                pick(r.balance, &c, Some("user-42"), None, || i)
            })
            .collect()
    }

    fn fastest_rec(pool: Option<&[&str]>) -> ServiceRecord {
        let mut r = rec(
            Balance::Fastest,
            OnAllDown::Open,
            API.iter().map(|id| be(id, Healthy)).collect(),
        );
        r.pool = pool.map(|p| p.iter().map(|s| s.to_string()).collect());
        r
    }

    #[test]
    fn fastest_lookups_stay_in_the_pool() {
        let r = fastest_rec(Some(&API[..2]));
        assert_eq!(
            fastest_picks(&r, false, 4),
            [API[0], API[1], API[0], API[1]]
        );
    }

    #[test]
    fn fastest_stale_heartbeat_ignores_the_pool() {
        let r = fastest_rec(Some(&API[..2]));
        assert_eq!(fastest_picks(&r, true, 3), API);
    }

    #[test]
    fn fastest_missing_empty_or_unknown_pool_is_round_robin() {
        for pool in [None, Some(&[][..]), Some(&["10.9.9.9:80"][..])] {
            let r = fastest_rec(pool);
            assert_eq!(fastest_picks(&r, false, 2), [API[0], API[1]], "{pool:?}");
        }
    }
}
