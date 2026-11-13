//! `emfor-check`: emfor checker plugins for redis, MySQL, PostgreSQL,
//! LDAP and DNS.
//!
//! Usage: `emfor-check redis|mysql|pgsql|ldap|dns`. The argument picks the plugin;
//! the process then speaks plugin protocol 1 on stdin and stdout
//! (docs/plugins/protocol-v1.md). See docs/deploy/plugins.md.

use std::process::ExitCode;

use emfor_check::{Kind, Service};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let kind = match args.as_slice() {
        [name] => Kind::from_name(name),
        _ => None,
    };
    let Some(kind) = kind else {
        eprintln!("usage: emfor-check redis|mysql|pgsql|ldap|dns");
        return ExitCode::from(2);
    };
    // The drivers build their own rustls configurations from the process
    // default provider; make it the same one the shared configuration uses.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    emfor_plugin::run(Service::new(kind))
}
