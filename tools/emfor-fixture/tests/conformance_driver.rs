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

/// Spec scenario "Driver detects wrong counts": 3:1 expected, 2:2 printed.
#[test]
fn wrong_counts_fail() {
    let dir = tempfile::tempdir().unwrap();
    let cases = dir.path().join("cases.json");
    let backend = |id: &str| {
        let (ip, port) = id.split_once(':').unwrap();
        serde_json::json!({"id": id, "ip": ip, "port": port.parse::<u16>().unwrap(),
                           "weight": 1, "site": null, "state": "healthy", "checked_at": 1, "reason": null})
    };
    let case = serde_json::json!([{
        "case": "counts",
        "store": {"schema_version": "1", "heartbeat": "fresh", "raw": {}, "services": [{
            "name": "api.svc.emfor", "on_all_down": "open", "balance": "weighted", "all_down": false,
            "backends": [backend("10.0.0.11:8080"), backend("10.0.0.12:8080")]}]},
        "max_staleness_ms": 3000,
        "lookups": [{"name": "api.svc.emfor",
                     "expect_counts": {"10.0.0.11:8080": 3, "10.0.0.12:8080": 1}}]
    }]);
    std::fs::write(&cases, case.to_string()).unwrap();
    let vectors = dir.path().join("sticky.json");
    std::fs::write(&vectors, r#"{"hashes": [], "picks": []}"#).unwrap();
    let script = "echo 'OK 10.0.0.11:8080'; echo 'OK 10.0.0.11:8080'; echo 'OK 10.0.0.12:8080'; echo 'OK 10.0.0.12:8080'\n";
    let out = driver_with_args(
        script,
        &[
            "--cases",
            cases.to_str().unwrap(),
            "--vectors",
            vectors.to_str().unwrap(),
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("want counts"), "{stdout}");
    assert!(stdout.contains("1 lookups, 1 failures"), "{stdout}");
}

/// A malformed `expect_counts` is a setup error (exit 2), not a silent 0.
#[test]
fn bad_counts_are_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let cases = dir.path().join("cases.json");
    let case = serde_json::json!([{
        "case": "bad counts", "store": null, "max_staleness_ms": 3000,
        "lookups": [{"name": "api.svc.emfor", "expect_counts": {"10.0.0.11:8080": -1}}]
    }]);
    std::fs::write(&cases, case.to_string()).unwrap();
    let out = driver_with_args("exit 0\n", &["--cases", cases.to_str().unwrap()]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("expect_counts must map"));
}

/// Run the driver with one case and no pick vectors.
fn driver_with_case(script: &str, case: serde_json::Value) -> String {
    let dir = tempfile::tempdir().unwrap();
    let cases = dir.path().join("cases.json");
    std::fs::write(&cases, serde_json::json!([case]).to_string()).unwrap();
    let vectors = dir.path().join("sticky.json");
    std::fs::write(&vectors, r#"{"hashes": [], "picks": []}"#).unwrap();
    let out = driver_with_args(
        script,
        &[
            "--cases",
            cases.to_str().unwrap(),
            "--vectors",
            vectors.to_str().unwrap(),
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        matches!(out.status.code(), Some(0 | 1)),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// The right code with no message fails: every client must say why.
#[test]
fn error_without_message_fails() {
    let case = serde_json::json!({
        "case": "missing store", "store": null, "max_staleness_ms": 3000,
        "lookups": [{"name": "api.svc.emfor", "expect_error": "UNAVAILABLE"}]
    });
    let stdout = driver_with_case("echo 'ERR UNAVAILABLE'\n", case.clone());
    assert!(stdout.contains("1 lookups, 1 failures"), "{stdout}");
    assert!(stdout.contains("message"), "{stdout}");
    let stdout = driver_with_case("echo 'ERR UNAVAILABLE no store'\n", case);
    assert!(stdout.contains("1 lookups, 0 failures"), "{stdout}");
}

/// Each lookup reads a store whose fresh heartbeat was written just before
/// it, so slow lookups earlier in a case cannot make it stale. The program
/// reads the heartbeat from the LMDB file (key and value are stored next to
/// each other) and answers like a client would.
#[test]
fn each_lookup_gets_a_fresh_store() {
    let script = r#"sleep 0.2
hb=$(grep -ao 'meta/heartbeat[0-9]*' "$EMFOR_STORE" | head -1 | sed 's/meta.heartbeat//')
now=$(date +%s%3N)
if [ $((now - hb)) -lt 500 ]; then echo 'OK 10.0.0.11:8080'; else echo "ERR ALL_DOWN heartbeat $((now - hb)) ms old"; fi
"#;
    let backend = serde_json::json!({"id": "10.0.0.11:8080", "ip": "10.0.0.11", "port": 8080,
        "state": "healthy", "checked_at": 1, "reason": null});
    let lookup = serde_json::json!({"name": "api.svc.emfor", "expect": "10.0.0.11:8080"});
    let case = serde_json::json!({
        "case": "slow program",
        "store": {"schema_version": "1", "heartbeat": "fresh", "raw": {}, "services": [{
            "name": "api.svc.emfor", "on_all_down": "closed", "balance": "failover",
            "all_down": false, "backends": [backend]}]},
        "max_staleness_ms": 500,
        "lookups": [lookup.clone(), lookup.clone(), lookup.clone(), lookup]
    });
    let stdout = driver_with_case(script, case);
    assert!(stdout.contains("4 lookups, 0 failures"), "{stdout}");
}
