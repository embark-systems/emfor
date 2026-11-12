//! Bakes the default config path into the daemon from `EMFOR_PREFIX`.

include!("build/prefix.rs");

fn main() {
    println!("cargo:rerun-if-env-changed=EMFOR_PREFIX");
    println!("cargo:rerun-if-changed=build/prefix.rs");
    let prefix = std::env::var("EMFOR_PREFIX").ok();
    match default_config_path(prefix.as_deref()) {
        Ok(path) => println!("cargo:rustc-env=EMFOR_DEFAULT_CONFIG_PATH={path}"),
        Err(e) => panic!("{e}"),
    }
}
