//! `tcp` checker: a completed TCP connect is `up`.

use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::net::TcpStream;

use super::{CheckRequest, CheckResult, Checker, CheckerFactory, OptionError, reject_unknown_keys};

pub struct TcpFactory;

impl CheckerFactory for TcpFactory {
    fn kind(&self) -> &str {
        "tcp"
    }

    fn validate(&self, options: &toml::Table) -> Result<Arc<dyn Checker>, Vec<OptionError>> {
        let mut errors = Vec::new();
        reject_unknown_keys(options, &[], &mut errors);
        if errors.is_empty() {
            Ok(Arc::new(TcpChecker))
        } else {
            Err(errors)
        }
    }
}

#[derive(Debug)]
pub struct TcpChecker;

impl Checker for TcpChecker {
    fn check(&self, req: CheckRequest) -> BoxFuture<'_, CheckResult> {
        Box::pin(async move {
            match TcpStream::connect(req.target).await {
                Ok(stream) => {
                    drop(stream);
                    CheckResult::Up
                }
                Err(e) if is_local_io_error(&e) => CheckResult::Error(e.to_string()),
                Err(e) => CheckResult::Down(e.to_string()),
            }
        })
    }
}

/// True when an I/O error is caused by the emfor host itself (out of file
/// descriptors, buffers, memory, or local ports) rather than the backend.
pub(crate) fn is_local_io_error(e: &std::io::Error) -> bool {
    use nix::errno::Errno;
    matches!(
        e.raw_os_error().map(Errno::from_raw),
        Some(Errno::EMFILE | Errno::ENFILE | Errno::ENOBUFS | Errno::ENOMEM | Errno::EADDRNOTAVAIL)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::net::TcpListener;

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest::new("a", addr, Duration::from_secs(1))
    }

    fn checker(opts: &str) -> Result<Arc<dyn Checker>, Vec<OptionError>> {
        TcpFactory.validate(&opts.parse::<toml::Table>().unwrap())
    }

    #[tokio::test]
    async fn listening_backend_is_up() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        assert_eq!(checker("").unwrap().check(req(addr)).await, CheckResult::Up);
    }

    #[tokio::test]
    async fn closed_port_is_down_refused() {
        // A test running in parallel may grab the freed port and listen on
        // it, which makes the check Up. Try a fresh port when that happens.
        // (Binding without listening would hold the port, but macOS drops
        // the SYN instead of refusing it.)
        let mut result = CheckResult::Up;
        for _ in 0..5 {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();
            drop(l);
            result = checker("").unwrap().check(req(addr)).await;
            if result != CheckResult::Up {
                break;
            }
        }
        match result {
            CheckResult::Down(r) => assert!(r.to_lowercase().contains("refused"), "reason: {r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[test]
    fn rejects_any_option() {
        let errs = checker(r#"path = "/""#).unwrap_err();
        assert_eq!(errs, vec![OptionError::new("path", "unknown key")]);
    }

    #[test]
    fn local_errno_classification() {
        assert!(is_local_io_error(&std::io::Error::from_raw_os_error(
            nix::libc::EMFILE
        )));
        assert!(!is_local_io_error(&std::io::Error::from_raw_os_error(
            nix::libc::ECONNREFUSED
        )));
    }
}
