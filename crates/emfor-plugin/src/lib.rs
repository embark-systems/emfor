//! Write emfor checker plugins in Rust.
//!
//! A plugin is a long-running child process of the emfor daemon that speaks
//! protocol 1 (docs/plugins/protocol-v1.md): one JSON object per line, the
//! daemon's requests on stdin and the plugin's answers on stdout. This crate
//! handles the protocol; a plugin implements [`Plugin`] and calls [`run`].
//!
//! ```no_run
//! use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options, Plugin};
//!
//! struct Always;
//!
//! impl Plugin for Always {
//!     fn name(&self) -> &str { "always" }
//!     fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
//!     fn validate(&self, _options: &Options) -> Vec<OptionError> { Vec::new() }
//!     async fn check(&self, _req: CheckRequest) -> CheckOutcome { CheckOutcome::Up }
//! }
//!
//! fn main() -> std::process::ExitCode {
//!     emfor_plugin::run(Always)
//! }
//! ```

use std::future::Future;
use std::net::IpAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

/// The protocol version this crate speaks.
pub const PROTOCOL: u32 = 1;

/// A service's `check_options`, converted from TOML to JSON by the daemon.
pub type Options = serde_json::Map<String, serde_json::Value>;

/// One problem in `check_options`; `key` is relative to `check_options`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionError {
    pub key: String,
    pub message: String,
}

impl OptionError {
    pub fn new(key: impl Into<String>, message: impl Into<String>) -> Self {
        OptionError {
            key: key.into(),
            message: message.into(),
        }
    }
}

/// One check to run.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckRequest {
    pub service: String,
    pub ip: IpAddr,
    pub port: u16,
    pub options: Options,
    /// Time the plugin has to answer (`timeout_ms`). [`run`] answers
    /// `down` with reason `timeout` when [`Plugin::check`] takes longer.
    pub timeout: Duration,
}

/// The result of one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// The backend is healthy.
    Up,
    /// The backend failed the check.
    Down(String),
    /// The plugin could not check the backend; not a backend failure.
    Error(String),
}

/// A checker plugin.
pub trait Plugin: Send + Sync + 'static {
    /// Name sent in the handshake.
    fn name(&self) -> &str;
    /// Version sent in the handshake.
    fn version(&self) -> &str;
    /// Check a service's `check_options`; an empty list means valid.
    fn validate(&self, options: &Options) -> Vec<OptionError>;
    /// Run one check. Only called with options that passed [`Plugin::validate`].
    /// [`run`] answers a check that panics `error` with reason
    /// `check panicked: <message>`.
    fn check(&self, req: CheckRequest) -> impl Future<Output = CheckOutcome> + Send;
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Request {
    Hello {
        protocol: u32,
    },
    Validate {
        id: u64,
        #[serde(default)]
        options: Options,
    },
    Check {
        id: u64,
        #[serde(default)]
        service: String,
        target: Target,
        #[serde(default)]
        options: Options,
        timeout_ms: u64,
    },
}

#[derive(Debug, Deserialize)]
struct Target {
    ip: String,
    port: u16,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Response<'a> {
    Hello {
        protocol: u32,
        name: &'a str,
        version: &'a str,
    },
    Validate {
        id: u64,
        errors: Vec<OptionError>,
    },
    Check {
        id: u64,
        result: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

/// Why [`serve`] stopped before its input ended.
#[derive(Debug)]
pub enum ServeError {
    /// Reading the input or writing the output failed.
    Io(std::io::Error),
    /// The daemon asked for another protocol version.
    Protocol(u32),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::Io(e) => write!(f, "i/o error: {e}"),
            ServeError::Protocol(p) => {
                write!(
                    f,
                    "daemon speaks protocol {p}, this plugin speaks {PROTOCOL}"
                )
            }
        }
    }
}

impl std::error::Error for ServeError {}

