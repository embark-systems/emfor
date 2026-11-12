//! `http` checker: one `GET` per check, judged by response status.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use reqwest::redirect::Policy;

use super::tcp::is_local_io_error;
use super::{CheckRequest, CheckResult, Checker, CheckerFactory, OptionError, reject_unknown_keys};

const ALLOWED: &[&str] = &[
    "path",
    "scheme",
    "host_header",
    "expect_status",
    "tls_verify",
];

pub struct HttpFactory;

/// Inclusive status range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusRange(pub u16, pub u16);

#[derive(Debug, Clone)]
pub struct HttpOptions {
    pub path: String,
    pub https: bool,
    pub host_header: Option<String>,
    pub expect_status: Vec<StatusRange>,
    pub tls_verify: bool,
}

impl CheckerFactory for HttpFactory {
    fn kind(&self) -> &str {
        "http"
    }

    fn validate(&self, options: &toml::Table) -> Result<Arc<dyn Checker>, Vec<OptionError>> {
        let opts = parse_options(options)?;
        Ok(Arc::new(HttpChecker {
            opts,
            clients: Mutex::new(HashMap::new()),
        }))
    }
}

fn parse_options(options: &toml::Table) -> Result<HttpOptions, Vec<OptionError>> {
    let mut errors = Vec::new();
    reject_unknown_keys(options, ALLOWED, &mut errors);

    let path = match options.get("path") {
        None => "/".to_string(),
        Some(toml::Value::String(p)) if p.starts_with('/') => p.clone(),
        Some(toml::Value::String(_)) => {
            errors.push(OptionError::new("path", "must start with '/'"));
            "/".into()
        }
        Some(_) => {
            errors.push(OptionError::new("path", "must be a string"));
            "/".into()
        }
    };

    let https = match options.get("scheme") {
        None => false,
        Some(toml::Value::String(s)) if s == "http" => false,
        Some(toml::Value::String(s)) if s == "https" => true,
        Some(_) => {
            errors.push(OptionError::new("scheme", "must be \"http\" or \"https\""));
            false
        }
    };

    let host_header = match options.get("host_header") {
        None => None,
        Some(toml::Value::String(h))
            if !h.is_empty() && reqwest::header::HeaderValue::from_str(h).is_ok() =>
        {
            Some(h.clone())
        }
        Some(_) => {
            errors.push(OptionError::new(
                "host_header",
                "must be a non-empty header-safe string",
            ));
            None
        }
    };

    let expect_status = match options.get("expect_status") {
        None => vec![StatusRange(200, 399)],
        Some(toml::Value::Array(items)) if !items.is_empty() => {
            let mut ranges = Vec::new();
            for (i, item) in items.iter().enumerate() {
                match parse_status_item(item) {
                    Ok(r) => ranges.push(r),
                    Err(msg) => errors.push(OptionError::new(format!("expect_status[{i}]"), msg)),
                }
            }
            ranges
        }
        Some(_) => {
            errors.push(OptionError::new(
                "expect_status",
                "must be a non-empty array",
            ));
            Vec::new()
        }
    };

    let tls_verify = match options.get("tls_verify") {
        None => true,
        Some(toml::Value::Boolean(b)) => *b,
        Some(_) => {
            errors.push(OptionError::new("tls_verify", "must be a boolean"));
            true
        }
    };

    if errors.is_empty() {
        Ok(HttpOptions {
            path,
            https,
            host_header,
            expect_status,
            tls_verify,
        })
    } else {
        Err(errors)
    }
}

fn parse_status_item(item: &toml::Value) -> Result<StatusRange, String> {
    let code = |n: i64| -> Result<u16, String> {
        if (100..=599).contains(&n) {
            Ok(n as u16)
        } else {
            Err(format!("status {n} outside 100-599"))
        }
    };
    let parse_num = |s: &str| {
        s.trim()
            .parse::<i64>()
            .map_err(|_| format!("invalid status \"{s}\""))
    };
    match item {
        toml::Value::Integer(n) => {
            let c = code(*n)?;
            Ok(StatusRange(c, c))
        }
        toml::Value::String(s) => match s.split_once('-') {
            Some((lo, hi)) => {
                let (lo, hi) = (code(parse_num(lo)?)?, code(parse_num(hi)?)?);
                if lo > hi {
                    Err(format!("range \"{s}\": lower bound exceeds upper bound"))
                } else {
                    Ok(StatusRange(lo, hi))
                }
            }
            None => {
                let c = code(parse_num(s)?)?;
                Ok(StatusRange(c, c))
            }
        },
        _ => Err("must be a status code or \"lo-hi\" range".into()),
    }
}

