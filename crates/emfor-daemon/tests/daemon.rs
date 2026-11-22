//! End-to-end tests: run the `emfor` binary and read its LMDB store.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use emfor_format::{HealthState, ServiceHistory};
use heed::types::Str;
use heed::{CommitOrAbort, Database, EnvFlags, EnvOpenOptions};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

mod common;
use common::{
    Daemon, Locked, Reader, emfor, free_port, http_backend, listener, sockets, store_header,
    wait_for, write_main_file,
};

fn tcp_service(name: &str, addr: &str) -> String {
    format!(
        "[services.\"{name}\"]\ncheck = \"tcp\"\nbackends = [\"{addr}\"]\ninterval = \"200ms\"\ntimeout = \"100ms\"\nrise = 2\nfall = 2\njitter = \"0%\"\n\n"
    )
}

/// A state writer that panics while the daemon runs (a store commit that
/// panics) makes the daemon log it and exit with status 70, which the
/// systemd unit restarts, instead of running on with a stale store.
#[test]
fn daemon_exits_70_when_the_writer_panics() {
    let dir = tempfile::tempdir().unwrap();
    let trigger = dir.path().join("panic-now");
    let mut d = Daemon::start_in(
        dir,
        &tcp_service("api.svc.emfor", "127.0.0.1:1"),
        &[("EMFOR_TEST_COMMIT_PANIC_FILE", &trigger)],
    );
    wait_for(|| d.log().contains("emfor started"));
    assert!(d.running(), "daemon stopped before the commit panicked");
    std::fs::write(&trigger, "").unwrap();
    wait_for(|| !d.running());
    let (code, log) = d.wait();
    assert_eq!(code, 70, "{log}");
    assert!(log.contains("test hook: a commit panics"), "{log}");
    assert!(log.contains("state writer panicked; exiting"), "{log}");
}

/// Start with a `[dns]` table on a free port. Tests run in parallel, so
/// another test can take the port between `free_port` and the daemon's bind;
/// then try again with a new port.
fn start_dns(services: &str) -> (Daemon, String) {
    for _ in 0..5 {
        let dns = format!("[dns]\nlisten = [\"127.0.0.1:{}\"]\n\n", free_port());
        let mut d = Daemon::start(&format!("{dns}{services}"));
        std::thread::sleep(Duration::from_millis(200));
        if d.running() {
            return (d, dns);
        }
        let (_, err) = d.wait();
        assert!(err.contains("Address already in use"), "{err}");
    }
    panic!("no free DNS port after 5 tries");
}

// spectra-scenario: service-config / Valid reload adds a service
#[test]
fn sighup_with_new_service_adds_its_record() {
    let (_l, addr) = listener();
    let d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    wait_for(|| r.service("api.svc.emfor").is_some());
    let cfg = format!(
        "{}{}{}",
        store_header(d.dir.path()),
        tcp_service("api.svc.emfor", &addr),
        tcp_service("web.svc.emfor", &addr)
    );
    write_main_file(&d.config(), &cfg);
    d.signal(Signal::SIGHUP);
    wait_for(|| r.service("web.svc.emfor").is_some());
    wait_for(|| r.service("web.svc.emfor").unwrap().backends[0].state == HealthState::Healthy);
}

// spectra-scenario: service-config / Invalid reload is rejected
#[test]
fn invalid_reload_keeps_previous_config() {
    let (_l, addr) = listener();
    let mut d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.service("api.svc.emfor")
            .is_some_and(|s| s.backends[0].state == HealthState::Healthy)
    });
    let before = r.svc_keys();
    let bad = format!("{}{}", store_header(d.dir.path()), tcp_service("api.svc.emfor", &addr))
        .replace("timeout = \"100ms\"", "timeout = \"5s\"")
        .replace("interval = \"200ms\"", "interval = \"2s\"\n[services.\"x.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:1\"]");
    write_main_file(&d.config(), &bad);
    d.signal(Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(500));
    assert!(d.running());
    assert_eq!(r.svc_keys(), before);
    assert_eq!(
        r.service("api.svc.emfor").unwrap().backends[0].state,
        HealthState::Healthy
    );
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    assert!(err.contains("timeout must not exceed interval"), "{err}");
    assert!(err.contains("reload rejected"), "{err}");
}

// spectra-scenario: service-config / Store settings change requires restart
#[test]
fn store_change_on_reload_requires_restart() {
    let (_l, addr) = listener();
    let d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let cfg = format!(
        "[store]\npath = \"{}\"\n\n{}",
        d.dir.path().join("other.mdb").display(),
        tcp_service("api.svc.emfor", &addr)
    );
    write_main_file(&d.config(), &cfg);
    d.signal(Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(300));
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    assert!(err.contains("store settings require restart"), "{err}");
}

// spectra-scenario: service-config / Running the daemon
// spectra-scenario: state-store / Heartbeat zero after shutdown
#[test]
fn sigterm_exits_zero_and_zeroes_heartbeat() {
    let (_l, addr) = listener();
    let d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let store = d.store();
    let r = Reader::open(&store);
    wait_for(|| r.get("meta/heartbeat").is_some_and(|h| h != "0"));
    let (code, _) = d.stop();
    assert_eq!(code, 0);
    assert_eq!(r.get("meta/heartbeat").as_deref(), Some("0"));
}

/// Spec "Damaged store is replaced at start": a store file whose meta
/// pages are zero bytes, as after a power loss with `MDB_NOSYNC`.
// spectra-scenario: state-store / Damaged store after a power loss
#[test]
fn damaged_store_is_replaced_at_start() {
    use std::io::Write;
    let (_l, addr) = listener();
    // A good store from a run that stopped, then its two meta pages zeroed.
    let mut d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let (path, page) = {
        let r = Reader::open(&d.store());
        wait_for(|| r.service("api.svc.emfor").is_some());
        (
            d.store(),
            u64::from(r.env.unnamed_database_stat().page_size),
        )
    };
    let dir = std::mem::replace(&mut d.dir, tempfile::tempdir().unwrap());
    let (code, err) = d.stop();
    assert_eq!(code, 0, "{err}");
    let len = std::fs::metadata(&path).unwrap().len();
    assert!(len > 2 * page, "store has data pages");
    let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.write_all(&vec![0u8; 2 * page as usize]).unwrap();
    drop(f);
    let corrupt = dir.path().join("state.mdb.corrupt");
    let d = Daemon::start_in(dir, &tcp_service("api.svc.emfor", &addr), &[]);
    wait_for(|| corrupt.exists());
    let r = Reader::open(&d.store());
    wait_for(|| r.get("meta/heartbeat").is_some_and(|h| h != "0"));
    assert!(r.service("api.svc.emfor").is_some());
    assert_eq!(std::fs::metadata(&corrupt).unwrap().len(), len);
    let store = d.store().display().to_string();
    let (code, err) = d.stop();
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("is damaged") && err.contains(&store), "{err}");
}

/// Spec "Reasons are bounded and printable": an escape sequence from a
/// check script reaches the store only escaped.
// spectra-scenario: health-checking / Escape sequence from a backend
#[test]
fn escape_sequence_in_a_reason_is_stored_escaped() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("check.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf 'bad\\033[2Jstate\\n'\nexit 1\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let svc = format!(
        "[services.\"x.svc.emfor\"]\ncheck = \"exec\"\nbackends = [\"127.0.0.1:1\"]\ninterval = \"200ms\"\ntimeout = \"100ms\"\njitter = \"0%\"\n[services.\"x.svc.emfor\".check_options]\ncommand = [\"{}\"]\n\n",
        script.display()
    );
    let d = Daemon::start_in(dir, &svc, &[]);
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.service("x.svc.emfor")
            .is_some_and(|s| s.backends[0].reason.is_some())
    });
    let reason = r.service("x.svc.emfor").unwrap().backends[0].reason.clone();
    assert_eq!(reason.as_deref(), Some("bad\\u{1b}[2Jstate"));
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

