//! `emfor lookup` (spec lookup-command): run the `emfor` binary on stores
//! written with the test fixture.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::cargo::cargo_bin;
use serde_json::{Value, json};

/// A store with one service `api.svc.emfor`, and a config file that names it.
struct Fixture {
    _dir: tempfile::TempDir,
    config: PathBuf,
    store: PathBuf,
}

/// Backends as (id, state, reason, backup, extra fields).
type Backend<'a> = (&'a str, &'a str, Option<&'a str>, bool);

fn service(extra: Value, backends: &[Backend]) -> Value {
    let bes: Vec<Value> = backends
        .iter()
        .map(|(id, state, reason, backup)| {
            let (ip, port) = id.rsplit_once(':').unwrap();
            let ip = ip.trim_matches(|c| c == '[' || c == ']');
            json!({"id": id, "ip": ip, "port": port.parse::<u16>().unwrap(),
                "state": state, "checked_at": 1, "reason": reason, "backup": backup})
        })
        .collect();
    let mut s = json!({"name": "api.svc.emfor", "on_all_down": "open",
        "balance": "round-robin", "all_down": false, "backends": bes});
    for (k, v) in extra.as_object().unwrap() {
        s[k] = v.clone();
    }
    s
}

fn fixture(heartbeat: &str, schema: &str, svc: Value) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("state.mdb");
    let doc = json!({"schema_version": schema, "heartbeat": heartbeat, "services": [svc]});
    emfor_fixture::write_store(&doc, 3000, emfor_fixture::now_ms(), &store).unwrap();
    let config = dir.path().join("emfor.toml");
    std::fs::write(
        &config,
        format!("[store]\npath = {:?}\n", store.display().to_string()),
    )
    .unwrap();
    Fixture {
        _dir: dir,
        config,
        store,
    }
}

