//! Choosing a backend from a service record: staleness, candidates and the
//! balance methods. Plain functions over a record, so every rule is
//! unit-tested; the rotation counter of the rotating methods is a parameter
//! (`next`), advanced by the caller.

use emfor_format::rules::Tier;
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

/// The store is stale when the heartbeat is 0, or differs from `now_ms` by
/// more than `max_ms` in either direction. A heartbeat far in the future
/// means the clock was set back, so a daemon that stopped writing would
/// otherwise look alive until the clock caught up.
pub fn is_stale(heartbeat: u64, now_ms: u64, max_ms: u64) -> bool {
    heartbeat == 0 || now_ms.abs_diff(heartbeat) > max_ms
}

/// Backends to pick from, in config order.
pub fn candidates(rec: &ServiceRecord, stale: bool) -> Result<Vec<&BackendRecord>, Error> {
    let all: Vec<&BackendRecord> = rec.backends.iter().collect();
    if let Some(c) = rules::healthy_or_all(rec.on_all_down, &all, !stale, |b| b) {
        return Ok(c);
    }
    Err(all_down_error(rec, stale))
}

/// The `ALL_DOWN` failure of a service with nothing to choose from.
fn all_down_error(rec: &ServiceRecord, stale: bool) -> Error {
    if !rec.backends.is_empty() && rec.backends.iter().all(|b| b.drain) {
        return Error::new(
            ErrorCode::AllDown,
            format!("service {}: every backend is drained", rec.name),
        );
    }
    let why = if stale {
        "the store is stale (emfor daemon not updating it)"
    } else {
        "no backend is healthy"
    };
    Error::new(
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
    )
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

/// How the balance method chooses among the candidates (spec lookup-command,
/// "Explain shows the tier and the reason of every backend").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// The rule in words.
    pub rule: String,
    /// True for `failover` and for `sticky` with a key: the choice does not
    /// depend on a rotation counter.
    pub deterministic: bool,
    /// The backend chosen, when `deterministic` and there is a candidate.
    pub chosen: Option<String>,
}

/// Why a lookup of one service would give the answer it gives: the part of
/// [`crate::Client::explain`] that comes from the service record. The
/// store fields are filled in by the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explanation {
    pub service: String,
    /// The store that was read.
    pub store_path: String,
    /// Milliseconds since the last heartbeat; negative for a heartbeat in the
    /// future (the clock was set back).
    pub heartbeat_age_ms: i64,
    pub stale: bool,
    /// `EMFOR_MAX_STALENESS` in milliseconds.
    pub max_staleness_ms: u64,
    pub balance: Balance,
    pub on_all_down: OnAllDown,
    pub tier: Tier,
    /// Ids of the backends that may be chosen, in config order.
    pub candidates: Vec<String>,
    /// Every other backend with the reason, in config order.
    pub excluded: Vec<(String, String)>,
    /// Candidates in slow start: id, step (1 to 10) and effective weight.
    pub slow_start: Vec<(String, u32, u32)>,
    pub selection: Selection,
    /// The failure a lookup gives when there is no candidate.
    pub error: Option<Error>,
}