/// Spec "Check scheduling with jitter and no overlap": checks run at the
/// fast pace while a backend's state is unknown.
// spectra-scenario: service-config / Running the daemon
// spectra-scenario: health-checking / Fast checks until the state is known
#[test]
fn backend_known_within_a_second_of_start() {
    let (_l, addr) = listener();
    let svc = format!(
        "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"{addr}\"]\ninterval = \"2s\"\ntimeout = \"1s\"\nrise = 2\njitter = \"0%\"\n\n"
    );
    let d = Daemon::start(&svc);
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.service("api.svc.emfor")
            .is_some_and(|s| s.backends[0].state == HealthState::Healthy)
    });
    // From the daemon's own start time to the check that made it healthy.
    let started: u64 = r.get("meta/started_at").unwrap().parse().unwrap();
    let healthy_at = r.service("api.svc.emfor").unwrap().backends[0]
        .checked_at
        .unwrap();
    let took = healthy_at.saturating_sub(started);
    assert!(took < 1000, "healthy {took} ms after the start");
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

// spectra-scenario: state-store / Second daemon refused
// spectra-scenario: state-store / Another daemon's store is not replaced
#[test]
fn second_daemon_on_same_store_exits_one() {
    let (_l, addr) = listener();
    let mut d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let out = Command::new(cargo_bin("emfor"))
        .arg("server")
        .arg("--config")
        .arg(d.config())
        .env_remove("RUST_LOG")
        .output_locked()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!("{}.writer", d.store().display())),
        "{err}"
    );
    assert!(d.running());
    assert!(!PathBuf::from(format!("{}.corrupt", d.store().display())).exists());
}

// spectra-scenario: service-config / Valid config
// spectra-scenario: service-config / Invalid config in check-config
// spectra-scenario: service-config / Without the flag
#[test]
fn check_config_valid_and_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    write_main_file(
        &cfg,
        "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\", \"10.0.0.2:80\"]\n\n[services.\"db.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.1.1:5432\", \"10.0.1.2:5432\", \"[2001:db8::1]:5432\"]\n",
    );
    let out = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--config"])
        .arg(&cfg)
        .output_locked()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "config ok: 2 services, 5 backends"
    );

    write_main_file(
        &cfg,
        "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\nintervall = \"1s\"\n",
    );
    let out = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--config"])
        .arg(&cfg)
        .output_locked()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("services.\"api.svc.emfor\".intervall"));
}

// spectra-scenario: service-config / Only backup backends
#[test]
fn check_config_rejects_a_service_of_only_backups() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    let write = |backends: &str| {
        write_main_file(
            &cfg,
            &format!("[services.\"db.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [{backends}]\n"),
        );
        Command::new(cargo_bin("emfor"))
            .args(["check-config", "--config"])
            .arg(&cfg)
            .output_locked()
            .unwrap()
    };
    let out = write(r#""10.0.0.1:5432", { addr = "10.0.1.1:5432", backup = true }"#);
    assert_eq!(out.status.code(), Some(0));
    let out = write(
        r#"{ addr = "10.0.1.1:5432", backup = true }, { addr = "10.0.1.2:5432", backup = true }"#,
    );
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("services.\"db.svc.emfor\".backends")
            && err.contains("at least one backend must not be a backup"),
        "{err}"
    );
}

/// "Peer secret file owner", `emfor check-config` scenarios: the owner is
/// compared with the user that runs the command, and only after the file has
/// been opened. This runs the real binary as uid 1001, so it needs root to
/// switch users and to give files away (the Docker test run is root); as
/// another user it has nothing to check.
// spectra-scenario: peer-group / Secret owned by root or by the checking user
// spectra-scenario: peer-group / Check-config run by an account that is not the daemon user
// spectra-scenario: peer-group / Check-config accepts a secret owned by root for every account
#[test]
fn check_config_compares_the_secret_owner_with_the_user_that_runs_it() {
    use std::os::unix::fs::{PermissionsExt, chown};
    use std::os::unix::process::CommandExt;
    if nix::unistd::geteuid().as_raw() != 0 {
        return;
    }
    // The checking user: not root, not the owner of the files below.
    let user = 1001;
    let dir = tempfile::tempdir().unwrap();
    // A user other than root has to reach the files: the directory of a
    // temporary directory is 0700.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let key = dir.path().join("peers.key");
    std::fs::write(&key, [b'k'; 40]).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o640)).unwrap();
    let cfg = dir.path().join("emfor.toml");
    write_main_file(
        &cfg,
        &format!(
            "site = \"dc1\"\n\n[store]\npath = \"{}\"\nmap_size = \"4MiB\"\n\n[peers]\nlisten = \"127.0.0.1:7001\"\nmembers = [\"127.0.0.1:7001\", \"127.0.0.1:7002\"]\nsecret_file = \"{}\"\n\n[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
            dir.path().join("state.mdb").display(),
            key.display(),
        ),
    );
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o644)).unwrap();
    let check = || {
        Command::new(cargo_bin("emfor"))
            .args(["check-config", "--config"])
            .arg(&cfg)
            .uid(user)
            .gid(user)
            .output_locked()
            .unwrap()
    };

    // Owned by root, readable through the group: accepted.
    chown(&key, Some(0), Some(user)).unwrap();
    let out = check();
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    // Owned by the user that runs the command: accepted.
    chown(&key, Some(user), Some(user)).unwrap();
    let out = check();
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    // Owned by another user (the daemon's, say), readable through the
    // group: refused, naming that uid and the checking user's.
    chown(&key, Some(1000), Some(user)).unwrap();
    let out = check();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("owned by uid 1000, not root or the user running emfor (uid 1001)"),
        "{err}"
    );

    // Owned by another user and not readable by the checking user: the
    // owner is checked after the file is opened, so the error is that the
    // file cannot be read, not who owns it.
    chown(&key, Some(1000), Some(0)).unwrap();
    let out = check();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("cannot read"), "{err}");
    assert!(!err.contains("owned by uid"), "{err}");
}

/// Spec "Store file mode warning", `emfor check-config` scenario: a mode that
/// lets other users write is a warning on standard error that says what to
/// do, not an error; the default settings give none.
// spectra-scenario: state-store / World-writable lock file
#[test]
fn check_config_warns_about_a_store_mode_that_others_can_write() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    let svc = "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n";
    let check = |text: &str| {
        write_main_file(&cfg, text);
        Command::new(cargo_bin("emfor"))
            .args(["check-config", "--config"])
            .arg(&cfg)
            .output_locked()
            .unwrap()
    };

    let out = check(&format!(
        "[store]\npath = \"{}\"\nlock_file_mode = \"0666\"\n\n{svc}",
        dir.path().join("state.mdb").display()
    ));
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "config ok: 1 services, 1 backends"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = err
        .lines()
        .filter(|l| l.contains("lock_file_mode"))
        .collect();
    assert_eq!(lines.len(), 1, "{err}");
    for part in [
        "\"0666\"",
        "stop the daemon's commits",
        "Use \"0660\"",
        "in the group of the emfor daemon",
        "Who can use the store",
        "at the next start",
    ] {
        assert!(lines[0].contains(part), "{part}: {err}");
    }

    // The default settings: nothing to warn about.
    let out = check(svc);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains("file_mode") && !err.contains("Who can use the store"),
        "{err}"
    );
}

/// Spec "Store file mode warning", daemon scenario: one `warn` line at
/// start, the daemon runs, and a reload does not repeat it.
// spectra-scenario: state-store / Daemon starts with a world-writable mode
#[test]
fn daemon_warns_at_start_about_a_store_mode_that_others_can_write() {
    let (_l, addr) = listener();
    let svc = tcp_service("api.svc.emfor", &addr);
    let mut d = Daemon::spawn_with(
        |dir| {
            format!(
                "[store]\npath = \"{}\"\nmap_size = \"4MiB\"\nfile_mode = \"0666\"\n\n{svc}",
                dir.join("state.mdb").display()
            )
        },
        &[],
    );
    d.wait_log("emfor started");
    assert!(d.running());
    let log = d.log();
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("Who can use the store"))
        .collect();
    assert_eq!(lines.len(), 1, "{log}");
    for part in [
        "WARN",
        "file_mode",
        "\"0666\"",
        "change what every lookup returns",
        "Use \"0640\"",
        "in the group of the emfor daemon",
    ] {
        assert!(lines[0].contains(part), "{part}: {log}");
    }

    // A reload applies the new service and does not log the warning again.
    let r = Reader::open(&d.store());
    d.write_whole_config(&format!(
        "[store]\npath = \"{}\"\nmap_size = \"4MiB\"\nfile_mode = \"0666\"\n\n{svc}{}",
        d.store().display(),
        tcp_service("web.svc.emfor", &addr)
    ));
    d.signal(Signal::SIGHUP);
    wait_for(|| r.service("web.svc.emfor").is_some());
    assert_eq!(d.log().matches("Who can use the store").count(), 1);
    assert!(d.running());
}

