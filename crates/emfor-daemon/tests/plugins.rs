//! End-to-end tests of plugins: run the `emfor` binary with `[plugins]`
//! configs that use the `emfor-test-plugin` binary.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use emfor_format::{BackendRecord, HealthState, ServiceRecord};
use heed::types::Str;
use heed::{Database, Env, EnvFlags, EnvOpenOptions, WithoutTls};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

const PLUGIN: &str = env!("CARGO_BIN_EXE_emfor-test-plugin");

/// A marker unique to one test, passed as an (ignored) plugin argument so
/// `pgrep -f` finds exactly that test's plugin processes.
fn marker(tag: &str) -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        "emfor-mark-{}-{}-{tag}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )
}

/// `[plugins."<name>"]` running the test plugin with `env` settings.
fn plugin_decl(name: &str, env: &[&str], mark: &str) -> String {
    let mut argv = vec!["/usr/bin/env".to_string()];
    argv.extend(env.iter().map(|s| s.to_string()));
    argv.push(PLUGIN.into());
    argv.push(mark.into());
    let quoted: Vec<String> = argv.iter().map(|a| format!("\"{a}\"")).collect();
    format!(
        "[plugins.\"{name}\"]\ncommand = [{}]\n\n",
        quoted.join(", ")
    )
}

/// A service checked by plugin `plugin` in test-plugin mode `mode`.
fn plugin_service(svc: &str, plugin: &str, mode: &str, settings: &str) -> String {
    format!(
        "[services.\"{svc}\"]\ncheck = \"{plugin}\"\nbackends = [\"10.0.2.1:6379\"]\njitter = \"0%\"\n{settings}\n[services.\"{svc}\".check_options]\nmode = \"{mode}\"\n\n"
    )
}

const FAST: &str = "interval = \"200ms\"\ntimeout = \"100ms\"\nrise = 2\nfall = 2\n";

fn store_header(dir: &Path) -> String {
    format!(
        "[store]\npath = \"{}\"\nmap_size = \"4MiB\"\n\n",
        dir.join("state.mdb").display()
    )
}

/// Process ids of plugins started with `mark`.
fn pgrep(mark: &str) -> Vec<u32> {
    let out = Command::new("pgrep").args(["-f", mark]).output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect()
}

fn alive(pid: u32) -> bool {
    kill(Pid::from_raw(pid as i32), None).is_ok()
}

fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("not within 15s: {what}");
}

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
    log: Arc<Mutex<String>>,
}

impl Daemon {
    fn start(body: &str) -> Daemon {
        let d = Daemon::spawn(body);
        wait_for("store created", || {
            std::fs::metadata(d.store()).is_ok_and(|m| m.len() > 0)
        });
        d
    }

