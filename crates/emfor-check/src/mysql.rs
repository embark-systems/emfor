//! The `mysql` plugin: log in, `SELECT 1`, and with `role = "primary"`
//! check that the server is not read-only; with `role = "replica"` check
//! that both replication threads run and, with `max_lag`, how far behind
//! the replica is.

use emfor_plugin::{CheckOutcome, CheckRequest};
use std::time::Duration;

use mysql_async::prelude::{FromValue, Queryable};
use mysql_async::{Conn, DriverError, Error, FromValueError, IoError, OptsBuilder, Row, SslOpts};

use crate::net::{self, io_failure};
use crate::options::{Role, Settings, TlsSettings, read_password};
use crate::replica::lag_outcome;

/// The server error for SQL it does not understand; older servers answer
/// `SHOW REPLICA STATUS` with it.
const ER_PARSE_ERROR: u16 = 1064;

/// Time a connection the check no longer waits for has to send `COM_QUIT`
/// and close.
const QUIT_TIMEOUT: Duration = Duration::from_secs(1);

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
        Ok(c) => Closing(Some(c)),
        Err(e) => return failure(Step::Login, &e),
    };
    let outcome = session(conn.get(), settings).await;
    conn.disconnect().await;
    outcome
}

/// A connection that closes without reading what the server still sends
/// when the check is dropped (on its timeout). mysql_async closes a dropped
/// `Conn` in a task of its own that first reads the rest of an unfinished
/// result, with no time limit: a server that stalls in the middle of a
/// result would keep that task and its socket for good, one more at every
/// check.
struct Closing(Option<Conn>);

impl Closing {
    fn get(&mut self) -> &mut Conn {
        self.0
            .as_mut()
            .expect("Closing holds its connection until disconnect or drop takes it")
    }

    async fn disconnect(mut self) {
        if let Some(conn) = self.0.take() {
            let _ = conn.disconnect().await;
        }
    }
}

impl Drop for Closing {
    fn drop(&mut self) {
        let Some(conn) = self.0.take() else { return };
        // `disconnect` marks the connection closed when first polled, which
        // `timeout` does at once, so dropping it later does not start
        // mysql_async's own cleanup.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = tokio::time::timeout(QUIT_TIMEOUT, conn.disconnect()).await;
            });
        }
    }
}

async fn session(conn: &mut Conn, settings: &Settings) -> CheckOutcome {
    match integer(conn, "SELECT 1", "SELECT 1").await {
        Ok(1) => {}
        Ok(other) => return CheckOutcome::Down(format!("query: SELECT 1 answered {other}")),
        Err(outcome) => return outcome,
    }
    match settings.role {
        Some(Role::Primary) => {}
        Some(Role::Replica) => return replica(conn, settings.max_lag).await,
        Some(Role::Master) | None => return CheckOutcome::Up,
    }
    match integer(conn, "SELECT @@global.read_only", "@@global.read_only").await {
        Ok(0) => CheckOutcome::Up,
        Ok(_) => CheckOutcome::Down("role is read-only, want primary".into()),
        Err(outcome) => outcome,
    }
}