/// Give the main config file of `d` the mode `mode`.
fn chmod_config(d: &Daemon, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(d.config(), std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Spec service-config "Config directory permissions", "Start with a main
/// file that others can write" and "Main file that others can write": the
/// daemon and `check-config` refuse it with status 2 and the fix, and the
/// store is not opened.
// spectra-scenario: service-config / Main file that others can write
// spectra-scenario: service-config / Start with a main file that others can write
#[test]
fn main_file_that_others_can_write_is_refused_at_start_and_by_check_config() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    let (_l, addr) = listener();
    write_main_file(
        &cfg,
        &format!(
            "{}{}",
            store_header(dir.path()),
            tcp_service("api.svc.emfor", &addr)
        ),
    );
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o664)).unwrap();
    let path = cfg.to_str().unwrap();
    for cmd in ["check-config", "server"] {
        let (code, _, err) = emfor(&[cmd, "--config", path]);
        assert_eq!(code, 2, "{cmd}: {err}");
        assert!(
            err.contains(path)
                && err.contains("mode 0664")
                && err.contains(&format!("chmod g-w,o-w {path}")),
            "{cmd}: {err}"
        );
    }
    assert!(
        !dir.path().join("state.mdb").exists(),
        "the store was opened"
    );

    // With the mode fixed both accept it.
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (code, _, err) = emfor(&["check-config", "--config", path]);
    assert_eq!(code, 0, "{err}");
}

/// Spec "Reload with a main file that others can write": the reload is
/// rejected, the daemon keeps its config, and a good file is read again.
// spectra-scenario: service-config / Reload with a main file that others can write
#[test]
fn reload_with_a_main_file_that_others_can_write_keeps_the_old_config() {
    let (_l, addr) = listener();
    let mut d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.service("api.svc.emfor")
            .is_some_and(|s| s.backends[0].state == HealthState::Healthy)
    });
    let with_web = format!(
        "{}{}{}",
        store_header(d.dir.path()),
        tcp_service("api.svc.emfor", &addr),
        tcp_service("web.svc.emfor", &addr)
    );
    d.write_whole_config(&with_web);
    chmod_config(&d, 0o664);
    d.signal(Signal::SIGHUP);
    d.wait_log("reload rejected");
    assert!(d.running());
    let log = d.log();
    assert!(
        log.contains("mode 0664") && log.contains("chmod g-w,o-w"),
        "{log}"
    );
    assert!(r.service("web.svc.emfor").is_none());
    assert_eq!(
        r.service("api.svc.emfor").unwrap().backends[0].state,
        HealthState::Healthy
    );

    // The mode fixed, the next reload applies the file.
    chmod_config(&d, 0o644);
    d.signal(Signal::SIGHUP);
    wait_for(|| r.service("web.svc.emfor").is_some());
}

/// Spec "status and lookup do not check the main file": they read only
/// `[store]` and run no plugin.
// spectra-scenario: service-config / status and lookup do not check the main file
#[test]
fn status_and_lookup_read_a_main_file_that_others_can_write() {
    let (_l, addr) = listener();
    let d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.service("api.svc.emfor")
            .is_some_and(|s| s.backends[0].state == HealthState::Healthy)
    });
    chmod_config(&d, 0o664);
    let cfg = d.config();
    let (code, out, err) = status(&cfg, &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("api.svc.emfor"), "{out}");
    let (code, out, err) = emfor(&["lookup", "api.svc.emfor", "--config", cfg.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains(&addr), "{out}");
}

/// A `conf.d` next to `dir`'s config, mode 0755 as a package makes it.
fn conf_d(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let d = dir.join("conf.d");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    d
}

/// A fragment in `conf_d`, mode 0644.
fn fragment(conf_d: &Path, name: &str, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    let p = conf_d.join(name);
    std::fs::write(&p, text).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
}

/// A config that reads `conf.d`, with the fragments `(name, text)`.
fn with_conf_d(fragments: &'static [(&'static str, &'static str)]) -> impl FnOnce(&Path) -> String {
    move |dir| {
        let d = conf_d(dir);
        for (name, text) in fragments {
            fragment(&d, name, text);
        }
        format!("include_dir = \"conf.d\"\n\n{}", store_header(dir))
    }
}

/// Spec service-config "Config directory with ordered fragments" and
/// "Listing the files read": check-config lists the files, in read order,
/// only when `include_dir` is set.
// spectra-scenario: service-config / check-config with fragments
// spectra-scenario: service-config / check-config without include_dir
// spectra-scenario: service-config / Without the flag
#[test]
fn check_config_lists_the_files_it_read() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    let d = conf_d(dir.path());
    fragment(
        &d,
        "20-b.toml",
        "[services.\"b.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.2:80\"]\n",
    );
    fragment(
        &d,
        "10-a.toml",
        "[services.\"a.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
    );
    write_main_file(&cfg, "include_dir = \"conf.d\"\n");
    let out = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--config"])
        .arg(&cfg)
        .output_locked()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let shown = |name: &str| format!("read {}", dir.path().join(name).display());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!(
            "config ok: 2 services, 2 backends\n{}\n{}\n{}\n",
            shown("emfor.toml"),
            shown("conf.d/10-a.toml"),
            shown("conf.d/20-b.toml")
        )
    );

    // Without include_dir the output is the single line it always was.
    write_main_file(
        &cfg,
        "[services.\"a.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
    );
    let out = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--config"])
        .arg(&cfg)
        .output_locked()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "config ok: 1 services, 1 backends\n"
    );
}

/// Spec service-config "Showing the merged config": the merged TOML alone on
/// standard output, the usual lines on standard error, and the saved output
/// checks again.
// spectra-scenario: service-config / Valid config with fragments
// spectra-scenario: service-config / Round trip
// spectra-scenario: service-config / Invalid config when showing the merged config
#[test]
fn check_config_show_prints_the_merged_config() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    let d = conf_d(dir.path());
    fragment(
        &d,
        "99-local.toml",
        "[services.\"a.svc.emfor\"]\ninterval = \"5s\"\n",
    );
    write_main_file(
        &cfg,
        "include_dir = \"conf.d\"\n\n[services.\"a.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\ninterval = \"2s\"\n",
    );
    let out = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--show", "--config"])
        .arg(&cfg)
        .output_locked()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("config ok: 1 services, 1 backends\nread "),
        "{stderr}"
    );
    assert!(!stdout.contains("config ok"), "{stdout}");
    let frag = d.join("99-local.toml");
    assert!(
        stdout.contains(&format!("interval = \"5s\" # {}", frag.display())),
        "{stdout}"
    );

    // Round trip: the output, saved as it is in another directory (no
    // `conf.d` there), is a config that checks, and `--show` of it gives the
    // same TOML without the comments.
    let other = tempfile::tempdir().unwrap();
    let saved = other.path().join("merged.toml");
    write_main_file(&saved, &stdout);
    let again = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--config"])
        .arg(&saved)
        .output_locked()
        .unwrap();
    assert_eq!(
        again.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&again.stdout),
        "config ok: 1 services, 1 backends\n"
    );
    let shown_again = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--show", "--config"])
        .arg(&saved)
        .output_locked()
        .unwrap();
    let first: toml::Table = stdout.parse().unwrap();
    let second: toml::Table = String::from_utf8_lossy(&shown_again.stdout)
        .parse()
        .unwrap();
    assert_eq!(first, second);
    assert!(
        String::from_utf8_lossy(&shown_again.stdout)
            .starts_with(&format!("# merged from {}", saved.display()))
    );

    // An invalid config prints nothing on standard output.
    write_main_file(&cfg, "intervall = 1\n");
    let bad = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--show", "--config"])
        .arg(&cfg)
        .output_locked()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(bad.stdout.is_empty());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("intervall"));
}

