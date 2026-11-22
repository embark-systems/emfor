//! The audit lines of the daemon's log: the hash of every config file read,
//! and what a reload changed.

mod common;

use common::Daemon;
use nix::sys::signal::Signal;
use sha2::{Digest, Sha256};

fn sha256sum(path: &std::path::Path) -> String {
    let hash = Sha256::digest(std::fs::read(path).unwrap());
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_line(log: &str, sha: &str) -> bool {
    log.lines()
        .any(|l| l.contains("config file read") && l.contains(&format!("sha256={sha}")))
}

// spectra-scenario: service-config / Daemon logs the hash of every file read
// spectra-scenario: service-config / Rejected reload still logs the hashes
#[test]
fn every_file_read_is_logged_with_its_hash() {
    let d = Daemon::spawn("");
    d.wait_log("emfor started");
    let first = sha256sum(&d.config());
    assert!(read_line(&d.log(), &first), "{}", d.log());

    // A reload that is accepted: the new content, the new hash.
    d.write_config("# edited\n");
    let second = sha256sum(&d.config());
    assert_ne!(first, second);
    d.signal(Signal::SIGHUP);
    d.wait_log("config reloaded");
    assert!(read_line(&d.log(), &second), "{}", d.log());

    // One that validation rejects still logs what was read.
    d.write_config("[services.a]\ncheck = \"nope\"\n");
    let third = sha256sum(&d.config());
    d.signal(Signal::SIGHUP);
    d.wait_log("reload rejected");
    assert!(read_line(&d.log(), &third), "{}", d.log());

    // And one whose file does not even parse: it was read all the same.
    d.write_config("this is = not [ toml\n");
    let fourth = sha256sum(&d.config());
    d.signal(Signal::SIGHUP);
    common::wait_within(
        "the fourth reload",
        std::time::Duration::from_secs(10),
        || d.log().matches("reload rejected").count() >= 2,
    );
    assert!(read_line(&d.log(), &fourth), "{}", d.log());
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
}

fn service(backends: &str, options: &str) -> String {
    format!(
        "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [{backends}]\ninterval = \"1s\"\ntimeout = \"500ms\"\n[services.\"api.svc.emfor\".check_options]\n{options}\n"
    )
}

// spectra-scenario: service-config / Backend added and another drained
// spectra-scenario: service-config / Check options changed without exposing them
// spectra-scenario: service-config / No difference
#[test]
fn a_reload_logs_what_it_changed_before_it_says_reloaded() {
    let d = Daemon::spawn(&service(
        "\"10.0.0.11:8080\"",
        "send = \"first-secret-text\"\nexpect = \"x\"",
    ));
    d.wait_log("emfor started");

    // No difference: the reload line and no `change=` line.
    d.signal(Signal::SIGHUP);
    d.wait_log("config reloaded");
    assert!(!d.log().contains("change="), "{}", d.log());

    d.write_config(&service(
        "{ addr = \"10.0.0.11:8080\", drain = true }, \"10.0.0.13:8080\"",
        "send = \"second-secret-text\"\nexpect = \"x\"",
    ));
    d.signal(Signal::SIGHUP);
    wait_for_second_reload(&d);
    let log = d.log();
    let reloaded = log.rfind("config reloaded").unwrap();
    for needle in [
        "change=backend_changed",
        "field=drain",
        "to=true",
        "change=backend_added",
        "backend=10.0.0.13:8080",
        "change=check_definition_changed",
        "resets_state=true",
    ] {
        let at = log
            .find(needle)
            .unwrap_or_else(|| panic!("{needle}: {log}"));
        assert!(at < reloaded, "{needle} after the reload line: {log}");
    }
    assert!(
        !log.contains("first-secret-text") && !log.contains("second-secret-text"),
        "{log}"
    );
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
}

fn wait_for_second_reload(d: &Daemon) {
    common::wait_within(
        "the second reload",
        std::time::Duration::from_secs(10),
        || d.log().matches("config reloaded").count() >= 2,
    );
}
