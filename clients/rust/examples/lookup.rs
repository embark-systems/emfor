//! `cargo run -p emfor-client --example lookup -- <service> [affinity-key]`:
//! look up a backend and print it.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 2 {
        eprintln!("usage: lookup <service> [affinity-key]");
        std::process::exit(2);
    }
    match emfor_client::lookup(&args[0], args.get(1).map(String::as_str)) {
        Ok(addr) => println!("{addr}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