fn run(config: &Path, args: &[&str]) -> Output {
    Command::new(cargo_bin("emfor"))
        .arg("lookup")
        .args(args)
        .arg("--config")
        .arg(config)
        .env_remove("EMFOR_MAX_STALENESS")
        .env_remove("EMFOR_STORE")
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn three() -> [Backend<'static>; 3] {
    [
        ("10.0.0.11:8080", "unhealthy", Some("refused"), false),
        ("10.0.0.12:8080", "healthy", None, false),
        ("10.0.0.13:8080", "healthy", None, false),
    ]
}

/// Scenario "Failover returns the first healthy backend".
#[test]
fn failover_prints_the_first_healthy_backend() {
    let f = fixture(
        "fresh",
        "2",
        service(json!({"balance": "failover"}), &three()),
    );
    let o = run(&f.config, &["api.svc.emfor"]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert_eq!(stdout(&o), "10.0.0.12:8080\n");
}

/// Scenario "Sticky with a key gives the same backend": twice the same, and
/// the address the client library gives.
#[test]
fn sticky_with_a_key_matches_the_client_library() {
    let f = fixture(
        "fresh",
        "2",
        service(json!({"balance": "sticky"}), &three()),
    );
    let a = stdout(&run(&f.config, &["api.svc.emfor", "--key", "user-42"]));
    let b = stdout(&run(&f.config, &["api.svc.emfor", "--key", "user-42"]));
    assert_eq!(a, b);
    let client = emfor_client::Client::for_store(f.store.clone());
    let want = client.lookup("api.svc.emfor", Some("user-42")).unwrap();
    assert_eq!(a.trim_end(), want);
}

/// Scenario "Round-robin prints one candidate".
#[test]
fn round_robin_prints_a_candidate() {
    let f = fixture("fresh", "2", service(json!({}), &three()));
    let o = run(&f.config, &["api.svc.emfor"]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let got = stdout(&o);
    assert!(
        got == "10.0.0.12:8080\n" || got == "10.0.0.13:8080\n",
        "{got:?}"
    );
}

/// Scenario "Lookup does not write the store": the data file is the same
/// after a lookup, with and without `--explain`.
#[test]
fn lookup_does_not_write_the_store() {
    let f = fixture("fresh", "2", service(json!({}), &three()));
    let before = std::fs::read(&f.store).unwrap();
    for args in [
        vec!["api.svc.emfor"],
        vec!["api.svc.emfor", "--explain"],
        vec!["api.svc.emfor", "--json"],
    ] {
        assert_eq!(run(&f.config, &args).status.code(), Some(0));
    }
    assert_eq!(std::fs::read(&f.store).unwrap(), before);
}

/// Example "exit status table".
#[test]
fn exit_statuses() {
    // 1: a closed service with no healthy backend.
    let down = [("10.0.0.11:8080", "unhealthy", Some("refused"), false)];
    let f = fixture(
        "fresh",
        "2",
        service(json!({"on_all_down": "closed"}), &down),
    );
    let o = run(&f.config, &["api.svc.emfor"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).is_empty());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("ALL_DOWN"),
        "{o:?}"
    );
    // 2: no service of that name, an unknown option, a config that cannot be read.
    assert_eq!(run(&f.config, &["other.svc.emfor"]).status.code(), Some(2));
    assert_eq!(
        run(&f.config, &["api.svc.emfor", "--dns", "SRV"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(&f.config, &["api.svc.emfor", "--nonsense"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(Path::new("/nonexistent/emfor.toml"), &["api.svc.emfor"])
            .status
            .code(),
        Some(2)
    );
    // 3: the store does not exist; a format version this command does not read.
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(
        &cfg,
        format!(
            "[store]\npath = {:?}\n",
            dir.path().join("none.mdb").display().to_string()
        ),
    )
    .unwrap();
    assert_eq!(run(&cfg, &["api.svc.emfor"]).status.code(), Some(3));
    let f = fixture("fresh", "3", service(json!({}), &three()));
    assert_eq!(run(&f.config, &["api.svc.emfor"]).status.code(), Some(3));
}

fn four() -> [Backend<'static>; 4] {
    [
        ("10.0.0.11:8080", "healthy", None, false),
        (
            "10.0.0.12:8080",
            "healthy",
            Some("connect: Connection refused"),
            false,
        ),
        ("10.0.0.13:8080", "healthy", None, true),
        (
            "10.0.0.14:8080",
            "unhealthy",
            Some("connect: Connection refused"),
            false,
        ),
    ]
}

fn json_of(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {o:?}"))
}

/// Scenario "Primary and backup backends".
#[test]
fn explain_names_the_tier_and_the_reason_of_every_backend() {
    let f = fixture("fresh", "2", service(json!({}), &four()));
    let o = run(&f.config, &["api.svc.emfor", "--explain"]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let out = stdout(&o);
    assert!(
        out.contains("tier: healthy primary backends whose last check passed"),
        "{out}"
    );
    assert!(out.contains("candidates:\n  10.0.0.11:8080\n"), "{out}");
    assert!(out.contains("  10.0.0.12:8080  last check failed"), "{out}");
    assert!(out.contains("  10.0.0.13:8080  backup held back"), "{out}");
    assert!(out.contains("  10.0.0.14:8080  unhealthy"), "{out}");
    assert!(out.contains("answer: 10.0.0.11:8080"), "{out}");
    assert!(
        out.contains("the actual choice depends on the rotation start"),
        "{out}"
    );
}

/// Scenario "Stale store".
#[test]
fn explain_says_when_the_store_is_stale() {
    let healthy = [
        ("10.0.0.11:8080", "healthy", None, false),
        ("10.0.0.12:8080", "healthy", None, false),
    ];
    let f = fixture("stale", "2", service(json!({}), &healthy));
    let out = stdout(&run(&f.config, &["api.svc.emfor", "--explain"]));
    assert!(out.contains("stale"), "{out}");
    assert!(out.contains("tier: every backend"), "{out}");
    assert!(
        out.contains("  10.0.0.11:8080\n  10.0.0.12:8080\n"),
        "{out}"
    );
}

/// Scenario "Closed service with no healthy backend": the explanation is
/// printed, no address, exit status 1.
#[test]
fn explain_when_a_closed_service_has_no_healthy_backend() {
    let down = [
        ("10.0.0.11:8080", "unhealthy", Some("refused"), false),
        ("10.0.0.12:8080", "unhealthy", Some("refused"), false),
    ];
    let f = fixture(
        "fresh",
        "2",
        service(json!({"on_all_down": "closed"}), &down),
    );
    let o = run(&f.config, &["api.svc.emfor", "--explain"]);
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    let out = stdout(&o);
    assert!(out.contains("tier: none"), "{out}");
    assert!(out.contains("  10.0.0.11:8080  unhealthy"), "{out}");
    assert!(out.contains("  10.0.0.12:8080  unhealthy"), "{out}");
    assert!(!out.lines().any(|l| l.starts_with("answer:")), "{out}");
    assert!(out.contains("no answer: ALL_DOWN"), "{out}");
}

/// Scenario "JSON for a fixed choice".
#[test]
fn json_for_a_fixed_choice() {
    let f = fixture(
        "fresh",
        "2",
        service(json!({"balance": "failover"}), &three()),
    );
    let o = run(&f.config, &["api.svc.emfor", "--explain", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let v = json_of(&o);
    assert_eq!(v["service"], "api.svc.emfor");
    assert_eq!(v["via"], "client");
    assert_eq!(v["family"], Value::Null);
    assert_eq!(v["store"]["path"], f.store.display().to_string());
    assert_eq!(v["store"]["stale"], false);
    assert!(v["store"]["heartbeat_age_ms"].is_u64());
    assert_eq!(v["balance"], "failover");
    assert_eq!(v["on_all_down"], "open");
    assert_eq!(v["result"]["status"], "ok");
    assert_eq!(v["result"]["address"], "10.0.0.12:8080");
    assert_eq!(v["explain"]["tier"], "primary-passing");
    assert_eq!(
        v["explain"]["candidates"],
        json!([{"id": "10.0.0.12:8080"}, {"id": "10.0.0.13:8080"}])
    );
    assert_eq!(
        v["explain"]["excluded"],
        json!([{"id": "10.0.0.11:8080", "reason": "unhealthy"}])
    );
    assert_eq!(v["explain"]["selection"]["deterministic"], true);
    assert_eq!(v["explain"]["selection"]["chosen"], "10.0.0.12:8080");
}

/// Scenario "JSON for a rotating choice"; and `--json` without `--explain`
/// has no `explain` field.
#[test]
fn json_for_a_rotating_choice() {
    let f = fixture("fresh", "2", service(json!({}), &three()));
    let v = json_of(&run(&f.config, &["api.svc.emfor", "--explain", "--json"]));
    assert_eq!(v["explain"]["selection"]["deterministic"], false);
    assert_eq!(v["explain"]["selection"]["chosen"], Value::Null);
    let plain = json_of(&run(&f.config, &["api.svc.emfor", "--json"]));
    assert!(plain.get("explain").is_none(), "{plain}");
    assert_eq!(plain["result"]["status"], "ok");
}

/// A failure before the store is read is still one JSON object, with the
/// error and the exit status of the failure.
#[test]
fn json_of_a_failed_lookup() {
    let f = fixture("fresh", "2", service(json!({}), &three()));
    let o = run(&f.config, &["other.svc.emfor", "--json"]);
    assert_eq!(o.status.code(), Some(2));
    let v = json_of(&o);
    assert_eq!(v["result"]["status"], "error");
    assert_eq!(v["result"]["error"]["code"], "NOT_FOUND");
    assert!(v.get("explain").is_none());
    let down = [("10.0.0.11:8080", "unhealthy", Some("refused"), false)];
    let g = fixture(
        "fresh",
        "2",
        service(json!({"on_all_down": "closed"}), &down),
    );
    let o = run(&g.config, &["api.svc.emfor", "--explain", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let v = json_of(&o);
    assert_eq!(v["result"]["error"]["code"], "ALL_DOWN");
    assert_eq!(v["explain"]["tier"], "none");
}

fn mixed() -> [Backend<'static>; 3] {
    [
        ("10.0.0.11:8080", "healthy", None, false),
        (
            "10.0.0.12:8080",
            "unhealthy",
            Some("connect: Connection refused"),
            false,
        ),
        (
            "[2001:db8::11]:8080",
            "healthy",
            Some("connect: Connection refused"),
            false,
        ),
    ]
}

/// Scenario "A and AAAA have different tiers".
#[test]
fn dns_a_and_aaaa_have_different_tiers() {
    let f = fixture("fresh", "2", service(json!({}), &mixed()));
    let a = stdout(&run(&f.config, &["api.svc.emfor", "--dns", "A"]));
    assert!(
        a.contains("tier: healthy primary backends whose last check passed"),
        "{a}"
    );
    assert!(a.contains("candidates:\n  10.0.0.11:8080\n"), "{a}");
    assert!(a.contains("  10.0.0.12:8080  unhealthy"), "{a}");
    assert!(!a.contains("2001:db8"), "{a}");
    let aaaa = stdout(&run(&f.config, &["api.svc.emfor", "--dns", "AAAA"]));
    assert!(
        aaaa.contains("tier: healthy primary backends including those whose last check failed"),
        "{aaaa}"
    );
    assert!(
        aaaa.contains("candidates:\n  [2001:db8::11]:8080\n"),
        "{aaaa}"
    );
    assert!(!aaaa.contains("10.0.0.11"), "{aaaa}");
    // What it says about itself: the store is read, no answer is promised.
    assert!(
        aaaa.contains("answering: as the DNS server answers an AAAA query"),
        "{aaaa}"
    );
    assert!(aaaa.contains("up to about one second"), "{aaaa}");
    assert!(!aaaa.lines().any(|l| l.starts_with("answer:")), "{aaaa}");
}

/// Scenario "No backend of the family".
#[test]
fn dns_with_no_backend_of_the_family() {
    let v4 = [("10.0.0.11:8080", "healthy", None, false)];
    let f = fixture("fresh", "2", service(json!({}), &v4));
    let o = run(&f.config, &["api.svc.emfor", "--dns", "AAAA"]);
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    let out = stdout(&o);
    assert!(out.contains("no AAAA backend"), "{out}");
    assert!(!out.lines().any(|l| l.starts_with("answer:")), "{out}");
    let j = json_of(&run(
        &f.config,
        &["api.svc.emfor", "--dns", "AAAA", "--json"],
    ));
    assert_eq!(j["via"], "dns");
    assert_eq!(j["family"], "AAAA");
    assert_eq!(j["result"]["status"], "error");
}

/// The DNS server counts a store as fresh whatever its heartbeat; `failover`
/// has a fixed answer there too; a service name is normalized.
#[test]
fn dns_uses_the_tiers_of_a_fresh_store_and_failover_is_fixed() {
    let f = fixture(
        "stale",
        "2",
        service(json!({"balance": "failover"}), &three()),
    );
    let o = run(&f.config, &["API.svc.emfor.", "--dns", "a", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let v = json_of(&o);
    assert_eq!(v["explain"]["tier"], "primary-passing");
    assert_eq!(v["result"]["address"], "10.0.0.12:8080");
    assert_eq!(v["explain"]["selection"]["deterministic"], true);
    assert_eq!(v["store"]["stale"], true);
    let text = stdout(&run(&f.config, &["api.svc.emfor", "--dns", "A"]));
    assert!(text.contains("does not look like it is running"), "{text}");
}

/// DNS: `weighted` is explained as the daemon chooses; unknown services and
/// unusable stores fail as the client mode does.
#[test]
fn dns_weighted_rule_and_failures() {
    let two = [
        ("10.0.0.11:8080", "healthy", None, false),
        ("10.0.0.12:8080", "healthy", None, false),
    ];
    let f = fixture("fresh", "2", service(json!({"balance": "weighted"}), &two));
    let out = stdout(&run(&f.config, &["api.svc.emfor", "--dns", "A"]));
    assert!(out.contains("smooth weighted round-robin"), "{out}");
    assert_eq!(
        run(&f.config, &["other.svc.emfor", "--dns", "A"])
            .status
            .code(),
        Some(2)
    );
    let g = fixture("fresh", "3", service(json!({}), &two));
    assert_eq!(
        run(&g.config, &["api.svc.emfor", "--dns", "A"])
            .status
            .code(),
        Some(3)
    );
}
