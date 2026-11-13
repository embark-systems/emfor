//! `emfor-test-plugin`: a protocol 1 plugin for emfor's own tests.
//!
//! Behavior comes from the service's `check_options`:
//! - `mode`: `up` (default), `down`, `error`, `bogus` (invalid `result`),
//!   `numeric` (a `result` that is not a string), `hang` (never answers),
//!   `crash` (exits), `last_words` (answers `down`, then exits at once), or
//!   `slow_first` (the first check
//!   is answered after 300ms, later ones at once, so answers come out of order),
//!   `long_stdout` (writes 70000 bytes to stdout without a line feed instead
//!   of answering), or `long_stderr` (writes a 70000-byte line to stderr,
//!   then answers `up`).
//!
//! Environment switches:
//! - `EMFOR_TEST_PLUGIN_PROTOCOL=<n>`: answer the handshake with protocol `n`.
//! - `EMFOR_TEST_PLUGIN_NO_HELLO=1`: never answer the handshake.
//! - `EMFOR_TEST_PLUGIN_EXIT_AT_START=1`: exit at once with status 3.
//! - `EMFOR_TEST_PLUGIN_BLOCK_FILE=<path>`: exit at once with status 3 while
//!   `<path>` exists, so a test can keep the plugin down across restarts.
//! - `EMFOR_TEST_PLUGIN_VALIDATE_HANG=1`: never answer `validate`.
//! - `EMFOR_TEST_PLUGIN_IGNORE_EOF=1`: keep running after stdin closes.
//! - `EMFOR_TEST_PLUGIN_GARBAGE=1`: write a non-JSON line and an unknown
//!   `type` before every answer.
//!
//! It writes `test plugin started` to stderr at start.

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use emfor_daemon::plugin::protocol::{PROTOCOL, Request, Response, ValidateError};

const MODES: &[&str] = &[
    "up",
    "down",
    "error",
    "bogus",
    "numeric",
    "hang",
    "crash",
    "slow_first",
    "last_words",
    "long_stdout",
    "long_stderr",
];

fn flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| v == "1")
}

fn main() {
    if flag("EMFOR_TEST_PLUGIN_EXIT_AT_START") {
        eprintln!("test plugin exiting at start");
        std::process::exit(3);
    }
    if let Some(path) = std::env::var_os("EMFOR_TEST_PLUGIN_BLOCK_FILE")
        && std::path::Path::new(&path).exists()
    {
        eprintln!("test plugin blocked at start");
        std::process::exit(3);
    }
    eprintln!("test plugin started");
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let garbage = flag("EMFOR_TEST_PLUGIN_GARBAGE");
    let first_check = Arc::new(AtomicBool::new(true));
    let mut workers = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("bad request {line:?}: {e}");
                continue;
            }
        };
        match req {
            Request::Hello { .. } => {
                if flag("EMFOR_TEST_PLUGIN_NO_HELLO") {
                    continue;
                }
                let protocol = std::env::var("EMFOR_TEST_PLUGIN_PROTOCOL")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(PROTOCOL);
                send(
                    &out,
                    false,
                    &Response::Hello {
                        protocol,
                        name: "emfor-test-plugin".into(),
                        version: "1.0.0".into(),
                    },
                );
            }
            Request::Validate { id, options } => {
                if flag("EMFOR_TEST_PLUGIN_VALIDATE_HANG") {
                    continue;
                }
                let mut errors = Vec::new();
                for (k, v) in &options {
                    if k != "mode" {
                        errors.push(ValidateError {
                            key: k.clone(),
                            message: "unknown key".into(),
                        });
                    } else if !v.as_str().is_some_and(|m| MODES.contains(&m)) {
                        errors.push(ValidateError {
                            key: "mode".into(),
                            message: format!("unknown mode {v}"),
                        });
                    }
                }
                send(&out, garbage, &Response::Validate { id, errors });
            }
            Request::Check { id, options, .. } => {
                let mode = options
                    .get("mode")
                    .and_then(|m| m.as_str())
                    .unwrap_or("up")
                    .to_string();
                let delay = if mode == "slow_first" && first_check.swap(false, Ordering::SeqCst) {
                    Duration::from_millis(300)
                } else {
                    Duration::ZERO
                };
                let out = out.clone();
                workers.push(std::thread::spawn(move || {
                    std::thread::sleep(delay);
                    let (result, reason) = match mode.as_str() {
                        "hang" => return,
                        "long_stdout" => {
                            let mut out = out.lock().unwrap();
                            let _ = out.write_all(&[b'x'; 70000]);
                            let _ = out.flush();
                            return;
                        }
                        "long_stderr" => {
                            eprintln!("{}", "y".repeat(70000));
                            ("up".into(), None)
                        }
                        "crash" => {
                            eprintln!("test plugin crashing");
                            std::process::exit(1);
                        }
                        "bogus" => ("maybe".into(), None),
                        "numeric" => (serde_json::Value::from(1), None),
                        "down" | "last_words" => ("down".into(), Some("test plugin says down")),
                        "error" => ("error".into(), Some("test plugin says error")),
                        _ => ("up".into(), None),
                    };
                    send(
                        &out,
                        garbage,
                        &Response::Check {
                            id,
                            result,
                            reason: reason.map(str::to_string),
                        },
                    );
                    if mode == "last_words" {
                        std::process::exit(1);
                    }
                }));
            }
        }
    }
    if flag("EMFOR_TEST_PLUGIN_IGNORE_EOF") {
        eprintln!("test plugin ignoring end of input");
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    // stdin closed: exit, without waiting for hanging checks.
}

fn send(out: &Mutex<std::io::Stdout>, garbage: bool, r: &Response) {
    let mut out = out.lock().unwrap();
    if garbage {
        let _ = writeln!(out, "this is not json");
        let _ = writeln!(out, r#"{{"type":"surprise","id":1}}"#);
    }
    let _ = writeln!(out, "{}", serde_json::to_string(r).unwrap());
    let _ = out.flush();
}
