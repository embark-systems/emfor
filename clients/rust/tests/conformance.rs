//! Runs every case and vector in tests/conformance/v1/ through the Rust API
//! (`testing::Client`, an explicit store path and staleness).
#![cfg(feature = "testing")]

mod support;

use std::path::Path;

use emfor_client::testing::{Client, rendezvous_hash, rendezvous_pick, rendezvous_pick_weighted};
use serde_json::Value;
use support::{check, data};

/// Build a case's store under `dir` and return a client for it.
fn client_for(case: &Value, dir: &Path) -> Client {
    let path = dir.join("state.mdb");
    support::write_case_store(case, &path);
    match case.get("max_staleness").and_then(Value::as_str) {
        Some(raw) => Client::new(path, raw),
        None => Client::with_max_staleness_ms(path, case["max_staleness_ms"].as_u64().unwrap()),
    }
}

// spectra-scenario: client-lookup / IPv4 backend
// spectra-scenario: client-lookup / IPv6 backend
// spectra-scenario: client-lookup / Error code by store condition
// spectra-scenario: client-lookup / Error code by store condition / store conditions
// spectra-scenario: client-lookup / Schema check wins over a missing service
// spectra-scenario: client-lookup / Name forms that find the same record
// spectra-scenario: client-lookup / Name forms that find the same record / name normalization
// spectra-scenario: client-lookup / Stale store with each on_all_down setting
// spectra-scenario: client-lookup / Unhealthy backends are skipped
// spectra-scenario: client-lookup / Open service with every backend down
// spectra-scenario: client-lookup / Open service prefers a passing unknown backend
// spectra-scenario: client-lookup / Open service with every unknown backend failing
// spectra-scenario: client-lookup / Open service with only unhealthy backends
// spectra-scenario: client-lookup / State and on_all_down values from a newer daemon
// spectra-scenario: client-lookup / Failing backend is skipped while another passes
// spectra-scenario: client-lookup / Every healthy backend failing
// spectra-scenario: client-lookup / Stale store ignores reason
// spectra-scenario: client-lookup / Backup takes over while no primary passes
// spectra-scenario: client-lookup / Primary passes again
// spectra-scenario: client-lookup / Consecutive lookups cover every candidate
// spectra-scenario: client-lookup / Unhealthy backend is never returned
// spectra-scenario: client-lookup / First healthy backend wins
// spectra-scenario: client-lookup / Staleness values
// spectra-scenario: client-lookup / Staleness values / EMFOR_MAX_STALENESS parsing
// spectra-scenario: client-lookup / Client passes the shared cases
// spectra-scenario: client-lookup / Future balance value
// spectra-scenario: client-lookup / Unknown fields ignored
// spectra-scenario: client-lookup / Three to one
// spectra-scenario: client-lookup / Unhealthy heavy backend
// spectra-scenario: client-lookup / Record from an older daemon
// spectra-scenario: client-lookup / Local site healthy
// spectra-scenario: client-lookup / Local site healthy / locality outcomes
// spectra-scenario: client-lookup / Lookups stay in the pool
// spectra-scenario: client-lookup / Stale heartbeat ignores the pool
// spectra-scenario: client-lookup / Missing or empty pool
// spectra-scenario: client-lookup / Ramping backend gets a smaller share
// spectra-scenario: client-lookup / Weighted ramping backend
// spectra-scenario: client-lookup / Slow start over
// spectra-scenario: client-lookup / Failover and sticky with a key ignore slow start
// spectra-scenario: client-lookup / Stale store does not ramp
// spectra-scenario: client-lookup / Same key gives the same backend
// spectra-scenario: client-lookup / Equal weights keep today's picks
// spectra-scenario: client-lookup / Equal weights keep today's picks / rendezvous picks
// spectra-scenario: client-lookup / Weights change the share of keys
// spectra-scenario: client-lookup / Weights change the share of keys / weighted picks
// spectra-scenario: client-lookup / Weights above 100 count as 100
// spectra-scenario: client-lookup / Weights above 100 count as 100 / weights above 100
// spectra-scenario: client-lookup / Empty key falls back to round-robin
// spectra-scenario: client-lookup / Lookup with a drained backend
// spectra-scenario: client-lookup / Lookup when all are drained
// spectra-scenario: client-lookup / Older store
#[test]
fn every_case_through_the_rust_api() {
    let cases = data("cases.json");
    let cases = cases.as_array().unwrap();
    let (mut ran, mut failures) = (0, Vec::new());
    for case in cases {
        let dir = tempfile::tempdir().unwrap();
        let client = client_for(case, dir.path());
        let name = case["case"].as_str().unwrap();
        for l in case["lookups"].as_array().unwrap() {
            let key = l.get("affinity_key").and_then(Value::as_str);
            let svc = l["name"].as_str().unwrap();
            let call = || {
                client
                    .lookup(svc, key)
                    .map_err(|e| (e.code.as_str().to_string(), e.message))
            };
            failures.extend(check(name, l, call));
        }
        ran += 1;
    }
    assert_eq!(ran, cases.len());
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// spectra-scenario: client-lookup / Client passes the shared cases
// spectra-scenario: client-lookup / Equal weights keep today's picks
// spectra-scenario: client-lookup / Equal weights keep today's picks / rendezvous hash vectors
// spectra-scenario: client-lookup / Equal weights keep today's picks / rendezvous picks
#[test]
fn sticky_vectors() {
    let v = data("sticky-v1.json");
    let hashes = v["hashes"].as_array().unwrap();
    for h in hashes {
        let (k, id) = (h["key"].as_str().unwrap(), h["id"].as_str().unwrap());
        assert_eq!(
            rendezvous_hash(k, id) as u64,
            h["hash"].as_u64().unwrap(),
            "{h}"
        );
    }
    let picks = v["picks"].as_array().unwrap();
    for p in picks {
        let ids: Vec<&str> = p["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.as_str().unwrap())
            .collect();
        assert_eq!(
            rendezvous_pick(p["key"].as_str().unwrap(), &ids),
            p["expect"].as_str().unwrap(),
            "{p}"
        );
    }
    assert!(hashes.len() >= 6 && picks.len() >= 4);
}

/// Spec client-lookup Example "weighted picks", row `user-42` with weights
/// 3, 1, 1; the same ids with every weight 1 pick as `rendezvous_pick`.
// spectra-scenario: client-lookup / Weights change the share of keys / weighted picks
#[test]
fn weighted_pick() {
    let ids = ["10.0.0.11:8080", "10.0.0.12:8080", "10.0.0.13:8080"];
    let weighted: Vec<(&str, u32)> = ids.iter().copied().zip([3, 1, 1]).collect();
    assert_eq!(
        rendezvous_pick_weighted("user-42", &weighted),
        "10.0.0.11:8080"
    );
    let equal: Vec<(&str, u32)> = ids.iter().map(|id| (*id, 1)).collect();
    assert_eq!(
        rendezvous_pick_weighted("user-42", &equal),
        rendezvous_pick("user-42", &ids)
    );
}