    /// Start the daemon without waiting for it to finish starting.
    fn spawn(body: &str) -> Daemon {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("emfor.toml");
        std::fs::write(&cfg, format!("{}{body}", store_header(dir.path()))).unwrap();
        let mut child = Command::new(cargo_bin("emfor"))
            .arg("server")
            .arg("--config")
            .arg(&cfg)
            .env_remove("RUST_LOG")
            .env("NO_COLOR", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let log = Arc::new(Mutex::new(String::new()));
        let mut stderr = child.stderr.take().unwrap();
        let sink = log.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
        Daemon { child, dir, log }
    }

    fn store(&self) -> PathBuf {
        self.dir.path().join("state.mdb")
    }

    fn log(&self) -> String {
        self.log.lock().unwrap().clone()
    }

    fn rewrite(&self, body: &str) {
        let cfg = self.dir.path().join("emfor.toml");
        std::fs::write(cfg, format!("{}{body}", store_header(self.dir.path()))).unwrap();
    }

    /// Rewrite the config, send SIGHUP, and wait for `outcome` in the log.
    fn reload(&self, body: &str, outcome: &str) {
        let before = self.log().matches(outcome).count();
        self.rewrite(body);
        kill(Pid::from_raw(self.child.id() as i32), Signal::SIGHUP).unwrap();
        wait_for(outcome, || self.log().matches(outcome).count() > before);
    }

    /// Pids from the `plugin <name> started (pid P, ...)` log lines, in order.
    fn started_pids(&self, name: &str) -> Vec<u32> {
        let needle = format!("plugin {name} started (pid ");
        self.log()
            .match_indices(&needle)
            .filter_map(|(i, _)| {
                let rest = &self.log()[i + needle.len()..];
                rest.split(',').next()?.parse().ok()
            })
            .collect()
    }

    fn stop(mut self) -> (i32, String) {
        kill(Pid::from_raw(self.child.id() as i32), Signal::SIGTERM).unwrap();
        let status = self.child.wait().unwrap();
        std::thread::sleep(Duration::from_millis(100)); // let the log thread finish
        (status.code().unwrap_or(-1), self.log())
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

struct Reader {
    env: Env<WithoutTls>,
    db: Database<Str, Str>,
}

impl Reader {
    fn open(path: &Path) -> Reader {
        let env = unsafe {
            let mut o = EnvOpenOptions::new().read_txn_without_tls();
            o.flags(EnvFlags::NO_SUB_DIR | EnvFlags::READ_ONLY);
            o.open(path).unwrap()
        };
        let rtxn = env.read_txn().unwrap();
        let db = env.open_database(&rtxn, None).unwrap().unwrap();
        rtxn.commit().unwrap();
        Reader { env, db }
    }

    fn service(&self, name: &str) -> Option<ServiceRecord> {
        let rtxn = self.env.read_txn().unwrap();
        self.db
            .get(&rtxn, &format!("svc/{name}"))
            .unwrap()
            .map(|j| serde_json::from_str(j).unwrap())
    }

    fn backend(&self, name: &str) -> Option<BackendRecord> {
        self.service(name).map(|s| s.backends[0].clone())
    }
}

fn check_config(body: &str) -> (i32, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    std::fs::write(&cfg, body).unwrap();
    let out = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--config"])
        .arg(&cfg)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

const SVC: &str = "cache.svc.emfor";

#[test]
fn plugin_service_becomes_healthy_and_stderr_is_logged() {
    let mark = marker("healthy");
    let d = Daemon::start(&format!(
        "{}{}",
        plugin_decl("fake", &[], &mark),
        plugin_service(SVC, "fake", "up", FAST)
    ));
    let r = Reader::open(&d.store());
    wait_for("backend healthy", || {
        r.backend(SVC)
            .is_some_and(|b| b.state == HealthState::Healthy)
    });
    let pids = d.started_pids("fake");
    assert_eq!(pids.len(), 1, "{}", d.log());
    assert_eq!(pgrep(&mark), pids);
    let (code, log) = d.stop();
    assert_eq!(code, 0);
    // Spec scenario "Plugin stderr is logged".
    assert!(
        log.lines()
            .any(|l| l.contains("INFO") && l.contains("plugin fake: test plugin started")),
        "{log}"
    );
    wait_for("plugin stopped on shutdown", || pgrep(&mark).is_empty());
}

#[test]
fn check_config_with_plugin() {
    let mark = marker("checkok");
    let (code, out, err) = check_config(&format!(
        "{}{}",
        plugin_decl("fake", &[], &mark),
        plugin_service(SVC, "fake", "up", "")
    ));
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "config ok: 1 services, 1 backends");
    assert!(pgrep(&mark).is_empty(), "plugin left running");
}

#[test]
fn check_config_plugin_rejects_option() {
    let mark = marker("reject");
    let (code, _, err) = check_config(&format!(
        "{}{}",
        plugin_decl("fake", &[], &mark),
        plugin_service(SVC, "fake", "nope", "")
    ));
    assert_eq!(code, 2);
    assert!(
        err.contains("services.\"cache.svc.emfor\".check_options.mode: unknown mode \"nope\""),
        "{err}"
    );
    assert!(pgrep(&mark).is_empty(), "plugin left running");
}

#[test]
fn check_config_with_broken_plugin() {
    let mark = marker("broken");
    let (code, _, err) = check_config(&format!(
        "{}{}",
        plugin_decl("fake", &["EMFOR_TEST_PLUGIN_EXIT_AT_START=1"], &mark),
        plugin_service(SVC, "fake", "up", "")
    ));
    assert_eq!(code, 2);
    assert!(
        err.contains("plugins.fake.command: plugin fake exited before the handshake"),
        "{err}"
    );
    // The service using it gets no second error.
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(pgrep(&mark).is_empty(), "plugin left running");
}

#[test]
fn broken_plugin_at_startup_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    let mark = marker("startup");
    std::fs::write(
        &cfg,
        format!(
            "{}{}{}",
            store_header(dir.path()),
            plugin_decl("fake", &["EMFOR_TEST_PLUGIN_PROTOCOL=2"], &mark),
            plugin_service(SVC, "fake", "up", "")
        ),
    )
    .unwrap();
    let out = Command::new(cargo_bin("emfor"))
        .arg("server")
        .arg("--config")
        .arg(&cfg)
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(
            "plugins.fake.command: plugin fake speaks protocol 2, emfor speaks protocol 1"
        ),
        "{err}"
    );
    assert!(!dir.path().join("state.mdb").exists());
    assert!(pgrep(&mark).is_empty(), "plugin left running");
}

#[test]
fn unchanged_plugin_survives_reload() {
    let mark = marker("keep");
    let decl = plugin_decl("fake", &[], &mark);
    let d = Daemon::start(&format!(
        "{decl}{}",
        plugin_service(SVC, "fake", "up", FAST)
    ));
    let r = Reader::open(&d.store());
    wait_for("backend healthy", || {
        r.backend(SVC)
            .is_some_and(|b| b.state == HealthState::Healthy)
    });
    let pid = d.started_pids("fake")[0];
    let slower = FAST.replace("200ms", "300ms");
    d.reload(
        &format!("{decl}{}", plugin_service(SVC, "fake", "up", &slower)),
        "config reloaded",
    );
    assert_eq!(d.started_pids("fake"), vec![pid]);
    assert!(alive(pid));
    // The interval change kept the backend state.
    assert_eq!(r.backend(SVC).unwrap().state, HealthState::Healthy);
}

#[test]
fn rejected_reload_with_new_plugin() {
    let mark = marker("prev");
    let new_mark = marker("mysql");
    let decl = plugin_decl("fake", &[], &mark);
    let d = Daemon::start(&format!(
        "{decl}{}",
        plugin_service(SVC, "fake", "up", FAST)
    ));
    let r = Reader::open(&d.store());
    wait_for("backend healthy", || {
        r.backend(SVC)
            .is_some_and(|b| b.state == HealthState::Healthy)
    });
    let pid = d.started_pids("fake")[0];
    let bad = format!(
        "{decl}{}{}",
        plugin_decl("mysql", &[], &new_mark),
        plugin_service(SVC, "fake", "up", &FAST.replace("100ms", "5s"))
    );
    d.reload(&bad, "reload rejected");
    assert_eq!(d.started_pids("mysql").len(), 1, "{}", d.log());
    assert!(pgrep(&new_mark).is_empty(), "new plugin left running");
    assert!(alive(pid));
    assert!(d.log().contains("timeout must not exceed interval"));
    // Previous checks continue.
    let before = r.backend(SVC).unwrap().checked_at;
    wait_for("checks continue", || {
        r.backend(SVC).unwrap().checked_at > before
    });
    assert_eq!(r.backend(SVC).unwrap().state, HealthState::Healthy);
}

#[test]
fn command_change_restarts_plugin_and_resets_backends() {
    let mark = marker("cmd");
    let slow_rise = "interval = \"200ms\"\ntimeout = \"100ms\"\nrise = 2\nfall = 2\n";
    let d = Daemon::start(&format!(
        "{}{}",
        plugin_decl("fake", &[], &mark),
        plugin_service(SVC, "fake", "up", slow_rise)
    ));
    let r = Reader::open(&d.store());
    wait_for("backend healthy", || {
        r.backend(SVC)
            .is_some_and(|b| b.state == HealthState::Healthy)
    });
    let old = d.started_pids("fake")[0];
    // Same plugin name, new command; rise 20 keeps the reset visible.
    d.reload(
        &format!(
            "{}{}",
            plugin_decl("fake", &["EXTRA=1"], &mark),
            plugin_service(
                SVC,
                "fake",
                "up",
                &slow_rise.replace("rise = 2", "rise = 20")
            )
        ),
        "config reloaded",
    );
    assert_eq!(r.backend(SVC).unwrap().state, HealthState::Unknown);
    let pids = d.started_pids("fake");
    assert_eq!(pids.len(), 2, "{}", d.log());
    assert_ne!(pids[1], old);
    wait_for("old plugin stopped", || !alive(old));
    assert_eq!(pgrep(&mark), vec![pids[1]]);
}

#[test]
fn crash_and_recovery() {
    let mark = marker("crash");
    // While the block file exists the plugin exits at start, so the outage
    // lasts past the first restart (1s) until the second (2s later). That
    // outlasts the store's once-per-second commit of reason changes, so the
    // `error` results are sure to reach the store. fall × interval = 8s
    // still outlasts the outage.
    let block = std::env::temp_dir().join(format!("{mark}.block"));
    let block_env = format!("EMFOR_TEST_PLUGIN_BLOCK_FILE={}", block.display());
    let settings = "interval = \"200ms\"\ntimeout = \"100ms\"\nrise = 2\nfall = 40\n";
    let d = Daemon::start(&format!(
        "{}{}",
        plugin_decl("fake", &[&block_env], &mark),
        plugin_service(SVC, "fake", "up", settings)
    ));
    let r = Reader::open(&d.store());
    wait_for("backend healthy", || {
        r.backend(SVC)
            .is_some_and(|b| b.state == HealthState::Healthy)
    });
    let pid = d.started_pids("fake")[0];
    std::fs::write(&block, "").unwrap();
    kill(Pid::from_raw(pid as i32), Signal::SIGKILL).unwrap();
    wait_for("error result during the outage", || {
        let b = r.backend(SVC).unwrap();
        assert_eq!(b.state, HealthState::Healthy, "backend left healthy: {b:?}");
        b.reason.as_deref().is_some_and(|r| r.starts_with("plugin"))
    });
    std::fs::remove_file(&block).unwrap();
    wait_for("plugin restarted", || {
        let b = r.backend(SVC).unwrap();
        assert_eq!(b.state, HealthState::Healthy, "backend left healthy: {b:?}");
        d.started_pids("fake").len() == 2
    });
    assert_eq!(d.log().matches("test plugin started").count(), 2);
    assert!(d.log().contains("plugin fake exited"), "{}", d.log());
    // Results return after the restart.
    wait_for("plugin results again", || {
        r.backend(SVC).unwrap().reason.is_none()
    });
    assert_eq!(r.backend(SVC).unwrap().state, HealthState::Healthy);
}

#[test]
fn plugin_timeout_is_not_a_backend_failure() {
    let mark = marker("hang");
    let d = Daemon::start(&format!(
        "{}{}",
        plugin_decl("fake", &[], &mark),
        plugin_service(SVC, "fake", "hang", FAST)
    ));
    let r = Reader::open(&d.store());
    wait_for("timeout reason", || {
        r.backend(SVC)
            .and_then(|b| b.reason)
            .is_some_and(|s| s == "plugin did not answer within timeout")
    });
    assert_ne!(r.backend(SVC).unwrap().state, HealthState::Unhealthy);
}

#[test]
fn invalid_plugin_output_is_logged() {
    let mark = marker("garbage");
    let d = Daemon::start(&format!(
        "{}{}",
        plugin_decl("fake", &["EMFOR_TEST_PLUGIN_GARBAGE=1"], &mark),
        plugin_service(SVC, "fake", "up", FAST)
    ));
    let r = Reader::open(&d.store());
    wait_for("backend healthy", || {
        r.backend(SVC)
            .is_some_and(|b| b.state == HealthState::Healthy)
    });
    let (_, log) = d.stop();
    assert!(
        log.lines()
            .any(|l| l.contains("WARN") && l.contains("plugin fake: ignoring invalid line")),
        "{log}"
    );
}

#[test]
fn check_config_validation_timeout() {
    let mark = marker("vhang");
    let t = Instant::now();
    let (code, _, err) = check_config(&format!(
        "{}{}{}",
        plugin_decl("fake", &["EMFOR_TEST_PLUGIN_VALIDATE_HANG=1"], &mark),
        plugin_service("a.svc.emfor", "fake", "up", ""),
        plugin_service("b.svc.emfor", "fake", "up", "")
    ));
    assert_eq!(code, 2);
    for svc in ["a.svc.emfor", "b.svc.emfor"] {
        assert!(
            err.contains(&format!(
                "services.\"{svc}\".check_options: plugin fake did not answer validation within 5s"
            )),
            "{err}"
        );
    }
    // The second service failed without waiting another 5s.
    assert!(
        t.elapsed() < Duration::from_secs(9),
        "took {:?}",
        t.elapsed()
    );
    assert!(pgrep(&mark).is_empty(), "plugin left running");
}

#[test]
fn reload_while_plugin_is_restarting() {
    let mark = marker("restarting");
    // Every check crashes the plugin, so it keeps restarting with growing
    // delays (1s, 2s, 4s, ...).
    let settings = "interval = \"200ms\"\ntimeout = \"100ms\"\n";
    let decl = plugin_decl("fake", &[], &mark);
    let d = Daemon::start(&format!(
        "{decl}{}",
        plugin_service(SVC, "fake", "crash", settings)
    ));
    wait_for("a 4s restart delay", || {
        d.log().contains("plugin fake restarting in 4s")
    });
    // New options cannot be validated while the plugin is down.
    d.reload(
        &format!("{decl}{}", plugin_service(SVC, "fake", "up", settings)),
        "reload rejected",
    );
    assert!(
        d.log()
            .contains("services.\"cache.svc.emfor\".check_options: plugin fake unavailable"),
        "{}",
        d.log()
    );
    // Unchanged options are accepted without asking the plugin.
    d.reload(
        &format!(
            "{decl}{}",
            plugin_service(SVC, "fake", "crash", &settings.replace("200ms", "300ms"))
        ),
        "config reloaded",
    );
    assert!(d.log().contains("restarting in 4s"));
}

/// A plugin that takes a second to start, so the daemon is still loading
/// its config when the test signals it.
fn slow_plugin_config(mark: &str) -> String {
    format!(
        "[plugins.\"slow\"]\ncommand = [\"/bin/sh\", \"-c\", \"sleep 1; exec {PLUGIN} {mark}\"]\n\n{}",
        plugin_service("s.svc.emfor", "slow", "up", FAST)
    )
}

/// Spawn the daemon with the slow plugin and wait until it has started the
/// plugin's shell: the daemon is now loading its config.
fn spawn_loading(mark: &str) -> Daemon {
    let d = Daemon::spawn(&slow_plugin_config(mark));
    let pid = d.child.id().to_string();
    wait_for("plugin spawned", || {
        Command::new("pgrep")
            .args(["-P", &pid])
            .output()
            .is_ok_and(|o| !o.stdout.is_empty())
    });
    d
}

#[test]
fn sighup_while_starting_reloads_once_started() {
    let mut d = spawn_loading(&marker("hup-start"));
    kill(Pid::from_raw(d.child.id() as i32), Signal::SIGHUP).unwrap();
    wait_for("reload after start", || d.log().contains("config reloaded"));
    assert!(d.child.try_wait().unwrap().is_none(), "{}", d.log());
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
    assert!(log.contains("emfor started"), "{log}");
}

#[test]
fn sigterm_while_starting_stops_cleanly() {
    let mark = marker("term-start");
    let d = spawn_loading(&mark);
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
    assert!(log.contains("shutting down"), "{log}");
    wait_for("plugin stopped", || pgrep(&mark).is_empty());
}

/// Spec "Plugin output line limit", oversized answer line.
#[test]
fn oversized_stdout_line_restarts_the_plugin() {
    let mark = marker("long-stdout");
    // One check, then 5s: an `error` keeps the normal interval, so nothing
    // overwrites the first check's reason while the plugin restarts (1s).
    let slow = "interval = \"5s\"\ntimeout = \"1s\"\nrise = 2\nfall = 2\n";
    let d = Daemon::start(&format!(
        "{}{}",
        plugin_decl("p", &[], &mark),
        plugin_service("s.svc.emfor", "p", "long_stdout", slow)
    ));
    wait_for("oversized line logged", || {
        d.log()
            .contains("plugin p wrote a line longer than 65536 bytes")
    });
    wait_for("plugin restarted", || d.started_pids("p").len() >= 2);
    let r = Reader::open(&d.store());
    let b = r.backend("s.svc.emfor").unwrap();
    assert_eq!(b.reason.as_deref(), Some("plugin exited"), "{}", d.log());
    assert_eq!(b.state, HealthState::Unknown);
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
}

/// Spec "Plugin output line limit", oversized stderr line.
#[test]
fn oversized_stderr_line_is_cut() {
    let mark = marker("long-stderr");
    let d = Daemon::start(&format!(
        "{}{}",
        plugin_decl("p", &[], &mark),
        plugin_service("s.svc.emfor", "p", "long_stderr", FAST)
    ));
    let r = Reader::open(&d.store());
    wait_for("backend healthy", || {
        r.backend("s.svc.emfor")
            .is_some_and(|b| b.state == HealthState::Healthy)
    });
    let log = d.log();
    assert!(log.contains(&"y".repeat(1000)), "stderr line logged");
    assert!(!log.contains(&"y".repeat(1001)), "cut for the log");
    assert_eq!(d.started_pids("p").len(), 1, "{log}");
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}
