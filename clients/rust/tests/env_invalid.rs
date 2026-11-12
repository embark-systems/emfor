//! An invalid EMFOR_MAX_STALENESS fails every lookup with UNAVAILABLE and a
//! message naming the variable; it is never replaced by the default.
mod support;

use emfor_client::{ErrorCode, lookup};

#[test]
fn invalid_staleness_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::write_store(&path);
    support::set_env(&path, Some("1.5s"));
    for _ in 0..2 {
        let e = lookup("api.svc.emfor", None).unwrap_err();
        assert_eq!(e.code, ErrorCode::Unavailable);
        assert!(e.message.contains("EMFOR_MAX_STALENESS"), "{e}");
    }
}
