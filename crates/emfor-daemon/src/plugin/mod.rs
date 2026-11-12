//! External plugins: long-lived child processes that check backends over
//! the line-delimited JSON protocol in [`protocol`].
//!
//! [`PluginFactory`] is the [`CheckerFactory`] for one running plugin. Its
//! `validate` blocks on the plugin's answer, so config parsing that may use
//! it runs on a blocking thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::runtime::Handle;

use crate::checker::{CheckRequest, CheckResult, Checker, CheckerFactory, OptionError};
use host::{CallError, PluginHost};
use protocol::{Request, Response, Target, options_to_json, timeout_ms};

pub mod host;
pub mod protocol;
mod set;

pub use set::{PluginSet, Prepared};

/// How long a plugin has to answer one `validate` request.
pub const VALIDATE_TIMEOUT: Duration = Duration::from_secs(5);

/// Checker factory backed by one running plugin. Build a new one per config
/// load: a validation timeout is remembered for the rest of that load.
pub struct PluginFactory {
    host: Arc<PluginHost>,
    handle: Handle,
    /// Set once a `validate` timed out; later validations fail at once.
    hung: AtomicBool,
    /// `check_options` the applied config already uses with this plugin and
    /// command. They were validated when that config was applied, so they
    /// are accepted while the plugin is down (waiting to restart).
    known: Vec<toml::Table>,
}

impl PluginFactory {
    pub fn new(host: Arc<PluginHost>, handle: Handle) -> Self {
        Self {
            host,
            handle,
            hung: AtomicBool::new(false),
            known: Vec::new(),
        }
    }

    /// Accept `known` options without asking the plugin while it is down.
    pub fn with_known_options(mut self, known: Vec<toml::Table>) -> Self {
        self.known = known;
        self
    }

    fn checker(&self, options: serde_json::Map<String, serde_json::Value>) -> Arc<dyn Checker> {
        Arc::new(PluginChecker {
            host: self.host.clone(),
            options,
        })
    }

    fn timeout_message(&self) -> String {
        format!(
            "plugin {} did not answer validation within {}s",
            self.host.name(),
            VALIDATE_TIMEOUT.as_secs()
        )
    }
}

/// `checker_id` of a plugin: `plugin:<name>:<command as JSON>`.
pub fn plugin_checker_id(name: &str, command: &[String]) -> String {
    let command = serde_json::to_string(command).expect("strings serialize");
    format!("plugin:{name}:{command}")
}

/// Error result for a request that got no answer.
fn call_error(name: &str, e: CallError) -> String {
    match e {
        CallError::Unavailable => format!("plugin {name} unavailable"),
        CallError::Exited => "plugin exited".into(),
    }
}

impl CheckerFactory for PluginFactory {
    fn kind(&self) -> &str {
        self.host.name()
    }

    fn checker_id(&self) -> String {
        plugin_checker_id(self.host.name(), self.host.command())
    }

    /// Blocks for up to [`VALIDATE_TIMEOUT`]; call from a blocking thread.
    fn validate(&self, options: &toml::Table) -> Result<Arc<dyn Checker>, Vec<OptionError>> {
        let fail = |m: String| Err(vec![OptionError::new("", m)]);
        if self.hung.load(Ordering::SeqCst) {
            return fail(self.timeout_message());
        }
        let json = options_to_json(options);
        let request = json.clone();
        let answer = self.handle.block_on(tokio::time::timeout(
            VALIDATE_TIMEOUT,
            self.host.call(|id| Request::Validate {
                id,
                options: request,
            }),
        ));
        match answer {
            Err(_) => {
                self.hung.store(true, Ordering::SeqCst);
                fail(self.timeout_message())
            }
            // Down, or exited while this request was pending.
            Ok(Err(_)) if self.known.contains(options) => Ok(self.checker(json)),
            Ok(Err(e)) => fail(call_error(self.host.name(), e)),
            Ok(Ok(Response::Validate { errors, .. })) if errors.is_empty() => {
                Ok(self.checker(json))
            }
            Ok(Ok(Response::Validate { errors, .. })) => Err(errors
                .into_iter()
                .map(|e| OptionError::new(e.key, e.message))
                .collect()),
            Ok(Ok(_)) => fail("invalid plugin response".into()),
        }
    }
}

/// Checks backends by sending `check` requests to a plugin.
pub struct PluginChecker {
    host: Arc<PluginHost>,
    options: serde_json::Map<String, serde_json::Value>,
}

impl std::fmt::Debug for PluginChecker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginChecker")
            .field("plugin", &self.host.name())
            .finish()
    }
}

/// Turn a plugin's `check` answer into a check result.
pub fn check_result(answer: Response) -> CheckResult {
    let Response::Check { result, reason, .. } = answer else {
        return CheckResult::Error("invalid plugin response".into());
    };
    let reason = || reason.clone().unwrap_or_else(|| "no reason given".into());
    match result.as_str().unwrap_or("") {
        "up" => CheckResult::Up,
        "down" => CheckResult::Down(reason()),
        "error" => CheckResult::Error(reason()),
        _ => CheckResult::Error("invalid plugin response".into()),
    }
}

impl Checker for PluginChecker {
    fn check(&self, req: CheckRequest) -> BoxFuture<'_, CheckResult> {
        Box::pin(async move {
            let answer = self
                .host
                .call(|id| Request::Check {
                    id,
                    service: req.service.to_string(),
                    target: Target {
                        ip: req.target.ip().to_string(),
                        port: req.target.port(),
                    },
                    options: self.options.clone(),
                    timeout_ms: timeout_ms(req.timeout),
                })
                .await;
            match answer {
                Ok(a) => check_result(a),
                Err(e) => CheckResult::Error(call_error(self.host.name(), e)),
            }
        })
    }

    fn timeout_result(&self) -> CheckResult {
        CheckResult::Error("plugin did not answer within timeout".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(result: &str, reason: Option<&str>) -> Response {
        Response::Check {
            id: 3,
            result: result.into(),
            reason: reason.map(str::to_string),
        }
    }

    #[test]
    fn answers_map_to_results() {
        assert_eq!(check_result(answer("up", None)), CheckResult::Up);
        assert_eq!(
            check_result(answer("down", Some("PONG not received"))),
            CheckResult::Down("PONG not received".into())
        );
        assert_eq!(
            check_result(answer("down", None)),
            CheckResult::Down("no reason given".into())
        );
        assert_eq!(
            check_result(answer("error", None)),
            CheckResult::Error("no reason given".into())
        );
        assert_eq!(
            check_result(answer("maybe", Some("x"))),
            CheckResult::Error("invalid plugin response".into())
        );
        assert_eq!(
            check_result(Response::Validate {
                id: 3,
                errors: vec![]
            }),
            CheckResult::Error("invalid plugin response".into())
        );
    }
}
