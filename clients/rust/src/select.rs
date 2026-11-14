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
            // A value this client does not know counts as open.
            OnAllDown::Open | OnAllDown::Unknown => {
                format!("service {} has no backends", rec.name)
            }
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
/// big-endian u32. Lookups use [`rendezvous_score`], whose weight-1 value
/// it is; this is kept for the tests and `testing::rendezvous_hash`.
#[cfg(any(test, feature = "testing"))]
pub fn rendezvous_hash(key: &str, id: &str) -> u32 {
    let mut h = Md5::new();
    h.update(key.as_bytes());
    h.update(id.as_bytes());
    let d = h.finalize();
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}

/// The largest weight the daemon accepts, and the most virtual nodes a
/// candidate gets in [`rendezvous_score`].
pub(crate) const MAX_STICKY_WEIGHT: u32 = 100;

/// Weighted rendezvous score of backend `id` with `weight` for `key`: the
/// largest of `h_0 .. h_(weight-1)`, where `h_0` is the first 4 bytes of
/// MD5(key ++ id) as a big-endian u32 and `h_i` hashes `key ++ id ++ "#" ++ i` (decimal) the same way. Weights the
/// daemon never writes are bounded: above [`MAX_STICKY_WEIGHT`] counts as
/// that, so a damaged store cannot make a lookup hash billions of times,
/// and 0 counts as 1.
pub fn rendezvous_score(key: &str, id: &str, weight: u32) -> u32 {
    let weight = weight.min(MAX_STICKY_WEIGHT);
    let mut base = Md5::new();
    base.update(key.as_bytes());
    base.update(id.as_bytes());
    let first = |d: &[u8]| u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
    let mut best = first(&base.clone().finalize());
    let mut digits = [0u8; 10];
    for i in 1..weight {
        let mut h = base.clone();
        h.update(b"#");
        h.update(decimal(i, &mut digits));
        best = best.max(first(&h.finalize()));
    }
    best
}

/// The decimal digits of `n`, written at the end of `buf`.
fn decimal(mut n: u32, buf: &mut [u8; 10]) -> &[u8] {
    let mut start = buf.len();
    loop {
        start -= 1;
        // n % 10 is below 10, so the cast keeps every bit.
        buf[start] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            return &buf[start..];
        }
    }
}

/// Index of the `(id, weight)` with the highest [`rendezvous_score`]; ties
/// go to the earlier one.
pub fn rendezvous_index<'a>(key: &str, candidates: impl Iterator<Item = (&'a str, u32)>) -> usize {
    let mut best: Option<(u32, usize)> = None;
    for (i, (id, weight)) in candidates.enumerate() {
        let h = rendezvous_score(key, id, weight);
        if best.is_none_or(|(b, _)| h > b) {
            best = Some((h, i));
        }
    }
    best.map_or(0, |(_, i)| i)
}

/// Pick one candidate by `balance`, with slow start off. `site` is the
/// host's site (`meta/site`); `next` gives this service's next round-robin
/// counter value.
#[cfg(test)]
pub fn pick(
    balance: Balance,
    candidates: &[&BackendRecord],
    affinity_key: Option<&str>,
    site: Option<&str>,
    next: impl FnOnce() -> u64,
) -> String {
    pick_at(balance, candidates, affinity_key, site, 0, 0, next)
}

/// [`pick`] at `now_ms` for a service whose `slow_start_ms` is given (0
/// when the store is stale). While a backend the method would choose from
/// is in slow start, the choice goes by effective weights with the counter;
/// `failover` and `sticky` with a key are not affected.
pub fn pick_at(
    balance: Balance,
    candidates: &[&BackendRecord],
    affinity_key: Option<&str>,
    site: Option<&str>,
    slow_start_ms: u64,
    now_ms: u64,
    next: impl FnOnce() -> u64,
) -> String {
    let key = affinity_key.filter(|k| !k.is_empty());
    match (balance, key) {
        (Balance::Failover, _) => return candidates[0].id.clone(),
        (Balance::Sticky, Some(k)) => {
            // Configured weights: slow start does not change sticky with a key.
            let i = rendezvous_index(k, candidates.iter().map(|b| (b.id.as_str(), b.weight)));
            return candidates[i].id.clone();
        }
        _ => {}
    }
    // `locality` chooses among the backends at the host's site, if any.
    let pool: Vec<&BackendRecord> = if balance == Balance::Locality {
        rules::subset_or_all(candidates, |b| {
            rules::preferred(Balance::Locality, b, site, None)
        })
    } else {
        candidates.to_vec()
    };
    let weights: Vec<u64> = if rules::any_ramping(&pool, slow_start_ms, now_ms, |b| b) {
        pool.iter()
            .map(|b| {
                let base = rules::base_weight(balance, b);
                u64::from(rules::effective_weight(b, base, slow_start_ms, now_ms))
            })
            .collect()
    } else if balance == Balance::Weighted {
        pool.iter().map(|b| u64::from(b.weight)).collect()
    } else {
        // round-robin, locality, sticky without a key, and a balance this
        // client does not know (written by a newer daemon).
        vec![1; pool.len()]
    };
    pool[cumulative_index(&weights, next())].id.clone()
}