/// Run `query` and read the first column of its first row as an integer;
/// `label` names it in the reason. No row, NULL or a value that is not an
/// integer is `down` (the driver's own conversion would panic on them).
async fn integer(conn: &mut Conn, query: &str, label: &str) -> Result<i64, CheckOutcome> {
    let row = conn
        .query_first::<Row, _>(query)
        .await
        .map_err(|e| failure(Step::Query, &e))?;
    let Some(value) = row.and_then(|r| r.as_ref(0).cloned()) else {
        return Err(CheckOutcome::Down(format!(
            "query: {label} answered no row"
        )));
    };
    i64::from_value_opt(value).map_err(|FromValueError(v)| {
        CheckOutcome::Down(format!(
            "query: {label} answered {}, want an integer",
            v.as_sql(true)
        ))
    })
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
        // mysql_async adds each root strictly, so one certificate rustls
        // cannot use as a root would fail every handshake; skip those, as
        // the shared configuration does.
        let roots: Vec<_> = net::trusted_certificates(tls)?
            .into_iter()
            .filter(|c| rustls::RootCertStore::empty().add(c.clone()).is_ok())
            .map(|c| c.to_vec().into())
            .collect();
        if roots.is_empty() {
            return Err(CheckOutcome::Error(match tls.ca_file {
                Some(_) => "ca_file: no usable certificate".into(),
                None => "tls: no usable system CA certificates; set ca_file".into(),
            }));
        }
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
    use std::net::SocketAddr;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// One protocol packet: 3-byte length, sequence id, payload.
    fn packet(seq: u8, payload: &[u8]) -> Vec<u8> {
        let mut p = u32::try_from(payload.len()).unwrap().to_le_bytes()[..3].to_vec();
        p.push(seq);
        p.extend_from_slice(payload);
        p
    }

    async fn read_packet(s: &mut TcpStream) -> Vec<u8> {
        let mut head = [0u8; 4];
        s.read_exact(&mut head).await.unwrap();
        let mut body = vec![0u8; u32::from_le_bytes([head[0], head[1], head[2], 0]) as usize];
        s.read_exact(&mut body).await.unwrap();
        body
    }

    /// A text result set with string columns `names`: the column count,
    /// the definitions and EOF, then `rows` (`None` is NULL) and EOF unless
    /// `rows` is `None`, which leaves the result unfinished.
    async fn result_set(s: &mut TcpStream, names: &[&str], rows: Option<&[&[Option<&str>]]>) {
        let mut seq = 1u8;
        let mut next = |payload: &[u8]| {
            let p = packet(seq, payload);
            seq += 1;
            p
        };
        let eof = [0xfe, 0, 0, 0x02, 0];
        let mut out = next(&[u8::try_from(names.len()).unwrap()]);
        for name in names {
            let mut c = Vec::new();
            for part in [&b"def"[..], b"", b"", b"", name.as_bytes(), b""] {
                c.push(u8::try_from(part.len()).unwrap());
                c.extend_from_slice(part);
            }
            // Fixed fields: utf8mb4, length 64, VAR_STRING, no flags.
            c.extend_from_slice(&[0x0c, 0xff, 0, 64, 0, 0, 0, 0xfd, 0, 0, 0, 0, 0]);
            out.extend(next(&c));
        }
        out.extend(next(&eof));
        if let Some(rows) = rows {
            for row in rows {
                let mut r = Vec::new();
                for value in *row {
                    match value {
                        Some(v) => {
                            r.push(u8::try_from(v.len()).unwrap());
                            r.extend_from_slice(v.as_bytes());
                        }
                        None => r.push(0xfb),
                    }
                }
                out.extend(next(&r));
            }
            out.extend(next(&eof));
        }
        s.write_all(&out).await.unwrap();
    }

    /// A MySQL server for one connection: it accepts any login, answers
    /// the driver's settings query and then `SELECT 1` with one row holding
    /// `answer`, or with `None` sends the columns and stalls. It returns
    /// when the client closes the connection, and reports when that was.
    async fn fake_server(
        answer: Option<Option<&'static str>>,
    ) -> (SocketAddr, tokio::task::JoinHandle<Instant>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            // LONG_PASSWORD, PROTOCOL_41, TRANSACTIONS, SECURE_CONNECTION,
            // PLUGIN_AUTH.
            let caps: u32 = 0x1 | 0x200 | 0x2000 | 0x8000 | 0x80000;
            let mut hello = vec![10];
            hello.extend_from_slice(b"8.0.36\0");
            hello.extend_from_slice(&7u32.to_le_bytes());
            hello.extend_from_slice(b"abcdefgh\0");
            hello.extend_from_slice(&(caps as u16).to_le_bytes());
            hello.push(0xff);
            hello.extend_from_slice(&2u16.to_le_bytes());
            hello.extend_from_slice(&((caps >> 16) as u16).to_le_bytes());
            hello.push(21);
            hello.extend_from_slice(&[0; 10]);
            hello.extend_from_slice(b"ijklmnopqrst\0mysql_native_password\0");
            s.write_all(&packet(0, &hello)).await.unwrap();
            read_packet(&mut s).await;
            s.write_all(&packet(2, &[0, 0, 0, 2, 0, 0, 0]))
                .await
                .unwrap();
            // The driver asks for max_allowed_packet and wait_timeout.
            let query = read_packet(&mut s).await;
            assert!(query.starts_with(b"\x03SELECT @@"), "{query:?}");
            let settings: &[&[Option<&str>]] = &[&[Some("67108864"), Some("28800")]];
            result_set(&mut s, &["a", "b"], Some(settings)).await;
            let query = read_packet(&mut s).await;
            assert_eq!(query, b"\x03SELECT 1");
            match answer {
                Some(value) => result_set(&mut s, &["1"], Some(&[&[value]])).await,
                None => result_set(&mut s, &["1"], None).await,
            }
            let mut rest = Vec::new();
            let _ = s.read_to_end(&mut rest).await;
            Instant::now()
        });
        (addr, server)
    }

    fn fake_req(addr: SocketAddr) -> CheckRequest {
        CheckRequest {
            service: "db".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: Default::default(),
            timeout: Duration::from_secs(2),
        }
    }

    fn no_options() -> Settings {
        Settings {
            user: Some("emfor".into()),
            ..Settings::default()
        }
    }

    #[tokio::test]
    async fn select_one_answers() {
        let rows = [
            (Some("1"), CheckOutcome::Up),
            (
                None,
                CheckOutcome::Down("query: SELECT 1 answered NULL, want an integer".into()),
            ),
            (
                Some("one"),
                CheckOutcome::Down("query: SELECT 1 answered 'one', want an integer".into()),
            ),
            (
                Some("2"),
                CheckOutcome::Down("query: SELECT 1 answered 2".into()),
            ),
        ];
        for (value, want) in rows {
            let (addr, server) = fake_server(Some(value)).await;
            let got = check(&fake_req(addr), &no_options()).await;
            assert_eq!(got, want, "{value:?}");
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn connection_cut_off_by_the_timeout_is_closed() {
        // The server sends the columns of SELECT 1 and then nothing; the
        // check's timeout drops the connection in the middle of the result.
        let (addr, server) = fake_server(None).await;
        let cut = tokio::time::timeout(
            Duration::from_millis(300),
            check(&fake_req(addr), &no_options()),
        )
        .await;
        assert!(cut.is_err(), "{cut:?}");
        let dropped = Instant::now();
        let closed = tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .expect("the connection is closed soon after the timeout")
            .unwrap();
        assert!(closed.duration_since(dropped) < Duration::from_secs(2));
    }

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

        // A certificate rustls cannot use as a root is skipped, as the
        // other plugins do, instead of failing every handshake.
        let bad = "-----BEGIN CERTIFICATE-----\nMAA=\n-----END CERTIFICATE-----\n";
        let mixed = dir.path().join("mixed.pem");
        let good = include_str!("../tests/data/test-ca.pem");
        std::fs::write(&mixed, format!("{bad}{good}")).unwrap();
        let tls = |ca_file| TlsSettings {
            verify: true,
            server_name: None,
            ca_file: Some(ca_file),
        };
        assert_eq!(ssl_opts(&tls(mixed)).unwrap().root_certs().len(), 1);
        let only_bad = dir.path().join("bad.pem");
        std::fs::write(&only_bad, bad).unwrap();
        assert_eq!(
            ssl_opts(&tls(only_bad)).err(),
            Some(CheckOutcome::Error("ca_file: no usable certificate".into()))
        );
    }
}
