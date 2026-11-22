//! Spec scenario "Future heartbeat of a stopped daemon": the clock of the
//! reader was set back 60 seconds after the daemon last wrote its heartbeat
//! (so the heartbeat is 60 s ahead of the reader), and the daemon is gone.
#![cfg(feature = "testing")]

mod support;

use emfor_client::ErrorCode;
use emfor_client::testing::Client;

fn client_with_heartbeat_ahead(on_all_down: &str, ahead_ms: u64) -> (tempfile::TempDir, Client) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.mdb");
    let mut store = support::store_json();
    store["services"][0]["on_all_down"] = on_all_down.into();
    let heartbeat = emfor_fixture::now_ms() + ahead_ms;
    emfor_fixture::write_store(&store, 3000, heartbeat, &path).unwrap();
    (dir, Client::with_max_staleness_ms(path, 3000))
}

// spectra-scenario: client-lookup / Future heartbeat of a stopped daemon
#[test]
fn heartbeat_sixty_seconds_ahead_is_stale() {
    let (_dir, closed) = client_with_heartbeat_ahead("closed", 60_000);
    let e = closed.lookup("api.svc.emfor", None).unwrap_err();
    assert_eq!(e.code, ErrorCode::AllDown, "{e}");

    let (_dir, open) = client_with_heartbeat_ahead("open", 60_000);
    let got = open.lookup("api.svc.emfor", None).unwrap();
    assert!(support::API.contains(&got.as_str()), "{got}");
}

#[test]
fn heartbeat_a_little_ahead_is_fresh() {
    let (_dir, closed) = client_with_heartbeat_ahead("closed", 1_000);
    let got = closed.lookup("api.svc.emfor", None).unwrap();
    assert!(support::API.contains(&got.as_str()), "{got}");
}