/// The first index, in config order, whose cumulative weight exceeds
/// `counter % W`, W being the sum of `weights`; with W = 0, `counter % len`.
fn cumulative_index(weights: &[u64], counter: u64) -> usize {
    let total: u64 = weights.iter().sum();
    if total == 0 {
        return (counter % weights.len() as u64) as usize;
    }
    let r = counter % total;
    let mut cumulative = 0;
    for (i, w) in weights.iter().enumerate() {
        cumulative += w;
        if cumulative > r {
            return i;
        }
    }
    weights.len() - 1
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
            backup: false,
            recovered_at: None,
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
            slow_start_ms: 0,
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
        // Open with nothing healthy prefers the backend not known to be down.
        assert_eq!(
            pick(r.balance, &candidates(&r, false).unwrap(), None, None, || 0),
            "10.0.1.6:5432"
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

    /// Values a newer daemon may write: a backend `state` the client does
    /// not know counts as `unhealthy`, an `on_all_down` it does not know as
    /// `open`.
    #[test]
    fn unknown_state_and_on_all_down_values() {
        let parse = |on_all_down: &str, second: &str| -> ServiceRecord {
            let json = format!(
                r#"{{"name":"api.svc.emfor","on_all_down":"{on_all_down}","all_down":true,"backends":[{{"id":"10.0.0.11:8080","ip":"10.0.0.11","port":8080,"state":"some-future-state","checked_at":1,"reason":null}},{{"id":"10.0.0.12:8080","ip":"10.0.0.12","port":8080,"state":"{second}","checked_at":1,"reason":"refused"}}]}}"#
            );
            serde_json::from_str(&json).expect("a record with unknown values parses")
        };
        let ids = |c: Vec<&BackendRecord>| c.iter().map(|b| b.id.clone()).collect::<Vec<_>>();
        // Not healthy: an `unknown` backend is preferred to it.
        let r = parse("open", "unknown");
        assert_eq!(ids(candidates(&r, false).unwrap()), ["10.0.0.12:8080"]);
        // Like `unhealthy`: open with nothing healthy or unknown uses all.
        let r = parse("some-future-mode", "unhealthy");
        assert_eq!(
            ids(candidates(&r, false).unwrap()),
            ["10.0.0.11:8080", "10.0.0.12:8080"]
        );
        // Never healthy: closed fails.
        let r = parse("closed", "unhealthy");
        assert_eq!(candidates(&r, false).unwrap_err().code, ErrorCode::AllDown);
        // A healthy backend is still chosen over it.
        let r = parse("some-future-mode", "healthy");
        assert_eq!(ids(candidates(&r, false).unwrap()), ["10.0.0.12:8080"]);
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

    /// The key is hashed as its UTF-8 bytes: "café" picks API[1] of the
    /// first two, and the double-encoded "cafÃ©" API[0]. clients/perl/t/emfor.t
    /// expects these picks.
    #[test]
    fn non_ascii_key_pick() {
        let two = [(API[0], 1), (API[1], 1)];
        assert_eq!(rendezvous_hash("café", API[0]), 3148372924);
        assert_eq!(rendezvous_hash("café", API[1]), 4131976985);
        assert_eq!(rendezvous_index("café", two.iter().copied()), 1);
        assert_eq!(rendezvous_index("cafÃ©", two.iter().copied()), 0);
    }

    /// Spec client-lookup Example "weighted hash vectors".
    #[test]
    fn weighted_hash_vectors() {
        let virtual_node = |key: &str, id: &str, i: u32| {
            let d = Md5::digest(format!("{key}{id}#{i}").as_bytes());
            u32::from_be_bytes([d[0], d[1], d[2], d[3]])
        };
        for (key, id, i, want) in [
            ("user-42", API[0], 1, 101315482),
            ("user-42", API[0], 2, 3080192789),
            ("user-42", API[2], 1, 491054110),
            ("user-7", API[2], 1, 2987525892),
        ] {
            assert_eq!(virtual_node(key, id, i), want, "{key} {id}#{i}");
        }
        // The score is the largest of h_0 .. h_(w-1).
        assert_eq!(rendezvous_score("user-42", API[0], 1), 1022706018);
        assert_eq!(rendezvous_score("user-42", API[0], 2), 1022706018);
        assert_eq!(rendezvous_score("user-42", API[0], 3), 3080192789);
        assert_eq!(rendezvous_score("user-7", API[2], 2), 3484999621);
        assert_eq!(rendezvous_score("user-42", API[0], 0), 1022706018);
        // Spec Example "weights above 100": h_100 of key-795 at .12 is
        // 4277241900, so only the cap keeps .11 (4257715895) ahead.
        assert_eq!(rendezvous_score("key-795", API[0], 100), 4257715895);
        assert_eq!(rendezvous_score("key-795", API[1], 100), 4228454921);
        assert_eq!(rendezvous_score("key-795", API[1], 101), 4228454921);
        assert_eq!(rendezvous_score("key-795", API[1], u32::MAX), 4228454921);
        assert_eq!(decimal(0, &mut [0; 10]), b"0");
        assert_eq!(decimal(u32::MAX, &mut [0; 10]), b"4294967295");
    }

    /// Spec client-lookup Example "weighted picks".
    #[test]
    fn weighted_picks() {
        let never = || panic!("sticky with a key must not use the counter");
        for (key, weights, want) in [
            ("user-42", [1, 1, 1], API[1]),
            ("user-42", [3, 1, 1], API[0]),
            ("user-42", [1, 1, 100], API[2]),
            ("user-7", [1, 1, 1], API[0]),
            ("user-7", [2, 2, 2], API[0]),
            ("user-7", [1, 1, 100], API[2]),
        ] {
            let all: Vec<BackendRecord> = API
                .iter()
                .zip(weights)
                .map(|(a, w)| BackendRecord {
                    weight: w,
                    ..be(a, Healthy)
                })
                .collect();
            let refs: Vec<&BackendRecord> = all.iter().collect();
            assert_eq!(
                pick(Balance::Sticky, &refs, Some(key), None, never),
                want,
                "{key} {weights:?}"
            );
        }
    }

    /// Spec client-lookup Scenario "Raising a weight only moves keys to
    /// that backend", and weight 1 picks as rendezvous hashing version 1.
    #[test]
    fn raising_a_weight_only_moves_keys_to_it() {
        let version_1 = |key: &str| {
            let hashes: Vec<u32> = API.iter().map(|id| rendezvous_hash(key, id)).collect();
            let mut best = 0;
            for (i, h) in hashes.iter().enumerate() {
                if *h > hashes[best] {
                    best = i;
                }
            }
            best
        };
        let pick_with =
            |key: &str, weights: [u32; 3]| rendezvous_index(key, API.iter().copied().zip(weights));
        let (mut to_heavy, mut moved) = (0, 0);
        for k in 0..1000 {
            let key = format!("key-{k}");
            let before = pick_with(&key, [1, 1, 1]);
            assert_eq!(before, version_1(&key), "{key}");
            let after = pick_with(&key, [1, 1, 2]);
            if after == 2 {
                to_heavy += 1;
            }
            if after != before {
                moved += 1;
                assert_eq!(after, 2, "{key} moved to another backend");
            }
        }
        assert_eq!((to_heavy, moved), (499, 151));
    }

    /// Slow start does not change the weights sticky with a key uses.
    #[test]
    fn weighted_sticky_ignores_slow_start() {
        let all: Vec<BackendRecord> = API
            .iter()
            .zip([3, 1, 1])
            .map(|(a, w)| BackendRecord {
                weight: w,
                recovered_at: Some(1_000),
                ..be(a, Healthy)
            })
            .collect();
        let refs: Vec<&BackendRecord> = all.iter().collect();
        let never = || panic!("sticky with a key must not use the counter");
        assert_eq!(
            pick_at(
                Balance::Sticky,
                &refs,
                Some("user-42"),
                None,
                60_000,
                2_000,
                never
            ),
            API[0]
        );
    }

    #[test]
    fn rendezvous_tie_goes_to_earlier() {
        assert_eq!(
            rendezvous_index("k", [("same", 1), ("same", 1)].into_iter()),
            0
        );
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
        assert_eq!(cumulative_index(&[u64::from(zero.weight)], 5), 0);
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

    /// Slow start of 100 s; `ramping` recovered 25 s ago (step 3 of 10).
    const SLOW: u64 = 100_000;
    const NOW: u64 = 1_700_000_100_000;

    fn ramping(id: &str) -> BackendRecord {
        let mut b = be(id, Healthy);
        b.recovered_at = Some(NOW - 25_000);
        b
    }

    /// How often each id is picked for counter values 0..n.
    fn slow_counts(
        balance: Balance,
        refs: &[&BackendRecord],
        key: Option<&str>,
        site: Option<&str>,
        slow: u64,
        n: u64,
    ) -> std::collections::BTreeMap<String, u64> {
        let mut m = std::collections::BTreeMap::new();
        for i in 0..n {
            *m.entry(pick_at(balance, refs, key, site, slow, NOW, || i))
                .or_insert(0) += 1;
        }
        m
    }

    fn map(pairs: &[(&str, u64)]) -> std::collections::BTreeMap<String, u64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn slow_start_round_robin_share() {
        let (a, b) = (be(API[0], Healthy), ramping(API[1]));
        let refs = [&a, &b];
        for balance in [Balance::RoundRobin, Balance::Fastest] {
            assert_eq!(
                slow_counts(balance, &refs, None, None, SLOW, 13),
                map(&[(API[0], 10), (API[1], 3)]),
                "{balance:?}"
            );
        }
        // Sticky without a key ramps too.
        assert_eq!(
            slow_counts(Balance::Sticky, &refs, Some(""), None, SLOW, 13),
            map(&[(API[0], 10), (API[1], 3)])
        );
        // Slow start off (or a stale store): plain round-robin.
        assert_eq!(
            slow_counts(Balance::RoundRobin, &refs, None, None, 0, 4),
            map(&[(API[0], 2), (API[1], 2)])
        );
    }

    #[test]
    fn slow_start_weighted_share() {
        let mut a = ramping(API[0]);
        a.weight = 3;
        let b = be(API[1], Healthy);
        assert_eq!(
            slow_counts(Balance::Weighted, &[&a, &b], None, None, SLOW, 19),
            map(&[(API[0], 9), (API[1], 10)])
        );
    }

    #[test]
    fn slow_start_over_is_full_share() {
        let mut a = ramping(API[0]);
        a.recovered_at = Some(NOW - 200_000);
        let b = be(API[1], Healthy);
        assert_eq!(
            slow_counts(Balance::RoundRobin, &[&a, &b], None, None, SLOW, 4),
            map(&[(API[0], 2), (API[1], 2)])
        );
    }

    #[test]
    fn slow_start_leaves_failover_and_sticky_key() {
        let (a, b) = (ramping(API[0]), be(API[1], Healthy));
        let refs = [&a, &b];
        assert_eq!(
            slow_counts(Balance::Failover, &refs, None, None, SLOW, 5),
            map(&[(API[0], 5)])
        );
        let all: Vec<BackendRecord> = API.iter().map(|id| ramping(id)).collect();
        let refs: Vec<&BackendRecord> = all.iter().collect();
        let never = || panic!("sticky with a key must not use the counter");
        assert_eq!(
            pick_at(
                Balance::Sticky,
                &refs,
                Some("user-42"),
                None,
                SLOW,
                NOW,
                never
            ),
            API[1]
        );
    }

    #[test]
    fn slow_start_locality_within_the_site() {
        let mut a = ramping(API[0]);
        a.site = Some("tpe".into());
        let mut b = be(API[1], Healthy);
        b.site = Some("tpe".into());
        let c = be(API[2], Healthy);
        assert_eq!(
            slow_counts(
                Balance::Locality,
                &[&a, &b, &c],
                None,
                Some("tpe"),
                SLOW,
                13
            ),
            map(&[(API[0], 3), (API[1], 10)])
        );
    }

    /// Time of one weighted sticky choice among 10 candidates of weight 1,
    /// 10 and 100 (design "先量測，再決定"). Run with
    /// `cargo test --release -p emfor-client --lib measure_weighted_sticky -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, run by hand in a release build"]
    fn measure_weighted_sticky() {
        let ids: Vec<String> = (1..=10).map(|i| format!("10.0.0.{i}:8080")).collect();
        for weight in [1u32, 10, 100] {
            let mut runs = Vec::new();
            for _ in 0..3 {
                let n = 100_000u32;
                let start = std::time::Instant::now();
                let mut sum = 0usize;
                for k in 0..n {
                    let key = format!("key-{k}");
                    sum += rendezvous_index(&key, ids.iter().map(|id| (id.as_str(), weight)));
                }
                std::hint::black_box(sum);
                runs.push(start.elapsed().as_secs_f64() * 1e6 / f64::from(n));
            }
            runs.sort_by(f64::total_cmp);
            println!(
                "10 x weight {weight}: {:.2} us per choice (runs {runs:.2?})",
                runs[1]
            );
        }
    }
}
