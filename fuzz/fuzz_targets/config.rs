//! An emfor.toml.
#![no_main]

use std::sync::OnceLock;

use emfor_daemon::checker::Registry;

libfuzzer_sys::fuzz_target!(|text: &str| {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    let registry = REGISTRY.get_or_init(Registry::builtin);
    emfor_fuzz::on_worker_stack(|| {
        let _ = emfor_daemon::config::parse(text, registry);
    });
});