/// A write error on standard output is reported with a status, not a panic
/// (101): `emfor check-config > /dev/full`. Only Linux has /dev/full.
// spectra-scenario: service-config / Full disk
#[cfg(target_os = "linux")]
#[test]
fn check_config_reports_a_full_disk() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    write_main_file(
        &cfg,
        "[services.\"a.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
    );
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let out = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--config"])
        .arg(&cfg)
        .stdin(Stdio::null())
        .stdout(full)
        .stderr(Stdio::piped())
        .spawn_locked()
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot write the output"));
}

/// A reader that has gone away is not a failure: `emfor check-config | head`.
/// The read end is closed before the child writes, so it gets EPIPE.
// spectra-scenario: service-config / Closed pipe
#[test]
fn check_config_ignores_a_closed_pipe() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    write_main_file(
        &cfg,
        "[services.\"a.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
    );
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--config"])
        .arg(&cfg)
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::piped())
        .spawn_locked()
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A name that is not UTF-8 is skipped with a warning that `check-config`,
/// which has no log, prints on stderr. Only Linux can create such a name.
// spectra-scenario: service-config / Skipped name reported by check-config and by a failed load
#[cfg(target_os = "linux")]
#[test]
fn check_config_warns_about_a_non_utf8_file_name() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    let d = conf_d(dir.path());
    std::fs::write(d.join(std::ffi::OsStr::from_bytes(b"b\xff.toml")), "").unwrap();
    write_main_file(&cfg, "include_dir = \"conf.d\"\n");
    let out = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--config"])
        .arg(&cfg)
        .output_locked()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not valid UTF-8"));
}

/// A fragment that changes `[dns] listen` is caught by the reload's restart
/// check, as the same change in the main file is.
// spectra-scenario: service-config / Fragment changes a listen address
#[test]
fn listen_change_in_a_fragment_requires_restart() {
    let mut d = Daemon::spawn_with(
        with_conf_d(&[(
            "10-a.toml",
            "[services.\"a.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"127.0.0.1:1\"]\n",
        )]),
        &[],
    );
    d.wait_log("emfor started");
    let port = free_port();
    fragment(
        &d.dir.path().join("conf.d"),
        "20-dns.toml",
        &format!("[dns]\nlisten = [\"127.0.0.1:{port}\"]\n"),
    );
    d.signal(Signal::SIGHUP);
    d.wait_log("reload rejected");
    assert!(
        d.log().contains("dns listen settings require restart"),
        "{}",
        d.log()
    );
    assert!(d.running());
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

/// Spec "Missing config directory": at start it is a config error.
// spectra-scenario: service-config / Missing directory at start
#[test]
fn missing_include_dir_at_startup_exits_two_without_store() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    write_main_file(
        &cfg,
        &format!("include_dir = \"conf.d\"\n\n{}", store_header(dir.path())),
    );
    let out = Command::new(cargo_bin("emfor"))
        .arg("server")
        .arg("--config")
        .arg(&cfg)
        .env_remove("RUST_LOG")
        .output_locked()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("include_dir"));
    assert!(!dir.path().join("state.mdb").exists());
}

/// Spec "Fragment merge rules", "Errors name their file" and "Missing
/// config directory": a reload re-reads the fragments; any error, a missing
/// directory included, rejects it and the daemon keeps its config.
// spectra-scenario: service-config / Directory removed before a reload
// spectra-scenario: service-config / One bad fragment rejects a reload
#[test]
fn reload_rereads_fragments_and_rejects_any_error() {
    let mut d = Daemon::spawn_with(
        with_conf_d(&[(
            "10-a.toml",
            "[services.\"a.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"127.0.0.1:1\"]\n",
        )]),
        &[],
    );
    d.wait_log("emfor started");
    let r = Reader::open(&d.store());
    wait_for(|| r.service("a.svc.emfor").is_some());
    assert!(d.log().contains("config file read:") && d.log().contains("10-a.toml"));

    let conf = d.dir.path().join("conf.d");
    fragment(
        &conf,
        "20-b.toml",
        "[services.\"b.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"127.0.0.1:1\"]\n",
    );
    d.signal(Signal::SIGHUP);
    wait_for(|| r.service("b.svc.emfor").is_some());

    fragment(
        &conf,
        "30-bad.toml",
        "[services.\"a.svc.emfor\"]\nrise = 0\n",
    );
    fragment(
        &conf,
        "25-c.toml",
        "[services.\"c.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"127.0.0.1:1\"]\n",
    );
    d.signal(Signal::SIGHUP);
    d.wait_log("reload rejected");
    assert!(d.log().contains("30-bad.toml"), "{}", d.log());
    assert!(r.service("c.svc.emfor").is_none());
    assert!(d.running());

    std::fs::remove_dir_all(&conf).unwrap();
    d.signal(Signal::SIGHUP);
    wait_for(|| d.log().matches("reload rejected").count() >= 2);
    assert!(d.running());
    assert!(r.service("a.svc.emfor").is_some());
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

// spectra-scenario: service-config / Daemon refuses an invalid config
#[test]
fn invalid_config_at_startup_exits_two_without_store() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    let store = dir.path().join("state.mdb");
    write_main_file(
        &cfg,
        &format!(
            "{}[services.a]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\nrise = 0\n",
            store_header(dir.path())
        ),
    );
    let out = Command::new(cargo_bin("emfor"))
        .arg("server")
        .arg("--config")
        .arg(&cfg)
        .env_remove("RUST_LOG")
        .output_locked()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("rise must be at least 1"));
    assert!(!store.exists());
}

/// A copy of `examples/emfor.toml` with mode 0644: the daemon refuses a main
/// file that group or others can write, and the mode of a checkout follows
/// the umask of whoever made it.
fn example_config() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/emfor.toml"),
    )
    .unwrap();
    let path = dir.path().join("emfor.toml");
    write_main_file(&path, &text);
    (dir, path)
}

#[test]
fn example_config_is_valid() {
    let (_dir, example) = example_config();
    let text = std::fs::read_to_string(&example).unwrap();
    for line in [
        "[dns]",
        "site = ",
        "balance = \"failover\"",
        "balance = \"weighted\"",
        "balance = \"locality\"",
        "dns_ttl = ",
        "expect = ",
        "tls = true",
    ] {
        assert!(
            text.lines().any(|l| l.starts_with(line)),
            "example shows {line} uncommented"
        );
    }
    let out = Command::new(cargo_bin("emfor"))
        .args(["check-config", "--config"])
        .arg(&example)
        .output_locked()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "config ok: 7 services, 17 backends"
    );
}

/// Write `pairs` into a store at `path` (created if missing) before the
/// daemon runs, as a previous run would have left them.
fn seed_store(path: &Path, pairs: &[(&str, &str)]) {
    let env = unsafe {
        let mut o = EnvOpenOptions::new().read_txn_without_tls();
        o.flags(EnvFlags::NO_SUB_DIR).map_size(4 << 20);
        o.open(path).unwrap()
    };
    let db: Database<Str, Str> = env
        .create_databases_and_commit(env.write_txn().unwrap(), |o, w| -> heed::Result<_> {
            Ok(CommitOrAbort::<_, ()>::Commit(
                o.create_database::<Str, Str>(w, None)?,
            ))
        })
        .unwrap()
        .unwrap_commit();
    let mut wtxn = env.write_txn().unwrap();
    for (k, v) in pairs {
        db.put(&mut wtxn, k, v).unwrap();
    }
    wtxn.commit().unwrap();
}

