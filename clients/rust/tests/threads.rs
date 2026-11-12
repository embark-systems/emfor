//! 8 threads × 1000 lookups of a round-robin service with 2 healthy
//! backends: every lookup returns one of them, and both are used.
mod support;

use std::collections::HashMap;

use emfor_client::lookup;

#[test]
fn concurrent_lookups() {
    let dir = tempfile::tempdir().unwrap();
    let path = support::tempdir_path(&dir);
    support::write_store(&path);
    support::set_env(&path, None);
    let handles: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                let mut seen = HashMap::new();
                for _ in 0..1000 {
                    *seen
                        .entry(lookup("api.svc.emfor", None).unwrap())
                        .or_insert(0) += 1;
                }
                seen
            })
        })
        .collect();
    let mut total: HashMap<String, u32> = HashMap::new();
    for h in handles {
        for (k, v) in h.join().unwrap() {
            *total.entry(k).or_default() += v;
        }
    }
    assert_eq!(total.values().sum::<u32>(), 8000);
    assert_eq!(total.len(), 2, "{total:?}");
    // One shared atomic counter: the split is exact.
    assert!(total.values().all(|&n| n == 4000), "{total:?}");
}
