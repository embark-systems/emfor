//! Runs the conformance cases through the C API (`emfor_lookup` and friends,
//! called as `extern "C"` functions), and checks the C status mapping.
#![cfg(all(feature = "capi", feature = "testing"))]

mod support;

use std::ffi::{CStr, CString, c_char};
use std::ptr;

use emfor_client::capi::{
    EMFOR_ADDR_MAX, emfor_last_error, emfor_lookup, emfor_status, emfor_status_name,
};
use emfor_client::testing::use_store;
use serde_json::{Value, json};
use support::{case_max_staleness, check, data, write_case_store};

fn cstr(p: *const c_char) -> String {
    // SAFETY: the C API returns valid NUL-terminated strings.
    unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string()
}

/// Call `emfor_lookup` with a buffer of `len` bytes filled with `'#'`.
fn c_lookup(name: Option<&str>, key: Option<&str>, len: usize) -> (emfor_status, Vec<u8>) {
    let name = name.map(|n| CString::new(n).unwrap());
    let key = key.map(|k| CString::new(k).unwrap());
    let mut out = vec![b'#'; len];
    // SAFETY: valid C strings or NULL; `out` holds `len` bytes.
    let st = unsafe {
        emfor_lookup(
            name.as_ref().map_or(ptr::null(), |n| n.as_ptr()),
            key.as_ref().map_or(ptr::null(), |k| k.as_ptr()),
            out.as_mut_ptr() as *mut c_char,
            len,
        )
    };
    (st, out)
}

/// Lookup through the C API, shaped for [`check`].
fn call(name: &str, key: Option<&str>) -> Result<String, (String, String)> {
    let (st, out) = c_lookup(Some(name), key, EMFOR_ADDR_MAX);
    if st == emfor_status::EMFOR_OK {
        let n = out.iter().position(|&b| b == 0).expect("NUL-terminated");
        Ok(String::from_utf8(out[..n].to_vec()).unwrap())
    } else {
        Err((cstr(emfor_status_name(st as i32)), cstr(emfor_last_error())))
    }
}

