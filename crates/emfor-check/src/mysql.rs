//! The `mysql` plugin: log in, `SELECT 1`, and with `role = "primary"`
//! check that the server is not read-only; with `role = "replica"` check
//! that both replication threads run and, with `max_lag`, how far behind
//! the replica is.

use emfor_plugin::{CheckOutcome, CheckRequest};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, DriverError, Error, IoError, OptsBuilder, Row, SslOpts};

use crate::net::{self, io_failure};
use crate::options::{Role, Settings, TlsSettings, read_password};
use crate::replica::lag_outcome;

/// The server error for SQL it does not understand; older servers answer
/// `SHOW REPLICA STATUS` with it.
const ER_PARSE_ERROR: u16 = 1064;

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
    match settings.role {
        Some(Role::Primary) => {}
        Some(Role::Replica) => return replica(conn, settings.max_lag).await,
        Some(Role::Master) | None => return CheckOutcome::Up,
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

/// Check a replica: `SHOW REPLICA STATUS`, or `SHOW SLAVE STATUS` on servers
/// that do not know the newer statement.
async fn replica(conn: &mut Conn, max_lag: Option<u64>) -> CheckOutcome {
    let row = match conn.query_first::<Row, _>("SHOW REPLICA STATUS").await {
        Err(Error::Server(e)) if e.code == ER_PARSE_ERROR => {
            conn.query_first::<Row, _>("SHOW SLAVE STATUS").await
        }
        other => other,
    };
    match row {
        Ok(None) => replica_outcome(None, max_lag),
        Ok(Some(row)) => match ReplicaStatus::from_row(&row) {
            Ok(status) => replica_outcome(Some(&status), max_lag),
            Err(outcome) => outcome,
        },
        Err(e) => failure(Step::Query, &e),
    }
}

/// The replica status columns the check needs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ReplicaStatus {
    io: String,
    io_error: String,
    sql: String,
    sql_error: String,
    /// Seconds behind the source; `None` when the server reports NULL.
    behind: Option<u64>,
}

impl ReplicaStatus {
    /// Read the columns under either name: MySQL 8.0.22 and later say
    /// `Replica_*` and `Source`, MariaDB and older servers `Slave_*` and
    /// `Master`.
    fn from_row(row: &Row) -> Result<ReplicaStatus, CheckOutcome> {
        fn text(row: &Row, names: &[&str]) -> Result<String, CheckOutcome> {
            for name in names {
                if let Some(v) = row.get_opt::<Option<String>, &str>(name) {
                    return v.map(Option::unwrap_or_default).map_err(|_| {
                        CheckOutcome::Down(format!(
                            "query: replica status column {name} is not text"
                        ))
                    });
                }
            }
            Err(CheckOutcome::Down(format!(
                "query: replica status has no {} column",
                names[0]
            )))
        }
        let behind_names = ["Seconds_Behind_Source", "Seconds_Behind_Master"];
        let behind = match behind_names
            .iter()
            .find_map(|name| row.get_opt::<Option<u64>, &str>(name))
        {
            Some(Ok(v)) => v,
            Some(Err(_)) => {
                return Err(CheckOutcome::Down(
                    "query: replica status column Seconds_Behind_Master is not a number".into(),
                ));
            }
            None => {
                return Err(CheckOutcome::Down(
                    "query: replica status has no Seconds_Behind_Source column".into(),
                ));
            }
        };
        Ok(ReplicaStatus {
            io: text(row, &["Replica_IO_Running", "Slave_IO_Running"])?,
            io_error: text(row, &["Last_IO_Error"])?,
            sql: text(row, &["Replica_SQL_Running", "Slave_SQL_Running"])?,
            sql_error: text(row, &["Last_SQL_Error"])?,
            behind,
        })
    }
}

/// Judge a replica's status: no row means the server is not a replica.
fn replica_outcome(status: Option<&ReplicaStatus>, max_lag: Option<u64>) -> CheckOutcome {
    let Some(status) = status else {
        return CheckOutcome::Down("role is not a replica, want replica".into());
    };
    let thread = |name: &str, value: &str, error: &str| {
        let detail = if error.is_empty() {
            String::new()
        } else {
            format!(": {error}")
        };
        CheckOutcome::Down(format!("replication: {name} thread is {value}{detail}"))
    };
    if status.io != "Yes" {
        return thread("IO", &status.io, &status.io_error);
    }
    if status.sql != "Yes" {
        return thread("SQL", &status.sql, &status.sql_error);
    }
    match max_lag {
        Some(max_lag) => lag_outcome(status.behind.map(|s| s as f64), max_lag),
        None => CheckOutcome::Up,
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
    fn mysql_replica_status_table() {
        let status = |io: &str, io_error: &str, sql: &str, sql_error: &str, behind| ReplicaStatus {
            io: io.into(),
            io_error: io_error.into(),
            sql: sql.into(),
            sql_error: sql_error.into(),
            behind,
        };
        let down = |r: &str| CheckOutcome::Down(r.into());
        let rows = [
            (None, None, down("role is not a replica, want replica")),
            (
                None,
                Some(status("Yes", "", "Yes", "", None)),
                CheckOutcome::Up,
            ),
            (
                None,
                Some(status(
                    "Connecting",
                    "error connecting to master",
                    "Yes",
                    "",
                    None,
                )),
                down("replication: IO thread is Connecting: error connecting to master"),
            ),
            (
                None,
                Some(status("No", "", "No", "", None)),
                down("replication: IO thread is No"),
            ),
            (
                None,
                Some(status("Yes", "", "No", "Duplicate entry", None)),
                down("replication: SQL thread is No: Duplicate entry"),
            ),
            (
                Some(30),
                Some(status("Yes", "", "Yes", "", Some(30))),
                CheckOutcome::Up,
            ),
            (
                Some(30),
                Some(status("Yes", "", "Yes", "", Some(31))),
                down("replication: 31s behind, max 30s"),
            ),
            (
                Some(30),
                Some(status("Yes", "", "Yes", "", None)),
                down("replication: lag unknown"),
            ),
        ];
        for (max_lag, status, want) in rows {
            assert_eq!(
                replica_outcome(status.as_ref(), max_lag),
                want,
                "{max_lag:?} {status:?}"
            );
        }
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
