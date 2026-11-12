//! The driver must catch a wrong client (spec scenario "Driver detects a
//! wrong answer"), and must report a crashing one rather than pass it.

use std::process::Command;

fn driver_with(script: &str) -> std::process::Output {
    driver_with_args(script, &[])
}

fn driver_with_args(script: &str, args: &[&str]) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    let prog = dir.path().join("lookup.sh");
    std::fs::write(&prog, script).unwrap();
    Command::new(env!("CARGO_BIN_EXE_emfor-conformance"))
        .args(args)
        .args(["--", "sh"])
        .arg(&prog)
        .output()
        .unwrap()
}

#[test]
fn wrong_answers_fail() {
    // Prints one fixed line per requested lookup.
    let out =
        driver_with("i=0; while [ $i -lt \"$3\" ]; do echo 'OK 10.9.9.9:1'; i=$((i+1)); done\n");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("FAIL "), "{stdout}");
    assert!(!stdout.contains(" 0 failures"), "{stdout}");
}

#[test]
fn crashing_program_fails() {
    let out = driver_with("exit 3\n");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("program exited with"), "{stdout}");
}

#[test]
fn hanging_program_times_out() {
    let start = std::time::Instant::now();
    let out = driver_with_args("exec sleep 60\n", &["--timeout-ms", "100"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("program timed out"), "{stdout}");
    // 51 runs of 100 ms, far below one 60 s sleep.
    assert!(start.elapsed() < std::time::Duration::from_secs(30));
}

#[test]
fn missing_program_is_a_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_emfor-conformance"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
