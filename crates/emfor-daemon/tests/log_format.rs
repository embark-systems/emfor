//! `[log] format`: the text or JSON lines of the daemon, and what a reload
//! may change.

mod common;

use common::Daemon;
use nix::sys::signal::Signal;

const SERVICE: &str = "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"127.0.0.1:9\"]\ninterval = \"1s\"\ntimeout = \"500ms\"\n";

fn config(log: &str) -> String {
    format!("[log]\n{log}\n{SERVICE}")
}

/// A reload that changes `format` is rejected, the daemon goes on with the
/// old config and its lines stay text; one that changes only `level` is
/// applied.
// spectra-scenario: service-config / Reload changes the format
// spectra-scenario: service-config / Reload changes only the level
#[test]
fn reload_cannot_change_the_format_but_can_change_the_level() {
    let d = Daemon::spawn(&config("format = \"text\""));
    d.wait_log("emfor started");

    d.write_config(&config("format = \"json\""));
    d.signal(Signal::SIGHUP);
    d.wait_log("reload rejected");
    let log = d.log();
    assert!(
        log.contains("log format settings require restart") && log.contains("log.format"),
        "{log}"
    );
    assert!(!log.contains("\"level\":"), "lines stay text: {log}");

    d.write_config(&config("format = \"text\"\nlevel = \"debug\""));
    d.signal(Signal::SIGHUP);
    d.wait_log("log level set to debug");
    d.wait_log("config reloaded");
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
}

/// The lines after `emfor started` of a daemon with `format = "json"`.
fn json_lines(log: &str) -> Vec<serde_json::Value> {
    let start = log.find("emfor started").expect("the daemon started");
    let from = log[..start].rfind('\n').map_or(0, |i| i + 1);
    log[from..]
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}\n{log}")))
        .collect()
}

/// With `format = "json"` every line after the config is read is one JSON
/// object, the fields of the event at its top level; a reload's audit lines
/// carry `change`.
// spectra-scenario: service-config / JSON lines
#[test]
fn json_format_writes_one_object_per_line() {
    let d = Daemon::spawn(&config("format = \"json\""));
    d.wait_log("emfor started");
    d.write_config(&format!(
        "[log]\nformat = \"json\"\n{}",
        SERVICE.replace("127.0.0.1:9", "127.0.0.1:9\", \"127.0.0.1:10")
    ));
    d.signal(Signal::SIGHUP);
    d.wait_log("config reloaded");
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
    let lines = json_lines(&log);
    for l in &lines {
        for key in ["timestamp", "level", "message"] {
            assert!(l.get(key).is_some(), "{key} in {l}");
        }
        assert!(
            l.get("fields").is_none(),
            "event fields are not nested: {l}"
        );
    }
    let added = lines
        .iter()
        .find(|l| l["change"] == "backend_added")
        .unwrap_or_else(|| panic!("an audit line in {log}"));
    assert_eq!(added["service"], "api.svc.emfor");
    assert_eq!(added["backend"], "127.0.0.1:10");
}

/// The `config file read` lines of a start are written once the config is
/// accepted, so with `format = "json"` they are JSON objects with `file` and
/// `sha256`.
// spectra-scenario: service-config / Config file read lines at start
#[test]
fn json_format_applies_to_the_config_file_read_lines_of_a_start() {
    let d = Daemon::spawn(&config("format = \"json\""));
    d.wait_log("emfor started");
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
    let read: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("config file read"))
        .collect();
    assert!(!read.is_empty(), "a config file read line in {log}");
    for l in read {
        let v: serde_json::Value =
            serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}"));
        assert_eq!(v["level"], "INFO", "{l}");
        assert!(
            v["file"].as_str().is_some_and(|f| f.ends_with(".toml")),
            "{l}"
        );
        let sha = v["sha256"]
            .as_str()
            .unwrap_or_else(|| panic!("sha256 in {l}"));
        assert_eq!(sha.len(), 64, "{l}");
    }
}

/// The `config file read` lines of a start are `info` lines that are written
/// whatever `[log] level` is.
// spectra-scenario: service-config / Config file read lines at start
#[test]
fn config_file_read_lines_of_a_start_ignore_the_log_level() {
    let d = Daemon::spawn(&config("level = \"warn\""));
    d.wait_log("config file read");
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
    assert!(log.contains("sha256="), "{log}");
}

/// Without `format` the lines are text.
// spectra-scenario: service-config / Format not set
#[test]
fn text_is_the_default() {
    let d = Daemon::spawn(&config(""));
    d.wait_log("emfor started");
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
    assert!(log.lines().all(|l| !l.starts_with('{')), "{log}");
}

/// The state change line of a daemon with `format = "json"` has the stable
/// fields at its top level.
// spectra-scenario: service-config / JSON lines
#[test]
fn json_state_change_line_has_the_stable_fields() {
    let d = Daemon::spawn(&config("format = \"json\""));
    d.wait_log("changed from unknown to unhealthy");
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
    let line = json_lines(&log)
        .into_iter()
        .find(|l| l["event"] == "backend_state_changed")
        .unwrap_or_else(|| panic!("a state change line in {log}"));
    assert_eq!(line["level"], "INFO");
    assert_eq!(line["service"], "api.svc.emfor");
    assert_eq!(line["backend"], "127.0.0.1:9");
    assert_eq!(line["from"], "unknown");
    assert_eq!(line["to"], "unhealthy");
    assert!(line["reason"].is_string(), "{line}");
    assert!(line["timestamp"].is_string(), "{line}");
}
