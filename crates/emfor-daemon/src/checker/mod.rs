//! Checker interface, result kinds, and the checker registry.

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

/// Result of one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    /// The backend passed.
    Up,
    /// The backend failed; the string says why.
    Down(String),
    /// emfor could not perform the check because of a local problem.
    /// Not counted as a backend failure.
    Error(String),
}

impl CheckResult {
    /// Reason text for `Down` / `Error`, `None` for `Up`.
    pub fn reason(&self) -> Option<&str> {
        match self {
            CheckResult::Up => None,
            CheckResult::Down(r) | CheckResult::Error(r) => Some(r),
        }
    }

    /// The same result with its reason passed through [`normalize_reason`].
    pub fn normalized(self) -> CheckResult {
        match self {
            CheckResult::Up => CheckResult::Up,
            CheckResult::Down(r) => CheckResult::Down(normalize_reason(&r)),
            CheckResult::Error(r) => CheckResult::Error(normalize_reason(&r)),
        }
    }
}

/// Longest reason kept, in characters.
pub const REASON_MAX_CHARS: usize = 512;

/// Make a reason safe to store and print, whatever produced it (a backend's
/// answer, a plugin, a script): tab, CR and LF become `\t`, `\r`, `\n`;
/// other control characters (Unicode Cc) become `\u{1b}` and the like;
/// then anything past [`REASON_MAX_CHARS`] is cut and replaced by `…`.
/// Backslashes stay as they are, so an excerpt that is already escaped
/// (the tcp checker's `expect: got "..."`) is not escaped twice.
pub fn normalize_reason(reason: &str) -> String {
    let mut out = String::with_capacity(reason.len());
    for c in reason.chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    if out.chars().count() > REASON_MAX_CHARS {
        out = out.chars().take(REASON_MAX_CHARS - 1).collect();
        out.push('…');
    }
    out
}

/// One check of one backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRequest {
    /// Service name.
    pub service: Arc<str>,
    /// Backend address.
    pub target: SocketAddr,
    /// The service's `timeout`; [`run_check`] enforces it.
    pub timeout: Duration,
}

impl CheckRequest {
    pub fn new(service: impl Into<Arc<str>>, target: SocketAddr, timeout: Duration) -> Self {
        Self {
            service: service.into(),
            target,
            timeout,
        }
    }
}

/// A configured checker that can check one backend address.
///
/// The same checker is called concurrently for every backend of a service.
/// Implementations do not need to enforce the timeout: [`run_check`] does.
pub trait Checker: Send + Sync + fmt::Debug {
    fn check(&self, req: CheckRequest) -> BoxFuture<'_, CheckResult>;

    /// Result recorded when a check does not finish within its timeout.
    fn timeout_result(&self) -> CheckResult {
        CheckResult::Down("timeout".into())
    }

    /// Set up per-backend resources (for example an HTTP client) ahead of a
    /// check, so one-time setup is not counted in the check's latency.
    /// Called before every check; must be cheap once the resources exist.
    /// Errors are left for the check itself to report.
    fn prepare(&self, _target: SocketAddr) -> BoxFuture<'_, ()> {
        // A zero-sized future: boxing it does not allocate.
        Box::pin(std::future::ready(()))
    }

    /// Release per-backend resources. Called when a reload removes `target`
    /// from a service whose checker is kept.
    fn forget(&self, _target: SocketAddr) {}
}

/// A validation problem inside `check_options`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionError {
    /// Key path relative to `check_options`, e.g. `"expect_status"`.
    pub key: String,
    pub message: String,
}

impl OptionError {
    pub fn new(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            message: message.into(),
        }
    }
}

/// Builds checkers of one kind from their `check_options`.
pub trait CheckerFactory: Send + Sync {
    /// The `check` value that selects this factory, e.g. `"tcp"`.
    fn kind(&self) -> &str;
    /// Identity of the checker implementation; a service whose checker id
    /// changes on reload starts over. Built-ins use their `kind`.
    fn checker_id(&self) -> String {
        self.kind().to_string()
    }
    /// Validate `options` and build a checker. A config that validates is
    /// guaranteed to be runnable.
    fn validate(&self, options: &toml::Table) -> Result<Arc<dyn Checker>, Vec<OptionError>>;
}

/// Maps `check` names to factories.
#[derive(Clone, Default)]
pub struct Registry {
    factories: HashMap<String, Arc<dyn CheckerFactory>>,
}

impl Registry {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Registry with the built-in `tcp`, `http`, and `exec` checkers.
    pub fn builtin() -> Self {
        let mut r = Self::empty();
        r.register(Arc::new(tcp::TcpFactory));
        r.register(Arc::new(http::HttpFactory));
        r.register(Arc::new(exec::ExecFactory));
        r
    }

