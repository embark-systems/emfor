//! The process's client reads EMFOR_STORE and EMFOR_MAX_STALENESS, and a
//! store that is missing on the first lookup is picked up once it exists.
mod support;

use emfor_client::{ErrorCode, lookup};

#[test]
fn missing_store_is_retried_and_env_is_used() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::set_env(&path, Some("1500ms"));
    let e = lookup("api.svc.emfor", None).unwrap_err();
    assert_eq!(e.code, ErrorCode::Unavailable, "{e}");
    assert!(e.message.contains(&path.display().to_string()), "{e}");
    support::write_store(&path);
    let got = lookup("api.svc.emfor", None).unwrap();
    assert!(support::API.contains(&got.as_str()), "{got}");
}
