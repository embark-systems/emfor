//! On FreeBSD the library registers `pthread_atfork` handlers (src/fork.rs)
//! that libc keeps until the process ends. A `dlclose` that unloaded the
//! library (a PHP extension is unloaded when PHP shuts it down) would leave
//! them pointing at unmapped code and crash the next `fork`, so the shared
//! library (`libemfor`) is marked not unloadable. Checked on FreeBSD
//! 15.1-RELEASE: without it, dlopen, lookup, dlclose and fork crashed.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("freebsd") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-z,nodelete");
    }
}
