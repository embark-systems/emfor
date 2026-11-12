//! Checks the shared conformance data in tests/conformance/v1/: every case's
//! store can be written, and the cases cover what the client-lookup spec
//! asks of them.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;

fn data(file: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/v1")
        .join(file);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

#[test]
fn every_store_writes_and_coverage_is_complete() {
    let cases = data("cases.json");
    let cases = cases.as_array().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (mut heartbeats, mut codes, mut balances, mut on_all_down) = (
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
    );
    let mut lookups = 0;
    for (i, case) in cases.iter().enumerate() {
        let name = case["case"].as_str().unwrap();
        let store = &case["store"];
        if !store.is_null() {
            let path = dir.path().join(format!("{i}.mdb"));
            emfor_fixture::write_store(
                store,
                case["max_staleness_ms"].as_u64().unwrap(),
                emfor_fixture::now_ms(),
                &path,
            )
            .unwrap_or_else(|e| panic!("case {name}: {e}"));
            heartbeats.insert(store["heartbeat"].as_str().unwrap().to_string());
            for svc in store["services"].as_array().unwrap() {
                balances.insert(
                    svc.get("balance")
                        .and_then(Value::as_str)
                        .unwrap_or("(absent)")
                        .to_string(),
                );
                on_all_down.insert(svc["on_all_down"].as_str().unwrap().to_string());
            }
        }
        for l in case["lookups"].as_array().unwrap() {
            lookups += 1;
            let kinds = ["expect", "expect_error", "expect_all_of", "expect_counts"]
                .iter()
                .filter(|k| l.get(**k).is_some())
                .count();
            assert_eq!(
                kinds, 1,
                "case {name}: each lookup needs exactly one expectation"
            );
            if let Some(c) = l.get("expect_error") {
                codes.insert(c.as_str().unwrap().to_string());
            }
        }
    }
    let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
    assert_eq!(heartbeats, set(&["fresh", "missing", "stale", "zero"]));
    assert_eq!(
        codes,
        set(&["ALL_DOWN", "INCOMPATIBLE", "NOT_FOUND", "UNAVAILABLE"])
    );
    assert!(
        set(&[
            "round-robin",
            "sticky",
            "failover",
            "weighted",
            "locality",
            "fastest",
            "some-future-method",
            "(absent)"
        ])
        .is_subset(&balances),
        "{balances:?}"
    );
    assert_eq!(on_all_down, set(&["closed", "open"]));
    assert!(
        cases.iter().any(|c| c["store"].is_null()),
        "a missing-store case"
    );
    assert!(
        cases.iter().any(|c| c.get("max_staleness").is_some()),
        "an EMFOR_MAX_STALENESS case"
    );
    assert!(lookups >= 40, "{lookups} lookups");
}

#[test]
fn sticky_vectors_are_well_formed() {
    let v = data("sticky-v1.json");
    assert_eq!(v["version"], 1);
    for h in v["hashes"].as_array().unwrap() {
        assert!(h["hash"].as_u64().unwrap() <= u32::MAX as u64);
        assert!(h["key"].is_string() && h["id"].is_string());
    }
    for p in v["picks"].as_array().unwrap() {
        let ids: Vec<&str> = p["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.as_str().unwrap())
            .collect();
        assert!(ids.contains(&p["expect"].as_str().unwrap()));
    }
}
