//! Shared helpers for the integration tests (included with `mod support;`).
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub const API: [&str; 2] = ["10.0.0.11:8080", "10.0.0.12:8080"];

/// A fresh store with round-robin service `api.svc.emfor` on two healthy backends.
pub fn store_json() -> serde_json::Value {
    let be = |id: &str| {
        let (ip, port) = id.split_once(':').unwrap();
        serde_json::json!({"id": id, "ip": ip, "port": port.parse::<u16>().unwrap(),
                           "state": "healthy", "checked_at": 1, "reason": null})
    };
    serde_json::json!({"schema_version": "1", "heartbeat": "fresh", "services": [
        {"name": "api.svc.emfor", "on_all_down": "closed", "balance": "round-robin", "all_down": false,
         "backends": [be(API[0]), be(API[1])]}]})
}

pub fn write_store(path: &Path) {
    emfor_fixture::write_store(&store_json(), 3000, emfor_fixture::now_ms(), path).unwrap();
}

/// Point the process's client at `path` (before its first lookup).
pub fn set_env(path: &Path, max_staleness: Option<&str>) {
    // SAFETY: called at the start of a single-test binary, before any thread
    // that reads the environment exists.
    unsafe {
        std::env::set_var("EMFOR_STORE", path);
        match max_staleness {
            Some(v) => std::env::set_var("EMFOR_MAX_STALENESS", v),
            None => std::env::remove_var("EMFOR_MAX_STALENESS"),
        }
    }
}

pub fn tempdir_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("state.mdb")
}

/// A file from tests/conformance/v1/.
pub fn data(file: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/v1")
        .join(file);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Write a case's store (if it has one) to `path`.
pub fn write_case_store(case: &Value, path: &Path) {
    if !case["store"].is_null() {
        let max_ms = case["max_staleness_ms"].as_u64().unwrap();
        emfor_fixture::write_store(&case["store"], max_ms, emfor_fixture::now_ms(), path).unwrap();
    }
}

/// A case's staleness setting in `EMFOR_MAX_STALENESS` form.
pub fn case_max_staleness(case: &Value) -> String {
    match case.get("max_staleness").and_then(Value::as_str) {
        Some(raw) => raw.to_string(),
        None => format!("{}ms", case["max_staleness_ms"].as_u64().unwrap()),
    }
}

/// Check one conformance lookup; return a failure message, or `None` when
/// it passes. `call` returns the address or `(error code name, message)`.
pub fn check(
    name: &str,
    lookup: &Value,
    mut call: impl FnMut() -> Result<String, (String, String)>,
) -> Option<String> {
    let what = format!("case {name:?}, lookup {lookup}");
    if let Some(want) = lookup.get("expect").and_then(Value::as_str) {
        return match call() {
            Ok(got) if got == want => None,
            other => Some(format!("{what}: got {other:?}")),
        };
    }
    if let Some(want) = lookup.get("expect_error").and_then(Value::as_str) {
        return match call() {
            Err((code, _)) if code == want => None,
            other => Some(format!("{what}: got {other:?}")),
        };
    }
    if let Some(want) = lookup.get("expect_counts").and_then(Value::as_object) {
        let want: std::collections::BTreeMap<String, u64> = want
            .iter()
            .map(|(k, v)| (k.clone(), v.as_u64().unwrap()))
            .collect();
        let mut got = std::collections::BTreeMap::new();
        for _ in 0..want.values().sum::<u64>() {
            match call() {
                Ok(a) => *got.entry(a).or_insert(0) += 1,
                Err(e) => return Some(format!("{what}: got error {e:?}")),
            }
        }
        return (got != want).then(|| format!("{what}: got counts {got:?}"));
    }
    let want: BTreeSet<&str> = lookup["expect_all_of"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let mut got = BTreeSet::new();
    for _ in 0..want.len() {
        match call() {
            Ok(a) => {
                got.insert(a);
            }
            Err(e) => return Some(format!("{what}: got error {e:?}")),
        }
    }
    let got: BTreeSet<&str> = got.iter().map(String::as_str).collect();
    (got != want).then(|| format!("{what}: got {got:?}"))
}
