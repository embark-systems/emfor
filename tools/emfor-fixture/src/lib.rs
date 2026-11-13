//! Writes the `store` object of a conformance case (tests/conformance/v1/)
//! into an LMDB file laid out like the daemon's store: one file opened with
//! `MDB_NOSUBDIR`, the default database, UTF-8 keys and values. It uses heed
//! from the daemon's workspace, so the file has the daemon's LMDB format.
//!
//! `store` fields:
//! - `schema_version`: string written to `meta/schema_version`; absent → key absent.
//! - `heartbeat`: `"fresh"` (now), `"stale"` (now − 2 × max staleness),
//!   `"zero"` (`"0"`) or `"missing"` (key absent).
//! - `services`: records written as JSON to `svc/<name>`.
//! - `raw`: extra keys written verbatim, for malformed records.

use std::path::Path;

use emfor_format::{KEY_HEARTBEAT, KEY_SCHEMA_VERSION, service_key};
use heed::types::Str;
use heed::{Database, EnvFlags, EnvOpenOptions};
use serde_json::Value;

/// Current Unix time in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Heartbeat value for a heartbeat form, or `None` for `"missing"`.
pub fn heartbeat_value(
    form: &str,
    now_ms: u64,
    max_staleness_ms: u64,
) -> Result<Option<String>, String> {
    Ok(match form {
        "fresh" => Some(now_ms.to_string()),
        "stale" => Some(now_ms.saturating_sub(2 * max_staleness_ms).to_string()),
        "zero" => Some("0".into()),
        "missing" => None,
        other => return Err(format!("unknown heartbeat form {other:?}")),
    })
}

/// Every key and value a `store` object describes, in write order.
pub fn store_entries(
    store: &Value,
    now_ms: u64,
    max_staleness_ms: u64,
) -> Result<Vec<(String, String)>, String> {
    let obj = store.as_object().ok_or("store must be an object")?;
    let mut out = Vec::new();
    if let Some(v) = obj.get("schema_version") {
        let v = v.as_str().ok_or("schema_version must be a string")?;
        out.push((KEY_SCHEMA_VERSION.to_string(), v.to_string()));
    }
    let form = obj
        .get("heartbeat")
        .and_then(Value::as_str)
        .unwrap_or("fresh");
    if let Some(hb) = heartbeat_value(form, now_ms, max_staleness_ms)? {
        out.push((KEY_HEARTBEAT.to_string(), hb));
    }
    for svc in obj
        .get("services")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = svc
            .get("name")
            .and_then(Value::as_str)
            .ok_or("every service needs a string name")?;
        out.push((service_key(name), svc.to_string()));
    }
    for (k, v) in obj
        .get("raw")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let v = v
            .as_str()
            .ok_or_else(|| format!("raw value of {k} must be a string"))?;
        out.push((k.clone(), v.to_string()));
    }
    Ok(out)
}

/// Write `store` to a new LMDB file at `path` and close it again, so the
/// same process can open the file afterwards (heed allows one environment
/// per file and process).
pub fn write_store(
    store: &Value,
    max_staleness_ms: u64,
    now_ms: u64,
    path: &Path,
) -> Result<(), String> {
    let entries = store_entries(store, now_ms, max_staleness_ms)?;
    let err = |e: heed::Error| format!("{}: {e}", path.display());
    let env = unsafe {
        let mut o = EnvOpenOptions::new();
        o.map_size(4 << 20);
        o.flags(EnvFlags::NO_SUB_DIR);
        // SAFETY: a fresh file written only by this function.
        o.open(path).map_err(err)?
    };
    let mut txn = env.write_txn().map_err(err)?;
    let db: Database<Str, Str> = env.create_database(&mut txn, None).map_err(err)?;
    for (k, v) in &entries {
        db.put(&mut txn, k, v).map_err(err)?;
    }
    txn.commit().map_err(err)?;
    env.prepare_for_closing().wait();
    Ok(())
}

/// Load the case at `index` of a cases file.
pub fn load_case(cases_path: &Path, index: usize) -> Result<Value, String> {
    let text = std::fs::read_to_string(cases_path)
        .map_err(|e| format!("{}: {e}", cases_path.display()))?;
    let cases: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", cases_path.display()))?;
    cases
        .as_array()
        .and_then(|a| a.get(index))
        .cloned()
        .ok_or_else(|| format!("{}: no case {index}", cases_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn read_all(path: &Path) -> BTreeMap<String, String> {
        let env = unsafe {
            let mut o = EnvOpenOptions::new();
            o.flags(EnvFlags::NO_SUB_DIR | EnvFlags::READ_ONLY);
            o.open(path).unwrap()
        };
        let txn = env.read_txn().unwrap();
        let db: Database<Str, Str> = env.open_database(&txn, None).unwrap().unwrap();
        let out = db
            .iter(&txn)
            .unwrap()
            .map(|r| r.map(|(k, v)| (k.to_string(), v.to_string())).unwrap())
            .collect();
        drop(txn);
        env.prepare_for_closing().wait();
        out
    }

    #[test]
    fn writes_every_key_of_a_case() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.mdb");
        let store = serde_json::json!({
            "schema_version": "1",
            "heartbeat": "stale",
            "services": [{"name": "db.svc.emfor", "on_all_down": "closed", "balance": "failover",
                          "all_down": false, "backends": []}],
            "raw": {"svc/bad.svc.emfor": "{not json"}
        });
        write_store(&store, 3000, 1_700_000_010_000, &path).unwrap();
        let all = read_all(&path);
        assert_eq!(all["meta/schema_version"], "1");
        assert_eq!(all["meta/heartbeat"], "1700000004000");
        assert_eq!(all["svc/bad.svc.emfor"], "{not json");
        let rec: Value = serde_json::from_str(&all["svc/db.svc.emfor"]).unwrap();
        assert_eq!(rec["balance"], "failover");
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn heartbeat_forms() {
        assert_eq!(
            heartbeat_value("fresh", 10_000, 3000).unwrap(),
            Some("10000".into())
        );
        assert_eq!(
            heartbeat_value("stale", 10_000, 3000).unwrap(),
            Some("4000".into())
        );
        assert_eq!(
            heartbeat_value("zero", 10_000, 3000).unwrap(),
            Some("0".into())
        );
        assert_eq!(heartbeat_value("missing", 10_000, 3000).unwrap(), None);
        assert!(heartbeat_value("old", 10_000, 3000).is_err());
    }

    #[test]
    fn absent_schema_version_and_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.mdb");
        write_store(&serde_json::json!({"heartbeat": "missing"}), 3000, 1, &path).unwrap();
        assert!(read_all(&path).is_empty());
    }
}
