//! `emfor-fixture <cases.json> <index> <out.mdb>`: write the store of one
//! conformance case to an LMDB file. See the library docs.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: emfor-fixture <cases.json> <index> <out.mdb>");
        std::process::exit(2);
    }
    let run = || -> Result<(), String> {
        let index: usize = args[2]
            .parse()
            .map_err(|_| format!("bad index {:?}", args[2]))?;
        let case = emfor_fixture::load_case(args[1].as_ref(), index)?;
        let store = case
            .get("store")
            .filter(|s| !s.is_null())
            .ok_or("case has no store (\"store\": null)")?;
        let max = case
            .get("max_staleness_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(3000);
        emfor_fixture::write_store(store, max, emfor_fixture::now_ms(), args[3].as_ref())
    };
    if let Err(e) = run() {
        eprintln!("emfor-fixture: {e}");
        std::process::exit(1);
    }
}
