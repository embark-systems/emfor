//! `PluginHost` against the `emfor-test-plugin` binary.

use std::time::{Duration, Instant};

use emfor_daemon::plugin::host::{CallError, PluginHost};
use emfor_daemon::plugin::protocol::{Request, Response, Target};

const PLUGIN: &str = env!("CARGO_BIN_EXE_emfor-test-plugin");

/// Command running the test plugin with `env` settings.
fn command(env: &[&str]) -> Vec<String> {
    let mut c = vec!["/usr/bin/env".to_string()];
    c.extend(env.iter().map(|s| s.to_string()));
    c.push(PLUGIN.to_string());
    c
}

fn check(mode: &str) -> impl FnOnce(u64) -> Request {
    let mode = mode.to_string();
    move |id| Request::Check {
        id,
        service: "cache.svc.emfor".into(),
        target: Target {
            ip: "10.0.2.1".into(),
            port: 6379,
        },
        options: serde_json::json!({ "mode": mode })
            .as_object()
            .unwrap()
            .clone(),
        timeout_ms: 900,
    }
}

fn result_of(r: Response) -> (u64, String, Option<String>) {
    match r {
        Response::Check { id, result, reason } => {
            (id, result.as_str().unwrap_or("").to_string(), reason)
        }
        other => panic!("expected check answer, got {other:?}"),
    }
}

fn alive(pid: u32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
}

#[tokio::test]
async fn handshake_check_and_stop() {
    let host = PluginHost::start("fake", &command(&[])).await.unwrap();
    let pid = host.pid().unwrap();
    assert!(alive(pid));
    let (_, result, reason) = result_of(host.call(check("down")).await.unwrap());
    assert_eq!(result, "down");
    assert_eq!(reason.as_deref(), Some("test plugin says down"));
    host.stop().await;
    assert_eq!(host.pid(), None);
    assert!(!alive(pid), "plugin {pid} still running after stop");
    assert_eq!(
        host.call(check("up")).await.unwrap_err(),
        CallError::Unavailable
    );
}

#[tokio::test]
async fn wrong_protocol_fails_start() {
    let err = PluginHost::start("fake", &command(&["EMFOR_TEST_PLUGIN_PROTOCOL=2"]))
        .await
        .unwrap_err();
    assert!(err.contains("fake"), "{err}");
    assert!(
        err.contains("protocol 2") && err.contains("protocol 1"),
        "{err}"
    );
}

#[tokio::test]
async fn exit_before_hello_fails_start() {
    let err = PluginHost::start("fake", &command(&["EMFOR_TEST_PLUGIN_EXIT_AT_START=1"]))
        .await
        .unwrap_err();
    assert!(
        err.contains("plugin fake exited before the handshake"),
        "{err}"
    );
}

#[tokio::test]
async fn missing_program_fails_start() {
    let err = PluginHost::start("fake", &["/nonexistent/plugin".to_string()])
        .await
        .unwrap_err();
    assert!(err.contains("cannot run /nonexistent/plugin"), "{err}");
}

#[tokio::test]
async fn no_hello_fails_start_after_timeout() {
    let t = Instant::now();
    let err = PluginHost::start("fake", &command(&["EMFOR_TEST_PLUGIN_NO_HELLO=1"]))
        .await
        .unwrap_err();
    assert!(
        err.contains("did not answer the handshake within 5s"),
        "{err}"
    );
    assert!(t.elapsed() >= Duration::from_secs(5));
}