#[derive(Debug)]
pub struct HttpChecker {
    opts: HttpOptions,
    /// One client per backend, so that a domain `host_header` can be resolved
    /// to that backend's address (correct SNI and certificate check) while
    /// the connection still goes to the backend.
    clients: Mutex<HashMap<SocketAddr, reqwest::Client>>,
}

impl HttpChecker {
    /// `host_header` without port, when it is a domain name (not an IP literal).
    fn host_name(&self) -> Option<&str> {
        let h = host_only(self.opts.host_header.as_deref()?);
        h.parse::<IpAddr>().is_err().then_some(h)
    }

    fn client_for(&self, target: SocketAddr) -> Result<reqwest::Client, reqwest::Error> {
        let mut clients = self.clients.lock().unwrap();
        if let Some(c) = clients.get(&target) {
            return Ok(c.clone());
        }
        let mut b = reqwest::Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .pool_max_idle_per_host(0)
            .tls_danger_accept_invalid_certs(!self.opts.tls_verify);
        if let Some(name) = self.host_name() {
            b = b.resolve(name, target);
        }
        let c = b.build()?;
        clients.insert(target, c.clone());
        Ok(c)
    }

    /// Request URL. The connection always goes to `target`: either the URL
    /// holds `target` itself, or a domain that [`Self::client_for`] resolves
    /// to `target`.
    fn url(&self, target: SocketAddr) -> String {
        let scheme = if self.opts.https { "https" } else { "http" };
        match self.host_name() {
            Some(name) => format!("{scheme}://{name}:{}{}", target.port(), self.opts.path),
            None => format!("{scheme}://{target}{}", self.opts.path),
        }
    }

    fn status_matches(&self, status: u16) -> bool {
        self.opts
            .expect_status
            .iter()
            .any(|r| (r.0..=r.1).contains(&status))
    }
}

/// `host_header` without any `:port` suffix or IPv6 brackets.
fn host_only(h: &str) -> &str {
    if let Some(rest) = h.strip_prefix('[') {
        return rest.split_once(']').map(|(a, _)| a).unwrap_or(h);
    }
    h.rsplit_once(':')
        .filter(|(_, p)| p.parse::<u16>().is_ok())
        .map(|(a, _)| a)
        .unwrap_or(h)
}

impl Checker for HttpChecker {
    fn check(&self, req: CheckRequest) -> BoxFuture<'_, CheckResult> {
        let target = req.target;
        Box::pin(async move {
            let client = match self.client_for(target) {
                Ok(c) => c,
                Err(e) => return CheckResult::Error(format!("cannot build http client: {e}")),
            };
            let mut req = client.get(self.url(target));
            if let Some(h) = &self.opts.host_header {
                req = req.header(reqwest::header::HOST, h);
            }
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if self.status_matches(status) {
                        CheckResult::Up
                    } else {
                        CheckResult::Down(format!("unexpected status {status}"))
                    }
                }
                Err(e) if has_local_io_error(&e) => CheckResult::Error(error_chain(&e)),
                Err(e) => CheckResult::Down(error_chain(&e)),
            }
        })
    }

    fn forget(&self, target: SocketAddr) {
        self.clients.lock().unwrap().remove(&target);
    }
}

fn has_local_io_error(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && is_local_io_error(io)
        {
            return true;
        }
        cur = err.source();
    }
    false
}

fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(err) = cur {
        let s = err.to_string();
        if !parts.last().is_some_and(|p| p.contains(&s)) {
            parts.push(s);
        }
        cur = err.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest::new("a", addr, std::time::Duration::from_secs(1))
    }

    /// Serve one fixed status per connection; send each raw request to `tx`.
    async fn server(status: u16) -> (SocketAddr, mpsc::UnboundedReceiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
                    let extra = if (300..400).contains(&status) {
                        "Location: /elsewhere\r\n"
                    } else {
                        ""
                    };
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        (addr, rx)
    }

    fn checker(opts: &str) -> Arc<dyn Checker> {
        HttpFactory
            .validate(&opts.parse::<toml::Table>().unwrap())
            .unwrap()
    }

    #[tokio::test]
    async fn status_matching_table() {
        // Rows of the "status matching" example in the health-checking spec.
        let cases: &[(&str, u16, Option<&str>)] = &[
            ("", 200, None),
            ("", 302, None),
            ("", 503, Some("503")),
            (r#"expect_status = ["200", "204"]"#, 204, None),
            (r#"expect_status = ["200-299"]"#, 301, Some("301")),
            ("", 204, None),
        ];
        for (opts, status, down_contains) in cases {
            let (addr, _rx) = server(*status).await;
            let r = checker(opts).check(req(addr)).await;
            match down_contains {
                None => assert_eq!(r, CheckResult::Up, "opts {opts:?} status {status}"),
                Some(code) => match r {
                    CheckResult::Down(reason) => assert!(reason.contains(code), "reason {reason}"),
                    other => panic!("opts {opts:?} status {status}: expected down, got {other:?}"),
                },
            }
        }
    }

    #[tokio::test]
    async fn sends_host_header_and_path() {
        let (addr, mut rx) = server(200).await;
        let c = checker(
            r#"host_header = "api.internal"
path = "/healthz""#,
        );
        assert_eq!(c.check(req(addr)).await, CheckResult::Up);
        let req = rx.recv().await.unwrap().to_lowercase();
        assert!(req.starts_with("get /healthz "), "request: {req}");
        assert!(req.contains("host: api.internal\r\n"), "request: {req}");
    }

    #[tokio::test]
    async fn ip_literal_host_header_still_connects_to_backend() {
        for host in [
            "10.255.255.1",
            "10.255.255.1:8080",
            "[2001:db8::1]:8080",
            "[2001:db8::1]",
        ] {
            let (addr, mut rx) = server(200).await;
            let c = checker(&format!("host_header = \"{host}\""));
            assert_eq!(
                c.check(req(addr)).await,
                CheckResult::Up,
                "host_header {host}"
            );
            let req = rx.recv().await.unwrap().to_lowercase();
            assert!(req.contains(&format!("host: {host}\r\n")), "request: {req}");
        }
    }

    #[tokio::test]
    async fn silent_backend_times_out() {
        // Accepts the connection, never answers.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = l.accept().await {
                held.push(s);
            }
        });
        let r = crate::checker::run_check(
            checker("").as_ref(),
            CheckRequest::new("a", addr, std::time::Duration::from_millis(200)),
        )
        .await;
        assert_eq!(r, CheckResult::Down("timeout".into()));
    }

    #[tokio::test]
    async fn forget_releases_backend_client() {
        let (addr, _rx) = server(200).await;
        let c = HttpChecker {
            opts: parse_options(&toml::Table::new()).unwrap(),
            clients: Mutex::new(HashMap::new()),
        };
        assert_eq!(c.check(req(addr)).await, CheckResult::Up);
        assert_eq!(c.clients.lock().unwrap().len(), 1);
        c.forget(addr);
        assert!(c.clients.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn refused_is_down() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        assert!(matches!(
            checker("").check(req(addr)).await,
            CheckResult::Down(_)
        ));
    }

    #[test]
    fn validation_errors() {
        let bad = |opts: &str| {
            HttpFactory
                .validate(&opts.parse::<toml::Table>().unwrap())
                .unwrap_err()
        };
        let e = bad(r#"expect_status = ["299-200"]"#);
        assert_eq!(e[0].key, "expect_status[0]");
        assert!(e[0].message.contains("lower bound exceeds upper bound"));
        assert_eq!(bad(r#"scheme = "ftp""#)[0].key, "scheme");
        assert_eq!(bad(r#"path = "healthz""#)[0].key, "path");
        assert_eq!(
            bad(r#"methd = "GET""#)[0],
            OptionError::new("methd", "unknown key")
        );
        assert_eq!(bad(r#"expect_status = [700]"#)[0].key, "expect_status[0]");
        // All problems reported together.
        assert_eq!(
            bad(r#"scheme = "ftp"
tls_verify = "yes""#)
            .len(),
            2
        );
    }

    #[test]
    fn host_only_strips_port() {
        assert_eq!(host_only("api.internal"), "api.internal");
        assert_eq!(host_only("api.internal:8443"), "api.internal");
        assert_eq!(host_only("[2001:db8::1]:80"), "2001:db8::1");
        assert_eq!(host_only("[2001:db8::1]"), "2001:db8::1");
    }
}
