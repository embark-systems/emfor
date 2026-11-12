//! Spec example "EMFOR_MAX_STALENESS parsing", row `unset → 3000 ms`: with the
//! variable unset, a heartbeat 2.5s old is fresh and one over 3s old is stale.
mod support;

use emfor_client::{ErrorCode, lookup};

#[test]
fn unset_staleness_is_three_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    // The service is `closed`, so a stale store gives ALL_DOWN.
    let written = emfor_fixture::now_ms() - 2500;
    emfor_fixture::write_store(&support::store_json(), 3000, written, &path).unwrap();
    support::set_env(&path, None);
    assert!(support::API.contains(&lookup("api.svc.emfor", None).unwrap().as_str()));
    let age = emfor_fixture::now_ms() - written;
    assert!(age < 3000, "too slow to test the fresh side: {age} ms");
    std::thread::sleep(std::time::Duration::from_millis(3200 - age));
    let e = lookup("api.svc.emfor", None).unwrap_err();
    assert_eq!(e.code, ErrorCode::AllDown, "{e}");
}