// One test function: the C API uses the process's single client, so the
// cases run one after another, each pointing it at its own store.
#[test]
fn conformance_and_status_mapping_through_the_c_api() {
    let dir = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();

    let cases = data("cases.json");
    let cases = cases.as_array().unwrap();
    for (i, case) in cases.iter().enumerate() {
        let path = dir.path().join(format!("case-{i}.mdb"));
        write_case_store(case, &path);
        use_store(&path, &case_max_staleness(case));
        let name = case["case"].as_str().unwrap();
        for l in case["lookups"].as_array().unwrap() {
            let key = l.get("affinity_key").and_then(Value::as_str);
            let svc = l["name"].as_str().unwrap();
            failures.extend(check(name, l, || call(svc, key)));
        }
    }

    // Sticky pick vectors, as lookups of a sticky service.
    let v = data("sticky-v1.json");
    let picks = v["picks"].as_array().unwrap();
    for (i, p) in picks.iter().enumerate() {
        let backends: Vec<Value> = p["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| {
                let id = id.as_str().unwrap();
                let (ip, port) = id.rsplit_once(':').unwrap();
                json!({"id": id, "ip": ip, "port": port.parse::<u16>().unwrap(),
                       "state": "healthy", "checked_at": 1, "reason": null})
            })
            .collect();
        let store = json!({"schema_version": "1", "heartbeat": "fresh", "services": [
            {"name": "s.svc.emfor", "on_all_down": "closed", "balance": "sticky",
             "all_down": false, "backends": backends}]});
        let path = dir.path().join(format!("pick-{i}.mdb"));
        emfor_fixture::write_store(&store, 3000, emfor_fixture::now_ms(), &path).unwrap();
        use_store(&path, "3s");
        let lookup =
            json!({"name": "s.svc.emfor", "affinity_key": p["key"], "expect": p["expect"]});
        failures.extend(check("sticky pick", &lookup, || {
            call("s.svc.emfor", p["key"].as_str())
        }));
    }

    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(cases.len() >= 30 && picks.len() >= 4);

    // Spec example "C status mapping": every row, through the C API.
    let path = dir.path().join("mapping.mdb");
    support::write_store(&path);
    use_store(&path, "3s");
    let (st, out) = c_lookup(Some("api.svc.emfor"), None, EMFOR_ADDR_MAX);
    assert_eq!(st, emfor_status::EMFOR_OK);
    assert!(
        support::API
            .iter()
            .any(|a| out.starts_with(a.as_bytes()) && out[a.len()] == 0)
    );
    for (status, want) in [
        (emfor_status::EMFOR_OK, "OK"),
        (emfor_status::EMFOR_UNAVAILABLE, "UNAVAILABLE"),
        (emfor_status::EMFOR_INCOMPATIBLE, "INCOMPATIBLE"),
        (emfor_status::EMFOR_NOT_FOUND, "NOT_FOUND"),
        (emfor_status::EMFOR_ALL_DOWN, "ALL_DOWN"),
        (emfor_status::EMFOR_INVALID_ARGUMENT, "INVALID_ARGUMENT"),
    ] {
        assert_eq!(cstr(emfor_status_name(status as i32)), want);
    }
    assert_eq!(cstr(emfor_status_name(99)), "UNKNOWN");
    assert_eq!(cstr(emfor_status_name(-1)), "UNKNOWN");

    assert_eq!(
        c_lookup(Some("nope.svc.emfor"), None, EMFOR_ADDR_MAX).0,
        emfor_status::EMFOR_NOT_FOUND
    );

    // NULL name.
    let (st, out) = c_lookup(None, None, EMFOR_ADDR_MAX);
    assert_eq!(st, emfor_status::EMFOR_INVALID_ARGUMENT);
    assert!(cstr(emfor_last_error()).contains("name is NULL"));
    assert!(out.iter().all(|&b| b == b'#'), "out unchanged on failure");

    // An 8-byte buffer for a 14-byte address: rejected, `out` untouched.
    let (st, out) = c_lookup(Some("api.svc.emfor"), None, 8);
    assert_eq!(st, emfor_status::EMFOR_INVALID_ARGUMENT);
    assert!(
        cstr(emfor_last_error()).contains("needs 15 bytes"),
        "{}",
        cstr(emfor_last_error())
    );
    assert!(out.iter().all(|&b| b == b'#'));
    // Exactly long enough (address plus NUL) succeeds.
    assert_eq!(
        c_lookup(Some("api.svc.emfor"), None, 15).0,
        emfor_status::EMFOR_OK
    );

    // NULL out.
    let name = CString::new("api.svc.emfor").unwrap();
    // SAFETY: `out` NULL is the case under test.
    let st = unsafe { emfor_lookup(name.as_ptr(), ptr::null(), ptr::null_mut(), 64) };
    assert_eq!(st, emfor_status::EMFOR_INVALID_ARGUMENT);

    // Not UTF-8.
    let bad = [0xffu8, 0];
    // SAFETY: NUL-terminated bytes.
    let st = unsafe {
        emfor_lookup(
            bad.as_ptr() as *const c_char,
            ptr::null(),
            ptr::null_mut(),
            0,
        )
    };
    assert_eq!(st, emfor_status::EMFOR_INVALID_ARGUMENT);
    assert!(cstr(emfor_last_error()).contains("UTF-8"));

    // The message is per thread: a failure here is not seen by another thread.
    let other = std::thread::spawn(|| cstr(emfor_last_error()))
        .join()
        .unwrap();
    assert_eq!(other, "");

    // Store cannot be opened, and schema version 2.
    use_store(dir.path().join("missing.mdb"), "3s");
    assert_eq!(
        c_lookup(Some("api.svc.emfor"), None, 64).0,
        emfor_status::EMFOR_UNAVAILABLE
    );
    let mut store = support::store_json();
    store["schema_version"] = json!("2");
    let path = dir.path().join("v2.mdb");
    emfor_fixture::write_store(&store, 3000, emfor_fixture::now_ms(), &path).unwrap();
    use_store(&path, "3s");
    assert_eq!(
        c_lookup(Some("api.svc.emfor"), None, 64).0,
        emfor_status::EMFOR_INCOMPATIBLE
    );

    // Closed service, all down.
    let mut store = support::store_json();
    for b in store["services"][0]["backends"].as_array_mut().unwrap() {
        b["state"] = json!("unhealthy");
    }
    store["services"][0]["all_down"] = json!(true);
    let path = dir.path().join("down.mdb");
    emfor_fixture::write_store(&store, 3000, emfor_fixture::now_ms(), &path).unwrap();
    use_store(&path, "3s");
    assert_eq!(
        c_lookup(Some("api.svc.emfor"), None, 64).0,
        emfor_status::EMFOR_ALL_DOWN
    );
}