/// Serve the plugin on stdin and stdout until stdin ends.
///
/// Returns success when stdin ends normally, and failure after an I/O error
/// or a handshake for another protocol version. Messages about ignored lines
/// and failures go to stderr, which the daemon logs.
pub fn run<P: Plugin>(plugin: P) -> ExitCode {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("{}: cannot start runtime: {e}", plugin.name());
            return ExitCode::FAILURE;
        }
    };
    let plugin = Arc::new(plugin);
    let name = plugin.name().to_owned();
    let input = tokio::io::BufReader::new(tokio::io::stdin());
    match rt.block_on(serve(plugin, input, tokio::io::stdout())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{name}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Serve the plugin on any input and output; [`run`] uses stdin and stdout.
///
/// Each check runs in its own task, so answers can go out in any order.
/// When the input ends, `serve` waits for running checks, writes their
/// answers and returns.
pub async fn serve<P, R, W>(plugin: Arc<P>, input: R, mut output: W) -> Result<(), ServeError>
where
    P: Plugin,
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            output.write_all(line.as_bytes()).await?;
            output.flush().await?;
        }
        Ok::<(), std::io::Error>(())
    });

    let result = read_requests(&plugin, input, &tx).await;
    drop(tx);
    let written = writer
        .await
        .unwrap_or_else(|e| Err(std::io::Error::other(e)));
    result?;
    written.map_err(ServeError::Io)
}

async fn read_requests<P: Plugin, R: AsyncBufRead + Unpin>(
    plugin: &Arc<P>,
    input: R,
    tx: &mpsc::UnboundedSender<String>,
) -> Result<(), ServeError> {
    let mut checks = JoinSet::new();
    let mut lines = input.lines();
    let outcome = loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break Ok(()),
            Err(e) => break Err(ServeError::Io(e)),
        };
        // Reap finished checks so the set does not grow without bound.
        while checks.try_join_next().is_some() {}
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{}: ignoring request line: {e}", plugin.name());
                continue;
            }
        };
        match req {
            Request::Hello { protocol } => {
                send(
                    tx,
                    &Response::Hello {
                        protocol: PROTOCOL,
                        name: plugin.name(),
                        version: plugin.version(),
                    },
                );
                if protocol != PROTOCOL {
                    break Err(ServeError::Protocol(protocol));
                }
            }
            Request::Validate { id, options } => {
                let errors = plugin.validate(&options);
                send(tx, &Response::Validate { id, errors });
            }
            Request::Check {
                id,
                service,
                target,
                options,
                timeout_ms,
            } => {
                let Ok(ip) = target.ip.parse::<IpAddr>() else {
                    send(
                        tx,
                        &check_response(
                            id,
                            CheckOutcome::Error(format!("invalid target ip {:?}", target.ip)),
                        ),
                    );
                    continue;
                };
                let timeout = Duration::from_millis(timeout_ms);
                let req = CheckRequest {
                    service,
                    ip,
                    port: target.port,
                    options,
                    timeout,
                };
                let plugin = Arc::clone(plugin);
                let tx = tx.clone();
                checks.spawn(async move {
                    // The check runs in a task of its own so that a panic
                    // in it still gets this request an answer.
                    let mut task = tokio::spawn(async move { plugin.check(req).await });
                    let outcome = match tokio::time::timeout(timeout, &mut task).await {
                        Ok(Ok(outcome)) => outcome,
                        Ok(Err(e)) => CheckOutcome::Error(task_failure(e)),
                        Err(_) => {
                            task.abort();
                            CheckOutcome::Down("timeout".into())
                        }
                    };
                    send(&tx, &check_response(id, outcome));
                });
            }
        }
    };
    while checks.join_next().await.is_some() {}
    outcome
}

/// The `error` reason for a check task that ended without an outcome:
/// `check panicked`, followed by the panic message when it is text.
fn task_failure(e: tokio::task::JoinError) -> String {
    if !e.is_panic() {
        return "check was cancelled".into();
    }
    let payload = e.into_panic();
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
    match message {
        Some(m) => format!("check panicked: {m}"),
        None => "check panicked".into(),
    }
}

/// Characters of a reason sent to emfor; the daemon keeps no more than this
/// (spec health-checking, reason normalization), so the result is the same.
pub const REASON_MAX_CHARS: usize = 512;

/// `reason` cut to its first [`REASON_MAX_CHARS`] characters, followed by
/// `…` when it was longer. A backend's error text can be any length, and a
/// line over the daemon's 65536-byte limit would get the plugin restarted.
fn cap_reason(reason: String) -> String {
    match reason.char_indices().nth(REASON_MAX_CHARS) {
        None => reason,
        Some((cut, _)) => {
            let mut short = reason;
            short.truncate(cut);
            short.push('…');
            short
        }
    }
}

