//! The `mysql` plugin: log in, `SELECT 1`, and with `role = "primary"`
//! check that the server is not read-only.

use emfor_plugin::{CheckOutcome, CheckRequest};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, DriverError, Error, IoError, OptsBuilder, SslOpts};

use crate::net::{self, io_failure};
use crate::options::{Role, Settings, TlsSettings, read_password};

/// Which step failed, for the reason prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Login,
    Query,
}

/// Check one MySQL backend with validated settings.
pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let password = match &settings.password_file {
        Some(path) => match read_password(path) {
            Ok(p) => Some(p),
            Err(reason) => return CheckOutcome::Error(reason),
        },
        None => None,
    };
    let mut opts = OptsBuilder::default()
        .ip_or_hostname(req.ip.to_string())
        .tcp_port(req.port)
        .user(settings.user.clone())
        .pass(password)
        .prefer_socket(false)
        .stmt_cache_size(0);
    if let Some(tls) = &settings.tls {
        match ssl_opts(tls) {
            Ok(ssl) => opts = opts.ssl_opts(ssl),
            Err(outcome) => return outcome,
        }
    }

    let mut conn = match Conn::new(opts).await {
        Ok(c) => c,
        Err(e) => return failure(Step::Login, &e),
    };
    let outcome = session(&mut conn, settings).await;
    let _ = conn.disconnect().await;
    outcome
}

async fn session(conn: &mut Conn, settings: &Settings) -> CheckOutcome {
    match conn.query_first::<i64, _>("SELECT 1").await {
        Ok(Some(1)) => {}
        Ok(other) => return CheckOutcome::Down(format!("query: SELECT 1 answered {other:?}")),
        Err(e) => return failure(Step::Query, &e),
    }
    if settings.role != Some(Role::Primary) {
        return CheckOutcome::Up;
    }
    match conn
        .query_first::<i64, _>("SELECT @@global.read_only")
        .await
    {
        Ok(Some(0)) => CheckOutcome::Up,
        Ok(Some(_)) => CheckOutcome::Down("role is read-only, want primary".into()),
        Ok(None) => CheckOutcome::Down("query: @@global.read_only answered no row".into()),
        Err(e) => failure(Step::Query, &e),
    }
}

/// TLS options with the same meaning as the shared rustls configuration.
fn ssl_opts(tls: &TlsSettings) -> Result<SslOpts, CheckOutcome> {
    let mut ssl = SslOpts::default();
    if tls.verify {
        let roots = net::trusted_certificates(tls)?
            .into_iter()
            .map(|c| c.to_vec().into())
            .collect();
        ssl = ssl.with_disable_built_in_roots(true).with_root_certs(roots);
    } else {
        ssl = ssl.with_danger_accept_invalid_certs(true);
    }
    Ok(ssl.with_danger_tls_hostname_override(tls.server_name.clone()))
}

/// Sort a driver error into a check outcome.
fn failure(step: Step, e: &Error) -> CheckOutcome {
    let prefix = match step {
        Step::Login => "auth: ",
        Step::Query => "query: ",
    };
    match e {
        Error::Io(IoError::Io(io)) => match step {
            Step::Login => io_failure("connect: ", io),
            Step::Query => io_failure("query: ", io),
        },
        Error::Io(IoError::Tls(t)) => CheckOutcome::Down(format!("tls: {t}")),
        Error::Driver(DriverError::NoClientSslFlagFromServer) => {
            CheckOutcome::Down("tls: server does not support TLS".into())
        }
        Error::Driver(DriverError::ConnectionClosed) => {
            CheckOutcome::Down(format!("{prefix}connection closed"))
        }
        Error::Server(s) => CheckOutcome::Down(format!("{prefix}{s}")),
        other => CheckOutcome::Down(format!("{prefix}{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mysql_async::ServerError;
    use std::io;

    #[test]
    fn errors_are_sorted_by_step() {
        let denied = Error::Server(ServerError {
            code: 1045,
            message: "Access denied for user 'emfor'@'10.0.0.5' (using password: YES)".into(),
            state: "28000".into(),
        });
        assert!(
            matches!(failure(Step::Login, &denied), CheckOutcome::Down(r) if r.starts_with("auth: ERROR 1045"))
        );
        assert!(
            matches!(failure(Step::Query, &denied), CheckOutcome::Down(r) if r.starts_with("query: ERROR 1045"))
        );

        let refused = Error::Io(IoError::Io(io::Error::from(
            io::ErrorKind::ConnectionRefused,
        )));
        assert!(
            matches!(failure(Step::Login, &refused), CheckOutcome::Down(r) if r.starts_with("connect: "))
        );
        assert!(
            matches!(failure(Step::Query, &refused), CheckOutcome::Down(r) if r.starts_with("query: "))
        );

        let emfile = Error::Io(IoError::Io(io::Error::from_raw_os_error(libc::EMFILE)));
        assert!(
            matches!(failure(Step::Login, &emfile), CheckOutcome::Error(r) if r.starts_with("connect: "))
        );

        let no_tls = Error::Driver(DriverError::NoClientSslFlagFromServer);
        assert_eq!(
            failure(Step::Login, &no_tls),
            CheckOutcome::Down("tls: server does not support TLS".into())
        );

        let closed = Error::Driver(DriverError::ConnectionClosed);
        assert_eq!(
            failure(Step::Query, &closed),
            CheckOutcome::Down("query: connection closed".into())
        );
    }

    #[test]
    fn ssl_options_follow_the_settings() {
        let off = ssl_opts(&TlsSettings {
            verify: false,
            server_name: None,
            ca_file: None,
        })
        .unwrap();
        assert!(off.accept_invalid_certs());
        assert_eq!(off.tls_hostname_override(), None);

        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, include_str!("../tests/data/test-ca.pem")).unwrap();
        let on = ssl_opts(&TlsSettings {
            verify: true,
            server_name: Some("db.internal".into()),
            ca_file: Some(ca),
        })
        .unwrap();
        assert!(!on.accept_invalid_certs());
        assert!(on.disable_built_in_roots());
        assert_eq!(on.root_certs().len(), 1);
        assert_eq!(on.tls_hostname_override(), Some("db.internal"));
    }
}