    pub fn register(&mut self, factory: Arc<dyn CheckerFactory>) {
        self.factories.insert(factory.kind().to_string(), factory);
    }

    pub fn get(&self, kind: &str) -> Option<&Arc<dyn CheckerFactory>> {
        self.factories.get(kind)
    }
}

/// Run one check with a hard timeout. A check that has not finished within
/// `req.timeout` gets the checker's [`Checker::timeout_result`]
/// (`Down("timeout")` unless the checker says otherwise).
pub async fn run_check(checker: &dyn Checker, req: CheckRequest) -> CheckResult {
    let timeout = req.timeout;
    let result = match tokio::time::timeout(timeout, checker.check(req)).await {
        Ok(result) => result,
        Err(_) => checker.timeout_result(),
    };
    // Every reason reaches the state, the store, the logs and the metrics
    // through here.
    result.normalized()
}

/// Reject any key in `options` not listed in `allowed`.
pub(crate) fn reject_unknown_keys(
    options: &toml::Table,
    allowed: &[&str],
    errors: &mut Vec<OptionError>,
) {
    for key in options.keys() {
        if !allowed.contains(&key.as_str()) {
            errors.push(OptionError::new(key.clone(), "unknown key"));
        }
    }
}

pub mod exec;
pub mod http;
pub mod tcp;

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec "Reasons are bounded and printable", example table.
    #[test]
    fn reasons_are_normalized() {
        let a = |n| "a".repeat(n);
        for (input, want) in [
            (
                "connection refused".to_string(),
                "connection refused".to_string(),
            ),
            ("a\tb".into(), "a\\tb".into()),
            ("x\u{1b}y".into(), "x\\u{1b}y".into()),
            ("x\u{85}y".into(), "x\\u{85}y".into()),
            (a(600), format!("{}…", a(511))),
            (a(512), a(512)),
            ("r\rn\n".into(), "r\\rn\\n".into()),
            ("del\u{7f}".into(), "del\\u{7f}".into()),
            ("got \"\\x00\"".into(), "got \"\\x00\"".into()),
            ("é ok 日本".into(), "é ok 日本".into()),
        ] {
            assert_eq!(normalize_reason(&input), want, "{input:?}");
        }
        // Escaping can push a reason over the limit.
        let r = normalize_reason(&"\u{1}".repeat(200));
        assert_eq!(r.chars().count(), REASON_MAX_CHARS);
        assert!(r.ends_with('…'));
    }

    #[tokio::test]
    async fn run_check_normalizes_the_reason() {
        #[derive(Debug)]
        struct Says(&'static str);
        impl Checker for Says {
            fn check(&self, _: CheckRequest) -> BoxFuture<'_, CheckResult> {
                Box::pin(async move { CheckResult::Down(self.0.into()) })
            }
        }
        let r = run_check(&Says("bad\u{1b}[2Jstate"), req(Duration::from_secs(1))).await;
        assert_eq!(r, CheckResult::Down("bad\\u{1b}[2Jstate".into()));
    }

    #[derive(Debug)]
    struct Sleepy(Duration);

    impl Checker for Sleepy {
        fn check(&self, _: CheckRequest) -> BoxFuture<'_, CheckResult> {
            Box::pin(async move {
                tokio::time::sleep(self.0).await;
                CheckResult::Up
            })
        }
    }

    /// Never finishes; times out as `error`.
    #[derive(Debug)]
    struct Stuck;

    impl Checker for Stuck {
        fn check(&self, _: CheckRequest) -> BoxFuture<'_, CheckResult> {
            Box::pin(futures::future::pending())
        }

        fn timeout_result(&self) -> CheckResult {
            CheckResult::Error("did not answer".into())
        }
    }

    fn req(timeout: Duration) -> CheckRequest {
        CheckRequest::new("api.svc.emfor", "127.0.0.1:1".parse().unwrap(), timeout)
    }

    #[tokio::test(start_paused = true)]
    async fn slow_check_is_down_timeout() {
        let r = run_check(&Sleepy(Duration::from_secs(5)), req(Duration::from_secs(1))).await;
        assert_eq!(r, CheckResult::Down("timeout".into()));
    }

    #[tokio::test(start_paused = true)]
    async fn fast_check_keeps_result() {
        let r = run_check(
            &Sleepy(Duration::from_millis(10)),
            req(Duration::from_secs(1)),
        )
        .await;
        assert_eq!(r, CheckResult::Up);
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_uses_checker_timeout_result() {
        let r = run_check(&Stuck, req(Duration::from_secs(1))).await;
        assert_eq!(r, CheckResult::Error("did not answer".into()));
    }

    #[test]
    fn unknown_kind_is_not_registered() {
        let r = Registry::builtin();
        assert!(r.get("redis").is_none());
        assert!(r.get("tcp").is_some());
        assert!(r.get("http").is_some());
        assert!(r.get("exec").is_some());
    }
}