/// Record in `excluded` the entries of `before` that `kept` left out.
fn leave_out<'a>(
    before: &[&'a BackendRecord],
    kept: &[&'a BackendRecord],
    why: &'static str,
    excluded: &mut Vec<(&'a BackendRecord, &'static str)>,
) {
    for b in before {
        if !kept.iter().any(|k| std::ptr::eq(*k, *b)) {
            excluded.push((*b, why));
        }
    }
}

/// Explain the lookup of `rec` the way [`candidates`], [`pool_candidates`] and
/// [`pick_at`] decide it, without advancing any counter. `smooth` says that
/// the weighted choices are made by the daemon's smooth weighted round-robin
/// (the DNS server) instead of the client's counter over cumulative weights.
pub fn describe(
    rec: &ServiceRecord,
    stale: bool,
    affinity_key: Option<&str>,
    site: Option<&str>,
    now_ms: u64,
    smooth: bool,
) -> Explanation {
    let all: Vec<&BackendRecord> = rec.backends.iter().collect();
    let found = rules::explain_tiers(rec.on_all_down, &all, !stale, |b| b);
    let tier = found.tier;
    let mut excluded: Vec<(&BackendRecord, &'static str)> = found
        .excluded
        .iter()
        .map(|(b, e)| (*b, e.as_str()))
        .collect();
    let mut cands: Vec<&BackendRecord> = found.candidates;
    if tier != Tier::None && rec.balance == Balance::Fastest && !stale {
        let kept = pool_candidates(rec, cands.clone(), stale);
        leave_out(&cands, &kept, "not in the pool", &mut excluded);
        cands = kept;
    }
    let mut at_site = false;
    if tier != Tier::None && rec.balance == Balance::Locality {
        let kept = rules::subset_or_all(&cands, |b| {
            rules::preferred(Balance::Locality, b, site, None)
        });
        at_site = kept.len() < cands.len()
            || kept
                .iter()
                .any(|b| rules::preferred(Balance::Locality, b, site, None));
        leave_out(&cands, &kept, "not at this site", &mut excluded);
        cands = kept;
    }
    // In config order, as the candidates are.
    let position = |b: &BackendRecord| all.iter().position(|a| std::ptr::eq(*a, b));
    excluded.sort_by_key(|(b, _)| position(b));

    let key = affinity_key.filter(|k| !k.is_empty());
    let fixed = matches!(rec.balance, Balance::Failover)
        || (rec.balance == Balance::Sticky && key.is_some());
    let slow_start_ms = if stale { 0 } else { rec.slow_start_ms };
    let ramping = !fixed && rules::any_ramping(&cands, slow_start_ms, now_ms, |b| b);
    let slow_start: Vec<(String, u32, u32)> = if ramping {
        cands
            .iter()
            .filter_map(|b| {
                let step = rules::slow_start_step(b, slow_start_ms, now_ms);
                let base = rules::base_weight(rec.balance, b);
                (step < rules::SLOW_START_STEPS).then(|| {
                    (
                        b.id.clone(),
                        step,
                        rules::effective_weight(b, base, slow_start_ms, now_ms),
                    )
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let ratio = |weights: &[u64]| {
        weights
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(":")
    };
    let selection = if cands.is_empty() {
        Selection {
            rule: "no backend can be chosen".into(),
            deterministic: false,
            chosen: None,
        }
    } else if rec.balance == Balance::Failover {
        Selection {
            rule: "failover: the first candidate in config order".into(),
            deterministic: true,
            chosen: Some(cands[0].id.clone()),
        }
    } else if let (Balance::Sticky, Some(k)) = (rec.balance, key) {
        let i = rendezvous_index(k, cands.iter().map(|b| (b.id.as_str(), b.weight)));
        Selection {
            rule: "sticky: weighted rendezvous hashing of the key over the candidates".into(),
            deterministic: true,
            chosen: Some(cands[i].id.clone()),
        }
    } else {
        let weights: Vec<u64> = if ramping {
            cands
                .iter()
                .map(|b| {
                    let base = rules::base_weight(rec.balance, b);
                    u64::from(rules::effective_weight(b, base, slow_start_ms, now_ms))
                })
                .collect()
        } else if rec.balance == Balance::Weighted {
            cands.iter().map(|b| u64::from(b.weight)).collect()
        } else {
            Vec::new()
        };
        let rule = match rec.balance {
            Balance::Weighted => {
                let how = if smooth {
                    "smooth weighted round-robin"
                } else {
                    "the candidates in turn"
                };
                format!(
                    "weighted: {how} in proportion to their weights ({})",
                    ratio(&weights)
                )
            }
            Balance::Locality => {
                if at_site {
                    format!(
                        "locality: round-robin over the candidates at this site ({})",
                        site.unwrap_or("-")
                    )
                } else {
                    "locality: round-robin over all candidates (none is at this site)".into()
                }
            }
            Balance::Fastest => {
                let pool_used = !stale
                    && rec
                        .pool
                        .as_ref()
                        .is_some_and(|p| !p.is_empty() && cands.iter().all(|b| p.contains(&b.id)));
                if pool_used {
                    "fastest: round-robin over the daemon's pool".into()
                } else {
                    "fastest: round-robin over the candidates (the daemon's pool is not used)"
                        .into()
                }
            }
            Balance::Sticky => "sticky: no key, so round-robin over the candidates".into(),
            Balance::Unknown => {
                "a balance this client does not know: round-robin over the candidates".into()
            }
            Balance::RoundRobin | Balance::Failover => "round-robin: the candidates in turn".into(),
        };
        let rule = if ramping && rec.balance != Balance::Weighted {
            let how = if smooth {
                "smooth weighted round-robin"
            } else {
                "the candidates in turn"
            };
            format!(
                "{rule}; slow start: {how} in proportion to the weights {}",
                ratio(&weights)
            )
        } else {
            rule
        };
        Selection {
            rule,
            deterministic: false,
            chosen: None,
        }
    };
    let error = (tier == Tier::None).then(|| all_down_error(rec, stale));
    Explanation {
        service: rec.name.clone(),
        store_path: String::new(),
        heartbeat_age_ms: 0,
        stale,
        max_staleness_ms: 0,
        balance: rec.balance,
        on_all_down: rec.on_all_down,
        tier,
        candidates: cands.iter().map(|b| b.id.clone()).collect(),
        excluded: excluded
            .into_iter()
            .map(|(b, why)| (b.id.clone(), why.to_string()))
            .collect(),
        slow_start,
        selection,
        error,
    }
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
            drain: false,
            checked_state: None,
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

    // spectra-scenario: client-lookup / Heartbeat boundaries
    // spectra-scenario: client-lookup / Heartbeat boundaries / heartbeat values
    #[test]
    fn heartbeat_boundaries() {
        let now = 1_700_000_010_000;
        assert!(!is_stale(1_700_000_007_000, now, 3000), "3000 ms old");
        assert!(is_stale(1_700_000_006_999, now, 3000), "3001 ms old");
        assert!(!is_stale(1_700_000_013_000, now, 3000), "3000 ms ahead");
        assert!(is_stale(1_700_000_013_001, now, 3000), "3001 ms ahead");
        assert!(is_stale(1_700_000_015_000, now, 3000), "5000 ms ahead");
        assert!(is_stale(0, now, 3000), "zero");
    }

    // spectra-scenario: client-lookup / Name forms that find the same record
    // spectra-scenario: client-lookup / Name forms that find the same record / name normalization
    #[test]
    fn name_normalization() {
        assert_eq!(normalize("api.svc.emfor"), "api.svc.emfor");
        assert_eq!(normalize("API.Svc.Emfor"), "api.svc.emfor");
        assert_eq!(normalize("api.svc.emfor."), "api.svc.emfor");
        assert_eq!(normalize("api.svc.emfor.."), "api.svc.emfor.");
    }

    // spectra-scenario: client-lookup / Stale store with each on_all_down setting
    // spectra-scenario: client-lookup / Unhealthy backends are skipped
    // spectra-scenario: client-lookup / Open service with every backend down
    // spectra-scenario: backend-selection / Unhealthy backends are skipped
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
    // spectra-scenario: client-lookup / State and on_all_down values from a newer daemon
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
    // spectra-scenario: client-lookup / Failing backend is skipped while another passes
    // spectra-scenario: client-lookup / Every healthy backend failing
    // spectra-scenario: client-lookup / Stale store ignores reason
    // spectra-scenario: backend-selection / Failing backends are skipped while another passes
    // spectra-scenario: backend-selection / Every healthy backend failing
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

    // spectra-scenario: client-lookup / Equal weights keep today's picks
    // spectra-scenario: client-lookup / Equal weights keep today's picks / rendezvous hash vectors
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

    // spectra-scenario: client-lookup / Equal weights keep today's picks
    // spectra-scenario: client-lookup / Equal weights keep today's picks / rendezvous picks
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
    // spectra-scenario: client-lookup / Weights change the share of keys
    // spectra-scenario: client-lookup / Weights change the share of keys / weighted hash vectors
    // spectra-scenario: client-lookup / Weights above 100 count as 100
    // spectra-scenario: client-lookup / Weights above 100 count as 100 / weights above 100
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
    // spectra-scenario: client-lookup / Weights change the share of keys
    // spectra-scenario: client-lookup / Weights change the share of keys / weighted picks
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
    // spectra-scenario: client-lookup / Raising a weight only moves keys to that backend
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
    // spectra-scenario: client-lookup / Failover and sticky with a key ignore slow start
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

    // spectra-scenario: client-lookup / Consecutive lookups cover every candidate
    // spectra-scenario: client-lookup / First healthy backend wins
    // spectra-scenario: client-lookup / Empty key falls back to round-robin
    // spectra-scenario: backend-selection / Sticky without a key
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

    // spectra-scenario: client-lookup / Three to one
    // spectra-scenario: client-lookup / Three to one / counter to backend
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

    // spectra-scenario: client-lookup / Unhealthy heavy backend
    // spectra-scenario: client-lookup / Record from an older daemon
    // spectra-scenario: backend-selection / Unhealthy heavy backend is skipped
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

    // spectra-scenario: client-lookup / Error code by store condition / store conditions
    // spectra-scenario: client-lookup / Future balance value
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

    // spectra-scenario: client-lookup / Lookups stay in the pool
    // spectra-scenario: backend-selection / Fast backends preferred
    #[test]
    fn fastest_lookups_stay_in_the_pool() {
        let r = fastest_rec(Some(&API[..2]));
        assert_eq!(
            fastest_picks(&r, false, 4),
            [API[0], API[1], API[0], API[1]]
        );
    }

    // spectra-scenario: client-lookup / Stale heartbeat ignores the pool
    #[test]
    fn fastest_stale_heartbeat_ignores_the_pool() {
        let r = fastest_rec(Some(&API[..2]));
        assert_eq!(fastest_picks(&r, true, 3), API);
    }

    // spectra-scenario: client-lookup / Missing or empty pool
    // spectra-scenario: backend-selection / Empty pool
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

    // spectra-scenario: client-lookup / Ramping backend gets a smaller share
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

    // spectra-scenario: client-lookup / Weighted ramping backend
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

    // spectra-scenario: client-lookup / Slow start over
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

    // spectra-scenario: client-lookup / Failover and sticky with a key ignore slow start
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

    fn record(balance: &str, extra: serde_json::Value, bes: serde_json::Value) -> ServiceRecord {
        let mut v = serde_json::json!({"name": "api.svc.emfor", "on_all_down": "open",
            "balance": balance, "all_down": false, "backends": bes});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        serde_json::from_value(v).unwrap()
    }

    fn backend_json(id: &str, extra: serde_json::Value) -> serde_json::Value {
        let (ip, port) = id.split_once(':').unwrap();
        let mut v = serde_json::json!({"id": id, "ip": ip, "port": port.parse::<u16>().unwrap(),
            "state": "healthy", "checked_at": 1, "reason": null});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    }

    /// Spec backend-selection "Drained backends are never selected" and
    /// client-lookup "Clients exclude drained backends": whatever the balance
    /// method, a drained backend is not a candidate, a sticky key moves to
    /// the other backend and comes back, and a record without `drain` reads
    /// as before.
    // spectra-scenario: client-lookup / Lookup with a drained backend
    // spectra-scenario: client-lookup / Older store
    // spectra-scenario: backend-selection / Drained backend is skipped
    #[test]
    fn drained_backend_is_never_a_candidate() {
        let drained = serde_json::json!({"state": "unhealthy", "reason": "drained",
            "drain": true, "checked_state": "healthy"});
        let bes = |a: serde_json::Value| {
            serde_json::json!([
                backend_json("10.0.0.11:80", a),
                backend_json("10.0.0.12:80", serde_json::json!({}))
            ])
        };
        for balance in ["round-robin", "sticky", "failover", "weighted", "locality"] {
            let rec = record(balance, serde_json::json!({}), bes(drained.clone()));
            let c = candidates(&rec, false).unwrap();
            assert_eq!(c.len(), 1, "{balance}");
            assert_eq!(c[0].id, "10.0.0.12:80");
            // Stale store, open service: still not the drained one.
            let c = candidates(&rec, true).unwrap();
            assert!(c.iter().all(|b| b.id != "10.0.0.11:80"), "{balance} stale");
        }
        // Without `drain` the same two backends are both candidates.
        let rec = record(
            "round-robin",
            serde_json::json!({}),
            bes(serde_json::json!({})),
        );
        assert_eq!(candidates(&rec, false).unwrap().len(), 2);
    }

    // spectra-scenario: client-lookup / Lookup with a drained backend
    // spectra-scenario: backend-selection / Sticky key moves away
    #[test]
    fn sticky_key_moves_away_from_a_drained_backend_and_comes_back() {
        let drained = serde_json::json!({"state": "unhealthy", "reason": "drained", "drain": true});
        let both = |a: serde_json::Value, b: serde_json::Value| {
            record(
                "sticky",
                serde_json::json!({}),
                serde_json::json!([
                    backend_json("10.0.0.11:80", a),
                    backend_json("10.0.0.12:80", b)
                ]),
            )
        };
        let pick_for = |rec: &ServiceRecord, key: &str| {
            let c = candidates(rec, false).unwrap();
            pick_at(Balance::Sticky, &c, Some(key), None, 0, 0, || 0)
        };
        // A key that maps to the first backend while both are up.
        let keys: Vec<String> = (0..64).map(|i| format!("user-{i}")).collect();
        let healthy = both(serde_json::json!({}), serde_json::json!({}));
        let key = keys
            .iter()
            .find(|k| pick_for(&healthy, k) == "10.0.0.11:80")
            .expect("some key maps to the first backend");
        let with_drain = both(drained, serde_json::json!({}));
        assert_eq!(pick_for(&with_drain, key), "10.0.0.12:80");
        assert_eq!(pick_for(&healthy, key), "10.0.0.11:80");
    }

    /// Every backend drained: nothing to choose, also for an open service.
    // spectra-scenario: client-lookup / Lookup when all are drained
    // spectra-scenario: backend-selection / Everything drained
    #[test]
    fn all_drained_is_all_down() {
        let drained = serde_json::json!({"state": "unhealthy", "reason": "drained", "drain": true});
        let rec = record(
            "round-robin",
            serde_json::json!({"all_down": true}),
            serde_json::json!([
                backend_json("10.0.0.11:80", drained.clone()),
                backend_json("10.0.0.12:80", drained)
            ]),
        );
        for stale in [false, true] {
            let e = candidates(&rec, stale).expect_err("nothing to choose");
            assert_eq!(e.code, ErrorCode::AllDown);
        }
    }

    /// Spec lookup-command, "Explain shows the tier ...": the rules of the
    /// balance methods, in words.
    #[test]
    fn describe_names_the_rule_of_every_balance() {
        let bes = || {
            serde_json::json!([
                backend_json(
                    "10.0.0.11:80",
                    serde_json::json!({"weight": 3, "site": "tpe1"})
                ),
                backend_json(
                    "10.0.0.12:80",
                    serde_json::json!({"weight": 1, "site": "khh1"})
                ),
            ])
        };
        let now = 1_700_000_000_000;
        let ex = |balance: &str, extra, key, site| {
            describe(&record(balance, extra, bes()), false, key, site, now, false)
        };
        let none = serde_json::json!({});
        let e = ex("failover", none.clone(), None, None);
        assert!(e.selection.deterministic);
        assert_eq!(e.selection.chosen.as_deref(), Some("10.0.0.11:80"));
        let e = ex("weighted", none.clone(), None, None);
        assert!(!e.selection.deterministic);
        assert!(e.selection.rule.contains("(3:1)"), "{}", e.selection.rule);
        let e = ex("locality", none.clone(), None, Some("khh1"));
        assert_eq!(e.candidates, ["10.0.0.12:80"]);
        assert_eq!(
            e.excluded,
            [("10.0.0.11:80".to_string(), "not at this site".to_string())]
        );
        assert!(e.selection.rule.contains("khh1"), "{}", e.selection.rule);
        let e = ex("locality", none.clone(), None, Some("other"));
        assert_eq!(e.candidates.len(), 2);
        assert!(e.selection.rule.contains("none is at this site"));
        let e = ex(
            "fastest",
            serde_json::json!({"pool": ["10.0.0.12:80"]}),
            None,
            None,
        );
        assert_eq!(e.candidates, ["10.0.0.12:80"]);
        assert_eq!(
            e.excluded,
            [("10.0.0.11:80".to_string(), "not in the pool".to_string())]
        );
        assert!(e.selection.rule.contains("pool"));
        let e = ex("fastest", serde_json::json!({}), None, None);
        assert_eq!(e.candidates.len(), 2);
        assert!(
            e.selection.rule.contains("not used"),
            "{}",
            e.selection.rule
        );
        let e = ex("sticky", none.clone(), None, None);
        assert!(!e.selection.deterministic);
        assert!(e.selection.rule.contains("no key"));
        let e = ex("something-new", none, None, None);
        assert!(
            e.selection.rule.contains("does not know"),
            "{}",
            e.selection.rule
        );
    }

    /// Spec "Slow start in lookups": a candidate in slow start shows its
    /// step and effective weight.
    #[test]
    fn describe_shows_slow_start() {
        let now = 1_700_000_100_000;
        let bes = serde_json::json!([
            backend_json("10.0.0.11:80", serde_json::json!({"weight": 10})),
            backend_json(
                "10.0.0.12:80",
                serde_json::json!({"weight": 10, "recovered_at": now - 10_000})
            ),
        ]);
        let rec = record(
            "weighted",
            serde_json::json!({"slow_start_ms": 30_000}),
            bes,
        );
        let e = describe(&rec, false, None, None, now, false);
        assert_eq!(e.slow_start.len(), 1);
        assert_eq!(e.slow_start[0].0, "10.0.0.12:80");
        assert_eq!(e.slow_start[0].1, 4);
        assert_eq!(e.slow_start[0].2, 40);
        // A stale store has no slow start.
        let e = describe(&rec, true, None, None, now, false);
        assert!(e.slow_start.is_empty());
    }

    /// The daemon's DNS server picks by smooth weighted round-robin whenever
    /// a backend is in slow start, whatever the `balance`: the rule says so
    /// with `smooth`, and the client's rule does not.
    #[test]
    fn describe_names_the_rotation_style() {
        let now = 1_700_000_100_000;
        let bes = serde_json::json!([
            backend_json("10.0.0.11:80", serde_json::json!({})),
            backend_json(
                "10.0.0.12:80",
                serde_json::json!({"recovered_at": now - 10_000})
            ),
        ]);
        let rec = record(
            "round-robin",
            serde_json::json!({"slow_start_ms": 30_000}),
            bes,
        );
        let client = describe(&rec, false, None, None, now, false);
        assert!(
            client
                .selection
                .rule
                .contains("slow start: the candidates in turn"),
            "{}",
            client.selection.rule
        );
        let dns = describe(&rec, false, None, None, now, true);
        assert!(
            dns.selection
                .rule
                .contains("slow start: smooth weighted round-robin"),
            "{}",
            dns.selection.rule
        );
        let weighted = record(
            "weighted",
            serde_json::json!({}),
            serde_json::json!([backend_json("10.0.0.11:80", serde_json::json!({})),]),
        );
        assert!(
            describe(&weighted, false, None, None, now, true)
                .selection
                .rule
                .contains("smooth weighted round-robin in proportion")
        );
        assert!(
            describe(&weighted, false, None, None, now, false)
                .selection
                .rule
                .contains("the candidates in turn in proportion")
        );
    }
}
