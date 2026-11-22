//! The log lines of a plugin's stderr. Its own test binary: it installs the
//! global log subscriber, which other tests in a shared process would race.

use std::time::{Duration, Instant};

use emfor_daemon::plugin::host::PluginHost;
use emfor_daemon::plugin::protocol::{Request, Target};

const PLUGIN: &str = env!("CARGO_BIN_EXE_emfor-test-plugin");

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

/// A writer that keeps what the log lines say, for a test.
#[derive(Clone, Default)]
struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Captured {
        self.clone()
    }
}

/// Spec checker-plugins "Last stderr lines of an exiting plugin are logged":
/// ending the tasks of a plugin that exited, or failed its handshake, does
/// not drop what it wrote to stderr just before.
// spectra-scenario: checker-plugins / Last stderr lines of an exiting plugin are logged
#[tokio::test]
async fn last_stderr_lines_of_an_exiting_plugin_are_logged() {
    let log = Captured::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_writer(log.clone())
            .with_ansi(false)
            .finish(),
    )
    .unwrap();
    let host = PluginHost::start("fake", &[PLUGIN.to_string()])
        .await
        .unwrap();
    let first = host.pid().unwrap();
    let _ = host.call(check("crash")).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.pid().is_none_or(|p| p == first) {
        assert!(Instant::now() < deadline, "plugin not restarted");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    host.stop().await;
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let last = text
        .find("plugin fake: test plugin crashing")
        .unwrap_or_else(|| panic!("last stderr line not logged:\n{text}"));
    let restart = text
        .find("restarting in")
        .unwrap_or_else(|| panic!("no restart logged:\n{text}"));
    assert!(last < restart, "logged after the restart line:\n{text}");

    // A plugin that exits before the handshake: what it wrote to stderr is
    // still logged, and the start fails.
    let failing = [
        "/usr/bin/env".to_string(),
        "EMFOR_TEST_PLUGIN_EXIT_AT_START=1".to_string(),
        PLUGIN.to_string(),
    ];
    let err = PluginHost::start("early", &failing).await.unwrap_err();
    assert!(err.contains("exited before the handshake"), "{err}");
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains("plugin early: test plugin exiting at start"),
        "{text}"
    );
}