#[tokio::test]
async fn answers_out_of_order_reach_their_requests() {
    let host = std::sync::Arc::new(PluginHost::start("fake", &command(&[])).await.unwrap());
    let h = host.clone();
    let slow = tokio::spawn(async move {
        let r = h.call(check("slow_first")).await.unwrap();
        (r, Instant::now())
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let fast = host.call(check("down")).await.unwrap();
    let fast_at = Instant::now();
    let (slow, slow_at) = slow.await.unwrap();
    assert!(fast_at < slow_at, "second request should be answered first");
    let (slow_id, slow_result, _) = result_of(slow);
    let (fast_id, fast_result, _) = result_of(fast);
    assert!(slow_id < fast_id);
    assert_eq!(slow_result, "up");
    assert_eq!(fast_result, "down");
    host.stop().await;
}

#[tokio::test]
async fn garbage_lines_are_ignored() {
    let host = PluginHost::start("fake", &command(&["EMFOR_TEST_PLUGIN_GARBAGE=1"]))
        .await
        .unwrap();
    for _ in 0..3 {
        let (_, result, _) = result_of(host.call(check("up")).await.unwrap());
        assert_eq!(result, "up");
    }
    host.stop().await;
}

#[tokio::test]
async fn crash_fails_pending_then_restarts() {
    let host = std::sync::Arc::new(PluginHost::start("fake", &command(&[])).await.unwrap());
    let first_pid = host.pid().unwrap();
    // A hanging request is pending when the plugin crashes.
    let h = host.clone();
    let pending = tokio::spawn(async move { h.call(check("hang")).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        host.call(check("crash")).await.unwrap_err(),
        CallError::Exited
    );
    assert_eq!(pending.await.unwrap().unwrap_err(), CallError::Exited);
    // Down until restarted: fails at once.
    let t = Instant::now();
    assert_eq!(
        host.call(check("up")).await.unwrap_err(),
        CallError::Unavailable
    );
    assert!(t.elapsed() < Duration::from_millis(100));
    // Restarted after the 1s backoff.
    let deadline = Instant::now() + Duration::from_secs(5);
    let new_pid = loop {
        if let Some(p) = host.pid() {
            break p;
        }
        assert!(Instant::now() < deadline, "plugin not restarted");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_ne!(new_pid, first_pid);
    let (_, result, _) = result_of(host.call(check("up")).await.unwrap());
    assert_eq!(result, "up");
    host.stop().await;
    assert!(!alive(new_pid));
}

#[tokio::test]
async fn answer_written_just_before_exit_is_delivered() {
    // The plugin answers `down` and exits at once; the answer must win over
    // "plugin exited".
    let host = PluginHost::start("fake", &command(&[])).await.unwrap();
    let (_, result, reason) = result_of(host.call(check("last_words")).await.unwrap());
    assert_eq!(result, "down");
    assert_eq!(reason.as_deref(), Some("test plugin says down"));
    host.stop().await;
}

#[tokio::test]
async fn stop_kills_a_plugin_that_ignores_end_of_input() {
    let host = PluginHost::start("fake", &command(&["EMFOR_TEST_PLUGIN_IGNORE_EOF=1"]))
        .await
        .unwrap();
    let pid = host.pid().unwrap();
    let t = Instant::now();
    host.stop().await;
    let took = t.elapsed();
    assert!(
        took >= Duration::from_millis(1900) && took < Duration::from_secs(4),
        "stop took {took:?}"
    );
    assert!(!alive(pid), "plugin {pid} still running");
}

#[tokio::test]
async fn dropped_request_is_forgotten() {
    let host = PluginHost::start("fake", &command(&[])).await.unwrap();
    let r = tokio::time::timeout(Duration::from_millis(100), host.call(check("hang"))).await;
    assert!(r.is_err());
    // Later requests still work.
    let (_, result, _) = result_of(host.call(check("up")).await.unwrap());
    assert_eq!(result, "up");
    host.stop().await;
}

#[tokio::test]
async fn plugin_that_stops_answering_is_restarted() {
    let host = PluginHost::start("fake", &command(&[])).await.unwrap();
    let first_pid = host.pid().unwrap();
    // Callers give up on three requests in a row, with no answer between.
    for _ in 0..3 {
        let r = tokio::time::timeout(Duration::from_millis(100), host.call(check("hang"))).await;
        assert!(r.is_err());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let new_pid = loop {
        if let Some(p) = host.pid().filter(|&p| p != first_pid) {
            break p;
        }
        assert!(Instant::now() < deadline, "stuck plugin not restarted");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(!alive(first_pid), "stuck plugin {first_pid} still running");
    let (_, result, _) = result_of(host.call(check("up")).await.unwrap());
    assert_eq!(result, "up");
    host.stop().await;
    assert!(!alive(new_pid));
}

#[tokio::test]
async fn answers_between_unanswered_requests_keep_the_plugin() {
    let host = PluginHost::start("fake", &command(&[])).await.unwrap();
    let pid = host.pid().unwrap();
    for _ in 0..4 {
        for _ in 0..2 {
            let r =
                tokio::time::timeout(Duration::from_millis(100), host.call(check("hang"))).await;
            assert!(r.is_err());
        }
        let (_, result, _) = result_of(host.call(check("up")).await.unwrap());
        assert_eq!(result, "up");
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(host.pid(), Some(pid), "plugin was restarted");
    host.stop().await;
}

// PluginFactory / PluginChecker through `run_check`.

use std::sync::Arc;

use emfor_daemon::checker::{CheckRequest, CheckResult, CheckerFactory, run_check};
use emfor_daemon::plugin::PluginFactory;

async fn factory(env: &[&str]) -> (Arc<PluginHost>, Arc<PluginFactory>) {
    let host = Arc::new(PluginHost::start("fake", &command(env)).await.unwrap());
    let f = Arc::new(PluginFactory::new(
        host.clone(),
        tokio::runtime::Handle::current(),
    ));
    (host, f)
}

async fn validate(
    f: &Arc<PluginFactory>,
    options: &str,
) -> Result<Arc<dyn emfor_daemon::checker::Checker>, Vec<emfor_daemon::checker::OptionError>> {
    let f = f.clone();
    let options: toml::Table = options.parse().unwrap();
    tokio::task::spawn_blocking(move || f.validate(&options))
        .await
        .unwrap()
}

fn req(timeout: Duration) -> CheckRequest {
    CheckRequest::new("cache.svc.emfor", "10.0.2.1:6379".parse().unwrap(), timeout)
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_checker_results() {
    let (host, f) = factory(&[]).await;
    let rows = [
        ("up", CheckResult::Up),
        ("down", CheckResult::Down("test plugin says down".into())),
        ("error", CheckResult::Error("test plugin says error".into())),
        (
            "bogus",
            CheckResult::Error("invalid plugin response".into()),
        ),
        // Not a string: reported at once, not after the timeout.
        (
            "numeric",
            CheckResult::Error("invalid plugin response".into()),
        ),
        (
            "hang",
            CheckResult::Error("plugin did not answer within timeout".into()),
        ),
    ];
    for (mode, want) in rows {
        let c = validate(&f, &format!("mode = \"{mode}\"")).await.unwrap();
        let t = Instant::now();
        let got = run_check(c.as_ref(), req(Duration::from_millis(500))).await;
        assert_eq!(got, want, "mode {mode}");
        if mode != "hang" {
            assert!(
                t.elapsed() < Duration::from_millis(400),
                "mode {mode} waited for the timeout"
            );
        }
    }
    host.stop().await;
    let c = validate(&f, "").await;
    assert!(c.is_err(), "validate after stop must fail");
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_checker_unavailable_after_crash() {
    let (host, f) = factory(&[]).await;
    let crash = validate(&f, "mode = \"crash\"").await.unwrap();
    let up = validate(&f, "mode = \"up\"").await.unwrap();
    assert_eq!(
        run_check(crash.as_ref(), req(Duration::from_secs(1))).await,
        CheckResult::Error("plugin exited".into())
    );
    assert_eq!(
        run_check(up.as_ref(), req(Duration::from_secs(1))).await,
        CheckResult::Error("plugin fake unavailable".into())
    );
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_validation_errors_and_checker_id() {
    let (host, f) = factory(&[]).await;
    let errs = validate(&f, "mode = \"nope\"\npassword_file = \"/x\"")
        .await
        .unwrap_err();
    let mut keys: Vec<_> = errs.iter().map(|e| e.key.as_str()).collect();
    keys.sort();
    assert_eq!(keys, ["mode", "password_file"]);
    assert!(f.checker_id().starts_with("plugin:fake:[\"/usr/bin/env\""));
    assert_eq!(f.kind(), "fake");
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn validation_timeout_is_sticky() {
    let (host, f) = factory(&["EMFOR_TEST_PLUGIN_VALIDATE_HANG=1"]).await;
    let t = Instant::now();
    let first = validate(&f, "").await.unwrap_err();
    assert!(t.elapsed() >= Duration::from_secs(5));
    assert_eq!(first[0].key, "");
    assert_eq!(
        first[0].message,
        "plugin fake did not answer validation within 5s"
    );
    let t = Instant::now();
    let second = validate(&f, "").await.unwrap_err();
    assert!(t.elapsed() < Duration::from_millis(100));
    assert_eq!(second, first);
    host.stop().await;
}

// Processes a plugin starts are stopped with it.

/// Command for a plugin that first starts `sleep 600` in the background,
/// writing its pid to `pidfile`, then runs the test plugin.
fn command_with_child(pidfile: &std::path::Path) -> Vec<String> {
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!(
            "sleep 600 </dev/null >/dev/null 2>&1 & echo $! > '{}'; exec '{PLUGIN}'",
            pidfile.display()
        ),
    ]
}

fn read_pid(pidfile: &std::path::Path) -> i32 {
    std::fs::read_to_string(pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// Whether `pid` is running. An orphan nobody reaps (in a container whose
/// init does not reap) stays a zombie, which counts as gone.
#[cfg(target_os = "linux")]
fn running(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !stat
            .rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')),
        Err(_) => false,
    }
}

#[cfg(not(target_os = "linux"))]
fn running(pid: i32) -> bool {
    alive(pid as u32)
}

async fn wait_gone(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while running(pid) {
        assert!(
            Instant::now() < deadline,
            "child {pid} of the plugin still running"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn stop_kills_what_the_plugin_started() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("child.pid");
    let host = PluginHost::start("fake", &command_with_child(&pidfile))
        .await
        .unwrap();
    let child = read_pid(&pidfile);
    assert!(running(child));
    host.stop().await;
    wait_gone(child).await;
}

#[tokio::test]
async fn restart_after_stall_kills_what_the_plugin_started() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("child.pid");
    let host = PluginHost::start("fake", &command_with_child(&pidfile))
        .await
        .unwrap();
    let first_pid = host.pid().unwrap();
    let child = read_pid(&pidfile);
    for _ in 0..3 {
        let r = tokio::time::timeout(Duration::from_millis(100), host.call(check("hang"))).await;
        assert!(r.is_err());
    }
    wait_gone(child).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.pid().is_none_or(|p| p == first_pid) {
        assert!(Instant::now() < deadline, "stuck plugin not restarted");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let second = read_pid(&pidfile);
    assert_ne!(second, child);
    host.stop().await;
    wait_gone(second).await;
}

#[tokio::test]
async fn crash_kills_what_the_plugin_started() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("child.pid");
    let host = PluginHost::start("fake", &command_with_child(&pidfile))
        .await
        .unwrap();
    let child = read_pid(&pidfile);
    assert_eq!(
        host.call(check("crash")).await.unwrap_err(),
        CallError::Exited
    );
    wait_gone(child).await;
    host.stop().await;
}