/// Spec state-store "Backend state history", scenario "Unparseable history
/// discarded".
// spectra-scenario: state-store / Unparseable history discarded
#[test]
fn unparseable_history_discarded_at_start() {
    let dir = tempfile::tempdir().unwrap();
    seed_store(
        &dir.path().join("state.mdb"),
        &[("hist/api.svc.emfor", "not json")],
    );
    let (_l, addr) = listener();
    let d = Daemon::start_in(dir, &tcp_service("api.svc.emfor", &addr), &[]);
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.get("hist/api.svc.emfor")
            .and_then(|v| serde_json::from_str::<ServiceHistory>(&v).ok())
            .is_some_and(|h| h.backends.contains_key(&addr))
    });
    let h: ServiceHistory = serde_json::from_str(&r.get("hist/api.svc.emfor").unwrap()).unwrap();
    assert_eq!(h.backends[&addr][0].from, HealthState::Unknown);
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    assert!(err.contains("discarding hist/api.svc.emfor"), "{err}");
}

/// Acceptance: a backend that stops accepting connections becomes unhealthy
/// after `fall` checks, and healthy again after `rise` checks once it listens.
#[test]
fn backend_goes_unhealthy_and_recovers() {
    let (l, addr) = listener();
    let d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    let state = || r.service("api.svc.emfor").map(|s| s.backends[0].state);
    let latency = || {
        r.service("api.svc.emfor")
            .and_then(|s| s.backends[0].latency_us)
    };
    wait_for(|| state() == Some(HealthState::Healthy));
    assert!(!r.service("api.svc.emfor").unwrap().all_down);
    // Spec "Check latency": a number after up checks (committed by the next
    // heartbeat at the latest), null once the backend is unhealthy.
    wait_for(|| latency().is_some());

    drop(l);
    wait_for(|| state() == Some(HealthState::Unhealthy));
    let rec = r.service("api.svc.emfor").unwrap();
    assert!(rec.all_down);
    assert_eq!(rec.backends[0].latency_us, None);
    assert!(
        rec.backends[0]
            .reason
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains("refused")
    );

    let _l2 = sockets(|| TcpListener::bind(&addr)).unwrap();
    wait_for(|| state() == Some(HealthState::Healthy));
    assert_eq!(r.service("api.svc.emfor").unwrap().backends[0].reason, None);
    wait_for(|| latency().is_some());
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

#[test]
fn reload_reports_store_change_together_with_other_errors() {
    let (_l, addr) = listener();
    let d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let cfg = format!(
        "[store]\npath = \"{}\"\n\n{}",
        d.dir.path().join("other.mdb").display(),
        tcp_service("api.svc.emfor", &addr).replace("rise = 2", "rise = 0")
    );
    write_main_file(&d.config(), &cfg);
    d.signal(Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(300));
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    assert!(err.contains("rise must be at least 1"), "{err}");
    assert!(err.contains("store settings require restart"), "{err}");
}

/// Spec scenario "Mixed ports in the zone": a warning at startup naming both
/// ports, and A queries still answered.
// spectra-scenario: dns-server / Mixed ports in the zone
#[test]
fn startup_reports_backends_on_different_ports() {
    use hickory_proto::op::{Message, Query};
    use hickory_proto::rr::{Name, RecordType};
    let (_l1, a1) = listener();
    let (_l2, a2) = listener();
    let svc = tcp_service("api.svc.emfor", &a1).replace(
        &format!("backends = [\"{a1}\"]"),
        &format!("backends = [\"{a1}\", \"{a2}\"]"),
    );
    let (d, dns) = start_dns(&svc);
    let dns_port: u16 = dns
        .split(':')
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let mut q = Message::query();
    q.add_query(Query::query(
        Name::from_ascii("api.svc.emfor.").unwrap(),
        RecordType::A,
    ));
    let sock = sockets(|| std::net::UdpSocket::bind("127.0.0.1:0")).unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let mut answered = false;
    for _ in 0..20 {
        sock.send_to(&q.to_vec().unwrap(), ("127.0.0.1", dns_port))
            .unwrap();
        let mut buf = [0u8; 512];
        if let Ok(n) = sock.recv(&mut buf) {
            let m = Message::from_vec(&buf[..n]).unwrap();
            if m.answers.len() == 1 {
                answered = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let (code, err) = d.stop();
    assert_eq!(code, 0, "{err}");
    assert!(answered, "no A answer: {err}");
    let mut ports: Vec<u16> = [&a1, &a2]
        .iter()
        .map(|a| a.rsplit_once(':').unwrap().1.parse().unwrap())
        .collect();
    ports.sort();
    let line = err
        .lines()
        .find(|l| l.contains("different ports"))
        .unwrap_or_else(|| panic!("no port warning: {err}"));
    assert!(
        line.contains("WARN") && line.contains("api.svc.emfor"),
        "{line}"
    );
    assert!(
        line.contains(&format!("({}, {})", ports[0], ports[1])),
        "{line}"
    );
}

/// Spec scenario "Reload reports the new config": no port warning while all
/// backends share a port, a warning naming both ports after a reload adds
/// a backend on another port.
// spectra-scenario: dns-server / Reload reports the new config
#[test]
fn reload_reports_backends_on_different_ports() {
    let (_l, addr) = listener();
    let (d, dns) = start_dns(&tcp_service("api.svc.emfor", &addr));
    let port = addr.rsplit_once(':').unwrap().1;
    let other = if port == "9090" { "9091" } else { "9090" };
    let cfg = format!(
        "{}{dns}{}",
        store_header(d.dir.path()),
        tcp_service("api.svc.emfor", &addr).replace(
            &format!("backends = [\"{addr}\"]"),
            &format!("backends = [\"{addr}\", \"127.0.0.2:{other}\"]")
        )
    );
    std::thread::sleep(Duration::from_millis(300));
    write_main_file(&d.config(), &cfg);
    d.signal(Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(500));
    let (code, err) = d.stop();
    assert_eq!(code, 0, "{err}");
    let mut ports = [port.parse::<u16>().unwrap(), other.parse().unwrap()];
    ports.sort();
    let warn_lines: Vec<&str> = err
        .lines()
        .filter(|l| l.contains("different ports"))
        .collect();
    assert_eq!(warn_lines.len(), 1, "{err}");
    let line = warn_lines[0];
    assert!(line.contains("WARN"), "{line}");
    assert!(line.contains("api.svc.emfor"), "{line}");
    assert!(
        line.contains(&format!("({}, {})", ports[0], ports[1])),
        "{line}"
    );
    let reloaded = err.find("config reloaded").expect("reload logged");
    assert!(err.find("different ports").unwrap() < reloaded, "{err}");
}

#[test]
fn reload_applies_log_level() {
    let (_l, addr) = listener();
    let d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let cfg = format!(
        "{}[log]\nlevel = \"debug\"\n\n{}",
        store_header(d.dir.path()),
        tcp_service("api.svc.emfor", &addr)
    );
    write_main_file(&d.config(), &cfg);
    d.signal(Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(300));
    let (code, err) = d.stop();
    assert_eq!(code, 0);
    assert!(err.contains("log level set to debug"), "{err}");
}

/// Spec scenario "Service record after checks": one healthy and one unhealthy
/// backend, in config order, with `all_down` false and a reason on the failed one.
// spectra-scenario: state-store / Service record after checks
#[test]
fn service_record_with_healthy_and_unhealthy_backends() {
    let (_l, up) = listener();
    let (closed, down) = listener();
    drop(closed);
    let svc = tcp_service("api.svc.emfor", &up).replace(
        &format!("backends = [\"{up}\"]"),
        &format!("backends = [\"{up}\", \"{down}\"]"),
    );
    let d = Daemon::start(&svc);
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.service("api.svc.emfor").is_some_and(|s| {
            s.backends[0].state == HealthState::Healthy
                && s.backends[1].state == HealthState::Unhealthy
        })
    });
    let rec = r.service("api.svc.emfor").unwrap();
    assert!(!rec.all_down);
    assert_eq!(
        rec.backends
            .iter()
            .map(|b| b.id.as_str())
            .collect::<Vec<_>>(),
        vec![up.as_str(), down.as_str()]
    );
    assert_eq!(rec.backends[0].reason, None);
    assert!(rec.backends[1].reason.is_some());
}

// spectra-scenario: service-config / Balance change applies on reload
// spectra-scenario: state-store / Balance written from config
#[test]
fn balance_written_from_config_and_changed_on_reload() {
    use emfor_format::Balance;
    let (_l, addr) = listener();
    let svc = tcp_service("api.svc.emfor", &addr);
    let d = Daemon::start(&format!("{svc}balance = \"failover\"\n"));
    let r = Reader::open(&d.store());
    wait_for(|| r.service("api.svc.emfor").is_some());
    let rec = r.get("svc/api.svc.emfor").unwrap();
    assert!(
        rec.contains(r#""on_all_down":"open","balance":"failover","all_down""#),
        "{rec}"
    );
    write_main_file(
        &d.config(),
        &format!("{}{svc}balance = \"sticky\"\n", store_header(d.dir.path())),
    );
    d.signal(Signal::SIGHUP);
    wait_for(|| r.service("api.svc.emfor").unwrap().balance == Balance::Sticky);
}

fn fastest_service(name: &str, check: &str, backends: &[&str]) -> String {
    let list: Vec<String> = backends.iter().map(|b| format!("\"{b}\"")).collect();
    format!(
        "[services.\"{name}\"]\ncheck = \"{check}\"\nbalance = \"fastest\"\nbackends = [{}]\ninterval = \"200ms\"\ntimeout = \"150ms\"\nrise = 1\nfall = 1\njitter = \"0%\"\n\n",
        list.join(", ")
    )
}

// spectra-scenario: backend-selection / New methods accepted
// spectra-scenario: backend-selection / All down open
// spectra-scenario: state-store / Pool written for a fastest service
#[test]
fn fastest_pool_follows_health() {
    let (l, addr) = listener();
    let d = Daemon::start(&fastest_service("api.svc.emfor", "tcp", &[&addr]));
    let r = Reader::open(&d.store());
    let pool = || r.service("api.svc.emfor").and_then(|s| s.pool);
    wait_for(|| pool() == Some(vec![addr.clone()]));
    assert_eq!(
        r.service("api.svc.emfor").unwrap().balance,
        emfor_format::Balance::Fastest
    );
    drop(l);
    wait_for(|| pool() == Some(vec![]));
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

#[test]
fn fastest_pool_excludes_slow_backend() {
    let fast = http_backend(Duration::ZERO);
    let slow = http_backend(Duration::from_millis(30));
    let d = Daemon::start(&fastest_service("api.svc.emfor", "http", &[&fast, &slow]));
    let r = Reader::open(&d.store());
    let ready = || {
        r.service("api.svc.emfor").is_some_and(|s| {
            s.backends
                .iter()
                .all(|b| b.state == HealthState::Healthy && b.latency_us.is_some())
        })
    };
    wait_for(ready);
    // Commits of pool-only changes follow the heartbeat; watch for 2 s.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let s = r.service("api.svc.emfor").unwrap();
        assert_eq!(s.pool, Some(vec![fast.clone()]), "{s:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

// spectra-scenario: service-config / Help shows the built-in default
#[test]
fn help_shows_the_built_in_config_path() {
    let out = Command::new(cargo_bin("emfor"))
        .arg("--help")
        .output_locked()
        .unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    // Tests are normally built without EMFOR_PREFIX; with it, only the
    // suffix is known here.
    let want = match std::env::var("EMFOR_PREFIX") {
        Err(_) => "[default: /etc/emfor/emfor.toml]".to_owned(),
        Ok(_) => "etc/emfor/emfor.toml]".to_owned(),
    };
    assert!(help.contains(&want), "{help}");
}

/// Run `emfor <args>`: (exit code, stdout, stderr). Fails instead of
/// hanging when emfor has not exited within 10 s (for example, because it
/// started a daemon).
/// Service-config "Command line subcommands": without a usable subcommand,
/// emfor prints usage or the `--check-config` hint, exits 2, and leaves
/// the store alone.
// spectra-scenario: service-config / No subcommand
// spectra-scenario: service-config / No subcommand / invocations without a usable subcommand
// spectra-scenario: service-config / No subcommand leaves the store alone
#[test]
fn no_subcommand_prints_usage_and_touches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    let store = dir.path().join("state.mdb");
    write_main_file(
        &cfg,
        &format!(
            "{}[services.a]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
            store_header(dir.path())
        ),
    );
    let cfg = cfg.to_str().unwrap();
    let hint = "--check-config was replaced by \"emfor check-config\"";
    let rows: [(&[&str], &[&str]); 5] = [
        (&[], &["server", "check-config", "status"]),
        (&["--config", cfg], &["server", "check-config", "status"]),
        (&["--check-config"], &[hint]),
        (&["--check-config", "--config", cfg], &[hint]),
        (&["server", "--check-config"], &[hint]),
    ];
    for (args, want) in rows {
        let (code, out, err) = emfor(args);
        assert_eq!(code, 2, "emfor {args:?}: {err}");
        assert_eq!(out, "", "emfor {args:?}");
        for w in want {
            assert!(err.contains(w), "emfor {args:?}: no {w:?} in {err}");
        }
    }
    for f in ["state.mdb", "state.mdb-lock", "state.mdb.writer"] {
        assert!(!dir.path().join(f).exists(), "{f} was created");
    }
    assert!(!store.exists());
}

/// Service-config "Command line subcommands", scenario "No subcommand":
/// beside a running daemon, `emfor` prints usage and the daemon keeps
/// publishing heartbeats.
// spectra-scenario: service-config / No subcommand
#[test]
fn no_subcommand_beside_a_running_daemon() {
    let (_l, addr) = listener();
    let mut d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    let heartbeat = || r.dump().get("meta/heartbeat").cloned().unwrap_or_default();
    wait_for(|| r.service("api.svc.emfor").is_some());
    let cfg = d.config();
    for args in [vec![], vec!["--config", cfg.to_str().unwrap()]] {
        let (code, _, err) = emfor(&args);
        assert_eq!(code, 2, "emfor {args:?}: {err}");
        assert!(err.contains("server"), "emfor {args:?}: {err}");
    }
    assert!(d.running());
    let before = heartbeat();
    wait_for(|| heartbeat() != before);
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

// spectra-scenario: service-config / Global config option position
#[test]
fn config_option_before_or_after_the_subcommand() {
    let (_dir, example) = example_config();
    let example = example.to_str().unwrap();
    let before = emfor(&["--config", example, "check-config"]);
    let after = emfor(&["check-config", "--config", example]);
    assert_eq!(before.0, 0, "{}", before.2);
    assert!(before.1.starts_with("config ok: "), "{}", before.1);
    assert_eq!(before, after);
}

#[test]
fn help_does_not_list_the_old_flag() {
    let (code, out, _) = emfor(&["--help"]);
    assert_eq!(code, 0);
    assert!(!out.contains("--check-config"), "{out}");
    for w in ["server", "check-config", "status"] {
        assert!(out.contains(w), "no {w} in {out}");
    }
}

/// Run `emfor status --config <cfg> <args>`: (exit code, stdout, stderr).
fn status(cfg: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(cargo_bin("emfor"))
        .arg("status")
        .arg("--config")
        .arg(cfg)
        .args(args)
        .env_remove("NO_COLOR")
        .output_locked()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// spectra-scenario: status-command / Status beside a running daemon
// spectra-scenario: status-command / Rest of the config is not validated
// spectra-scenario: status-command / Stopped daemon
// spectra-scenario: status-command / JSON for one service
// spectra-scenario: status-command / Unknown service
// spectra-scenario: status-command / Exit statuses
#[test]
fn status_beside_a_running_daemon() {
    let (_l, addr) = listener();
    let d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    wait_for(|| {
        r.service("api.svc.emfor")
            .is_some_and(|s| s.backends[0].state == HealthState::Healthy)
    });

    let (code, out, err) = status(&d.config(), &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("daemon: running, heartbeat "), "{out}");
    assert!(
        out.contains("api.svc.emfor  round-robin  on_all_down=open\n"),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l.starts_with(&format!("  {addr}  healthy  "))),
        "{out}"
    );
    assert!(
        !out.contains('\x1b'),
        "escape sequences in piped output: {out:?}"
    );

    // The daemon keeps writing heartbeats after status read the store.
    let hb = r.get("meta/heartbeat").unwrap();
    wait_for(|| r.get("meta/heartbeat").is_some_and(|h| h != hb));

    let (code, out, _) = status(&d.config(), &["--json", "--service", "api.svc.emfor"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["daemon"], "running");
    assert_eq!(v["services"].as_array().unwrap().len(), 1);
    assert_eq!(v["services"][0]["name"], "api.svc.emfor");

    let (code, _, err) = status(&d.config(), &["--service", "nope.svc.emfor"]);
    assert_eq!((code, err.trim()), (2, "service nope.svc.emfor not found"));

    // A broken service table does not stop status: only [store] is read.
    let text = std::fs::read_to_string(d.config()).unwrap();
    write_main_file(
        &d.config(),
        &text.replace("check = \"tcp\"", "check = \"tcp\"\nintervall = \"1s\""),
    );
    let (code, out, err) = status(&d.config(), &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("api.svc.emfor"));

    // Stop the daemon but keep its directory (Daemon::stop drops it).
    let mut d = d;
    d.signal(Signal::SIGTERM);
    assert!(d.child.wait().unwrap().success());
    let (code, out, _) = status(&d.config(), &[]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("daemon: stopped\n"), "{out}");
    assert!(out.contains("api.svc.emfor"), "{out}");
}

/// Status-command "Status text output" and "Status filtering and JSON
/// output": the daemon line and `--json` show the `[dns]` and `[metrics]`
/// addresses the running daemon wrote to `meta/dns_listen` and
/// `meta/metrics_listen`.
// spectra-scenario: state-store / Metrics table written at start
// spectra-scenario: state-store / DNS table written at start
// spectra-scenario: status-command / Metrics addresses on the daemon line
// spectra-scenario: status-command / DNS and metrics addresses on the daemon line
// spectra-scenario: status-command / JSON metrics addresses
// spectra-scenario: status-command / JSON DNS addresses
#[test]
fn status_shows_listen_addresses() {
    let (_l, backend) = listener();
    // Another test can take a port between picking and binding it.
    let (d, dns, metrics) = (0..5)
        .find_map(|_| {
            let dns = format!("127.0.0.1:{}", free_port());
            let metrics = format!("127.0.0.1:{}", free_port());
            let mut d = Daemon::start(&format!(
                "[dns]\nlisten = [\"{dns}\"]\n\n[metrics]\nlisten = [\"{metrics}\"]\n\n{}",
                tcp_service("api.svc.emfor", &backend)
            ));
            std::thread::sleep(Duration::from_millis(200));
            d.running().then_some((d, dns, metrics))
        })
        .expect("no free ports after 5 tries");
    let r = Reader::open(&d.store());
    // Exit status 0 needs a healthy backend.
    wait_for(|| {
        r.service("api.svc.emfor")
            .is_some_and(|s| s.backends[0].state == HealthState::Healthy)
    });
    assert_eq!(r.get("meta/dns_listen").unwrap(), format!("[\"{dns}\"]"));
    assert_eq!(
        r.get("meta/metrics_listen").unwrap(),
        format!("[\"{metrics}\"]")
    );
    let (code, out, err) = status(&d.config(), &[]);
    assert_eq!(code, 0, "{out}{err}");
    let line = out.lines().nth(1).unwrap();
    assert!(line.starts_with("daemon: running, "), "{out}");
    assert!(
        line.ends_with(&format!(", dns {dns}, metrics {metrics}")),
        "{out}"
    );
    let (code, out, _) = status(&d.config(), &["--json"]);
    assert_eq!(code, 0, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["dns_listen"], serde_json::json!([dns]));
    assert_eq!(v["metrics_listen"], serde_json::json!([metrics]));
    let (code, _) = d.stop();
    assert_eq!(code, 0);
}

// spectra-scenario: status-command / Store missing
// spectra-scenario: status-command / Exit statuses
// spectra-scenario: status-command / Exit statuses / exit status table
#[test]
fn status_without_a_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("none.mdb");
    let cfg = dir.path().join("emfor.toml");
    write_main_file(&cfg, &format!("[store]\npath = \"{}\"\n", store.display()));
    let (code, _, err) = status(&cfg, &[]);
    assert_eq!(code, 2);
    assert_eq!(
        err.trim(),
        format!("cannot open store {}: no such file", store.display())
    );
    assert!(!store.exists());
    assert!(!dir.path().join("none.mdb-lock").exists());
}

#[test]
fn status_refuses_another_format_version() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("state.mdb");
    {
        let env = unsafe {
            let mut o = EnvOpenOptions::new().read_txn_without_tls();
            o.flags(EnvFlags::NO_SUB_DIR);
            o.open(&store).unwrap()
        };
        let db: Database<Str, Str> = env
            .create_databases_and_commit(env.write_txn().unwrap(), |o, w| -> heed::Result<_> {
                Ok(CommitOrAbort::<_, ()>::Commit(
                    o.create_database::<Str, Str>(w, None)?,
                ))
            })
            .unwrap()
            .unwrap_commit();
        let mut wtxn = env.write_txn().unwrap();
        db.put(&mut wtxn, "meta/schema_version", "3").unwrap();
        wtxn.commit().unwrap();
    }
    let cfg = dir.path().join("emfor.toml");
    write_main_file(&cfg, &format!("[store]\npath = \"{}\"\n", store.display()));
    let (code, _, err) = status(&cfg, &[]);
    assert_eq!(code, 2);
    assert_eq!(
        err.trim(),
        "store format version 3; this emfor reads versions 1 and 2"
    );
}

// spectra-scenario: status-command / Watch without a terminal
#[test]
fn status_watch_needs_a_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    write_main_file(&cfg, "");
    let (code, _, err) = status(&cfg, &["--watch"]);
    assert_eq!((code, err.trim()), (2, "--watch needs a terminal"));
}

/// How a test ends `status --watch`.
enum WatchStop {
    Signal(Signal),
    Keys(&'static [u8]),
}

/// Run `status --watch` on a new pseudo-terminal (its standard input and
/// output), end it with `stop`, and return its exit status, everything it
/// wrote, and the terminal's `c_lflag` afterwards.
fn watch_on_a_pty(cfg: &Path, stop: &WatchStop) -> (Option<i32>, Vec<u8>, libc::tcflag_t) {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::{Arc, Mutex};
    let (mut master, mut slave) = (0, 0);
    // openpty's descriptors are not close-on-exec: mark them before another
    // test starts a process that would inherit them.
    // SAFETY: openpty fills two new descriptors, owned below; fcntl only
    // changes their flags.
    let rc = sockets(|| unsafe {
        let rc = libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        if rc == 0 {
            libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(slave, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        rc
    });
    assert_eq!(rc, 0);
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let mut child = Command::new(cargo_bin("emfor"))
        .arg("status")
        .arg("--config")
        .arg(cfg)
        .arg("--watch")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::null())
        .spawn_locked()
        .unwrap();
    let mut input = std::fs::File::from(master.try_clone().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reader = {
        let seen = seen.clone();
        let mut master = std::fs::File::from(master);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = master.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        })
    };
    let has = |s: &[u8]| {
        let seen = seen.lock().unwrap();
        seen.windows(s.len()).any(|w| w == s)
    };
    wait_for(|| has(b"store: "));
    match stop {
        WatchStop::Signal(sig) => kill(Pid::from_raw(child.id() as i32), *sig).unwrap(),
        WatchStop::Keys(keys) => input.write_all(keys).unwrap(),
    }
    let status = child.wait().unwrap();
    // SAFETY: tcgetattr only writes into `t`; `slave` is an open pty.
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut t) }, 0);
    // The reader ends once every slave descriptor is closed.
    drop(slave);
    drop(input);
    reader.join().unwrap();
    let out = seen.lock().unwrap().clone();
    (status.code(), out, t.c_lflag)
}

/// Spec status-command "Status watch mode", scenarios "Ending with a key"
/// and "Ending with a signal": `status --watch` gives the terminal back
/// (line wrapping, cursor, main screen, cooked mode) when it is ended by
/// `q`, Ctrl-C, `kill` or a closed ssh session.
// spectra-scenario: status-command / Ending with a key
// spectra-scenario: status-command / Ending with a signal
#[test]
fn status_watch_restores_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    write_main_file(&cfg, &store_header(dir.path()));
    for stop in [
        WatchStop::Signal(Signal::SIGTERM),
        WatchStop::Signal(Signal::SIGHUP),
        WatchStop::Signal(Signal::SIGINT),
        WatchStop::Keys(b"q"),
        WatchStop::Keys(b"\x03"),
    ] {
        let what = match &stop {
            WatchStop::Signal(sig) => sig.to_string(),
            WatchStop::Keys(k) => format!("{k:?}"),
        };
        let (code, out, lflag) = watch_on_a_pty(&cfg, &stop);
        assert_eq!(code, Some(0), "{what}");
        assert!(
            out.ends_with(b"\x1b[?7h\x1b[?25h\x1b[?1049l"),
            "{what}: {:?}",
            String::from_utf8_lossy(&out[out.len().saturating_sub(40)..])
        );
        for flag in [libc::ICANON, libc::ECHO, libc::ISIG] {
            assert_ne!(lflag & flag, 0, "{what}: lflag {lflag:#x} lacks {flag:#x}");
        }
    }
}

/// Spec status-command "Status history output", scenario "History with
/// watch refused".
// spectra-scenario: status-command / History with watch refused
#[test]
fn status_history_refuses_watch_and_json() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("emfor.toml");
    write_main_file(&cfg, "");
    for other in ["--watch", "--json"] {
        let (code, _, err) = status(&cfg, &["--history", other]);
        assert_eq!(code, 2, "{err}");
        assert!(err.contains("cannot be used with"), "{err}");
    }
}

