//! The client must use the daemon's LMDB build: Cargo.lock may hold only one
//! lmdb-master-sys, which heed (in the daemon) also uses.

#[test]
fn one_lmdb_in_the_workspace() {
    let lock =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock")).unwrap();
    let n = lock.matches("name = \"lmdb-master-sys\"").count();
    assert_eq!(
        n, 1,
        "Cargo.lock has {n} versions of lmdb-master-sys; the client and daemon must share one"
    );
}