fn check_response(id: u64, outcome: CheckOutcome) -> Response<'static> {
    let (result, reason) = match outcome {
        CheckOutcome::Up => ("up", None),
        CheckOutcome::Down(r) => ("down", Some(cap_reason(r))),
        CheckOutcome::Error(r) => ("error", Some(cap_reason(r))),
    };
    Response::Check { id, result, reason }
}

fn send(tx: &mpsc::UnboundedSender<String>, response: &Response<'_>) {
    let mut line = serde_json::to_string(response)
        .expect("a Response serializes: it holds only strings, numbers and lists");
    line.push('\n');
    // The receiver lives until every sender is gone.
    let _ = tx.send(line);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, BufReader};

    /// Test plugin: `options.sleep_ms` delays the answer; `options.result`
    /// picks it (`up` by default, `panic` panics); `options.bad` fails
    /// validation.
    struct Fake;

    impl Plugin for Fake {
        fn name(&self) -> &str {
            "fake"
        }
        fn version(&self) -> &str {
            "9.9.9"
        }
        fn validate(&self, options: &Options) -> Vec<OptionError> {
            if options.contains_key("bad") {
                vec![OptionError::new("bad", "unknown key")]
            } else {
                Vec::new()
            }
        }
        async fn check(&self, req: CheckRequest) -> CheckOutcome {
            if let Some(ms) = req.options.get("sleep_ms").and_then(Value::as_u64) {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            match req.options.get("result").and_then(Value::as_str) {
                Some("down") => CheckOutcome::Down(format!("{}:{} down", req.ip, req.port)),
                Some("error") => CheckOutcome::Error("broken".into()),
                Some("panic") => panic!("boom"),
                _ => CheckOutcome::Up,
            }
        }
    }

    /// Feed `input` to `serve` and return its answers and result.
    async fn exchange(input: &str) -> (Vec<Value>, Result<(), ServeError>) {
        let (out_w, mut out_r) = tokio::io::duplex(1 << 16);
        let result = serve(Arc::new(Fake), BufReader::new(input.as_bytes()), out_w).await;
        let mut text = String::new();
        out_r.read_to_string(&mut text).await.unwrap();
        let answers = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        (answers, result)
    }

    #[test]
    fn long_reasons_are_cut() {
        let reason = |o| match check_response(7, o) {
            Response::Check { reason, .. } => reason.expect("down has a reason"),
            _ => unreachable!("check_response answers a check"),
        };
        // A backend's error text far past the daemon's line limit.
        let long = "é".repeat(70_000);
        let cut = reason(CheckOutcome::Down(long.clone()));
        assert_eq!(cut.chars().count(), REASON_MAX_CHARS + 1);
        assert!(cut.starts_with(&"é".repeat(REASON_MAX_CHARS)));
        assert!(cut.ends_with('…'));
        let line = serde_json::to_string(&check_response(7, CheckOutcome::Error(long))).unwrap();
        assert!(line.len() < 65536, "line is {} bytes", line.len());
        // Up to the limit nothing changes.
        let exact = "x".repeat(REASON_MAX_CHARS);
        assert_eq!(reason(CheckOutcome::Down(exact.clone())), exact);
        assert_eq!(reason(CheckOutcome::Down("timeout".into())), "timeout");
    }

    #[tokio::test]
    async fn hello_and_validate() {
        let (answers, result) = exchange(concat!(
            r#"{"type":"hello","protocol":1}"#,
            "\n",
            r#"{"type":"validate","id":1,"options":{}}"#,
            "\n",
            r#"{"type":"validate","id":2,"options":{"bad":true}}"#,
            "\n",
        ))
        .await;
        result.unwrap();
        assert_eq!(
            answers,
            vec![
                json!({"type":"hello","protocol":1,"name":"fake","version":"9.9.9"}),
                json!({"type":"validate","id":1,"errors":[]}),
                json!({"type":"validate","id":2,"errors":[{"key":"bad","message":"unknown key"}]}),
            ]
        );
    }

    #[tokio::test]
    async fn other_protocol_is_answered_then_stops() {
        let (answers, result) = exchange(concat!(
            r#"{"type":"hello","protocol":2}"#,
            "\n",
            r#"{"type":"validate","id":1,"options":{}}"#,
            "\n",
        ))
        .await;
        assert!(matches!(result, Err(ServeError::Protocol(2))), "{result:?}");
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0]["protocol"], 1);
    }

    #[tokio::test]
    async fn check_results() {
        let (answers, result) = exchange(concat!(
            r#"{"type":"check","id":3,"service":"a","target":{"ip":"10.0.0.1","port":80},"options":{},"timeout_ms":900}"#,
            "\n",
        ))
        .await;
        result.unwrap();
        assert_eq!(answers, vec![json!({"type":"check","id":3,"result":"up"})]);

        let (answers, _) = exchange(concat!(
            r#"{"type":"check","id":4,"service":"a","target":{"ip":"::1","port":6379},"options":{"result":"down"},"timeout_ms":900}"#,
            "\n",
            r#"{"type":"check","id":5,"service":"a","target":{"ip":"10.0.0.1","port":1},"options":{"result":"error"},"timeout_ms":900}"#,
            "\n",
            r#"{"type":"check","id":6,"service":"a","target":{"ip":"nope","port":1},"options":{},"timeout_ms":900}"#,
            "\n",
        ))
        .await;
        let by_id = |id: u64| answers.iter().find(|a| a["id"] == id).unwrap().clone();
        assert_eq!(
            by_id(4),
            json!({"type":"check","id":4,"result":"down","reason":"::1:6379 down"})
        );
        assert_eq!(
            by_id(5),
            json!({"type":"check","id":5,"result":"error","reason":"broken"})
        );
        assert_eq!(by_id(6)["result"], "error");
    }

    #[tokio::test]
    async fn answers_come_out_of_order() {
        let (answers, result) = exchange(concat!(
            r#"{"type":"check","id":7,"service":"a","target":{"ip":"10.0.0.1","port":80},"options":{"sleep_ms":300},"timeout_ms":900}"#,
            "\n",
            r#"{"type":"check","id":8,"service":"a","target":{"ip":"10.0.0.2","port":80},"options":{},"timeout_ms":900}"#,
            "\n",
        ))
        .await;
        result.unwrap();
        let ids: Vec<u64> = answers.iter().map(|a| a["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, vec![8, 7]);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_check_is_down_timeout() {
        let (answers, result) = exchange(concat!(
            r#"{"type":"check","id":9,"service":"a","target":{"ip":"10.0.0.1","port":80},"options":{"sleep_ms":5000},"timeout_ms":900}"#,
            "\n",
        ))
        .await;
        result.unwrap();
        assert_eq!(
            answers,
            vec![json!({"type":"check","id":9,"result":"down","reason":"timeout"})]
        );
    }

    #[tokio::test]
    async fn panicking_check_is_error() {
        let (answers, result) = exchange(concat!(
            r#"{"type":"check","id":10,"service":"a","target":{"ip":"10.0.0.1","port":80},"options":{"result":"panic"},"timeout_ms":900}"#,
            "\n",
            r#"{"type":"check","id":11,"service":"a","target":{"ip":"10.0.0.1","port":80},"options":{},"timeout_ms":900}"#,
            "\n",
        ))
        .await;
        result.unwrap();
        let by_id = |id: u64| answers.iter().find(|a| a["id"] == id).cloned();
        assert_eq!(
            by_id(10),
            Some(json!({"type":"check","id":10,"result":"error","reason":"check panicked: boom"}))
        );
        assert_eq!(by_id(11).map(|a| a["result"].clone()), Some(json!("up")));
    }

    #[tokio::test]
    async fn bad_lines_are_ignored() {
        let (answers, result) = exchange(concat!(
            "not json\n",
            "\n",
            r#"{"type":"shutdown"}"#,
            "\n",
            r#"{"type":"validate","id":1,"options":{}}"#,
            "\n",
        ))
        .await;
        result.unwrap();
        assert_eq!(answers, vec![json!({"type":"validate","id":1,"errors":[]})]);
    }
}