/// Spec state-store "Backend state history", scenario "History kept across
/// a restart", and status-command "Status history output".
// spectra-scenario: state-store / History kept across a restart
// spectra-scenario: status-command / History of one service
#[test]
fn history_kept_across_a_restart() {
    let (l, addr) = listener();
    let mut d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    let r = Reader::open(&d.store());
    let state = || r.service("api.svc.emfor").map(|s| s.backends[0].state);
    wait_for(|| state() == Some(HealthState::Healthy));
    drop(l);
    wait_for(|| state() == Some(HealthState::Unhealthy));
    let before: ServiceHistory =
        serde_json::from_str(&r.get("hist/api.svc.emfor").unwrap()).unwrap();
    let transitions: Vec<_> = before.backends[&addr]
        .iter()
        .map(|e| (e.from, e.to))
        .collect();
    assert_eq!(
        transitions,
        [
            (HealthState::Unknown, HealthState::Healthy),
            (HealthState::Healthy, HealthState::Unhealthy)
        ]
    );
    drop(r);

    // Restart on the same directory.
    d.signal(Signal::SIGTERM);
    assert!(d.child.wait().unwrap().success());
    let dir = std::mem::replace(&mut d.dir, tempfile::tempdir().unwrap());
    let services = std::fs::read_to_string(dir.path().join("emfor.toml"))
        .unwrap()
        .split_once("\n\n")
        .unwrap()
        .1
        .to_owned();
    let d = Daemon::start_in(dir, &services, &[]);
    let r = Reader::open(&d.store());
    wait_for(|| r.get("meta/heartbeat").is_some_and(|h| h != "0"));
    let after: ServiceHistory =
        serde_json::from_str(&r.get("hist/api.svc.emfor").unwrap()).unwrap();
    assert_eq!(after.backends[&addr][..2], before.backends[&addr][..]);

    let (code, out, err) = status(&d.config(), &["--history"]);
    assert_eq!(code, 1, "backend down: {out}{err}");
    let lines: Vec<&str> = out.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == format!("  {addr}"))
        .unwrap_or_else(|| panic!("{out}"));
    // Newest first; the restarted daemon may already have added
    // `unknown -> unhealthy` above the earlier changes.
    let block: Vec<&str> = lines[at + 1..]
        .iter()
        .take_while(|l| l.starts_with("    "))
        .copied()
        .collect();
    let down = block
        .iter()
        .position(|l| l.contains(" healthy -> unhealthy  "));
    let up = block
        .iter()
        .position(|l| l.ends_with(" unknown -> healthy"));
    assert!(matches!((down, up), (Some(d), Some(u)) if d < u), "{out}");

    let (_, out, _) = status(&d.config(), &["--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v["history"]["api.svc.emfor"]["backends"][&addr].is_array());
    assert_eq!(v["flapping"], serde_json::json!([]));
}

/// Replace the config of `d` by a named pipe that nobody has open for
/// writing, so that every read of the config blocks in `open` (what a file
/// on a hung file system does). Returns the pipe's path.
fn config_that_hangs(d: &Daemon) -> PathBuf {
    let path = d.config();
    std::fs::remove_file(&path).unwrap();
    nix::unistd::mkfifo(&path, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
    path
}

/// Spec service-config "Stop request during a reload that cannot read the
/// config": the read of the config does not return, and `SIGTERM` still ends
/// the daemon.
// spectra-scenario: service-config / Stop request during a reload that cannot read the config
#[test]
fn stop_request_ends_a_reload_that_cannot_read_the_config() {
    let (_l, addr) = listener();
    let mut d = Daemon::start(&tcp_service("api.svc.emfor", &addr));
    config_that_hangs(&d);
    d.signal(Signal::SIGHUP);
    // The reload is now blocked in the read; give it time to get there.
    std::thread::sleep(Duration::from_millis(500));
    d.signal(Signal::SIGTERM);
    let deadline = Instant::now() + Duration::from_secs(5);
    while d.running() {
        assert!(
            Instant::now() < deadline,
            "the daemon is still running 5s after SIGTERM; log:\n{}",
            d.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let (code, log) = d.wait();
    assert_eq!(code, 0, "{log}");
    assert!(
        log.contains("stop requested while reloading; the reload is abandoned"),
        "{log}"
    );
}

/// Spec service-config "SIGHUP during a reload": three signals while a
/// reload is blocked cause exactly one more reload after it.
// spectra-scenario: service-config / SIGHUP during a reload
#[test]
fn sighups_during_a_reload_cause_one_more_reload() {
    use std::os::unix::fs::OpenOptionsExt;
    let (_l, addr) = listener();
    let body = tcp_service("api.svc.emfor", &addr);
    let mut d = Daemon::start(&body);
    let whole = std::fs::read_to_string(d.config()).unwrap();
    let fifo = config_that_hangs(&d);
    d.signal(Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(500));
    d.signal(Signal::SIGHUP);
    d.signal(Signal::SIGHUP);
    std::thread::sleep(Duration::from_millis(200));
    // Let the blocked reload read the config; then the one more reload
    // starts and blocks on the pipe again.
    let feed = |text: &str| {
        let mut w = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        w.write_all(text.as_bytes()).unwrap();
    };
    feed(&whole);
    d.wait_log("config reloaded");
    feed(&whole);
    wait_for(|| d.log().matches("config reloaded").count() == 2);
    // A third reload would be waiting to read the pipe: a non-blocking open
    // for writing then succeeds. With no reader it fails with ENXIO.
    std::thread::sleep(Duration::from_millis(500));
    let third = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(&fifo);
    assert_eq!(
        third.err().and_then(|e| e.raw_os_error()),
        Some(nix::libc::ENXIO),
        "a third reload is reading the config; log:\n{}",
        d.log()
    );
    assert_eq!(d.log().matches("config reloaded").count(), 2, "{}", d.log());
    assert!(d.running());
}
