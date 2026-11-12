//! The `pgsql` plugin: log in, `SELECT 1`, and with `role = "primary"`
//! check that the server is not in recovery (a standby).

use std::error::Error as _;
use std::io;

use emfor_plugin::{CheckOutcome, CheckRequest};
use tokio_postgres::config::SslMode;
use tokio_postgres::{Client, Config, Error, NoTls};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::net::{self, io_failure};
use crate::options::{Role, Settings, read_password};

/// Which step failed, for the reason prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Login,
    Query,
}

/// Check one PostgreSQL backend with validated settings.
pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let mut config = Config::new();
    config
        .hostaddr(req.ip)
        .port(req.port)
        .application_name("emfor-check")
        .connect_timeout(req.timeout);
    if let Some(user) = &settings.user {
        config.user(user);
    }
    if let Some(db) = &settings.database {
        config.dbname(db);
    }
    if let Some(path) = &settings.password_file {
        match read_password(path) {
            Ok(p) => {
                config.password(p);
            }
            Err(reason) => return CheckOutcome::Error(reason),
        }
    }

    let connected = match &settings.tls {
        Some(tls) => {
            // TLS name and SNI come from `host`; the address from `hostaddr`.
            config
                .host(
                    tls.server_name
                        .clone()
                        .unwrap_or_else(|| req.ip.to_string()),
                )
                .ssl_mode(SslMode::Require);
            let client_config = match net::client_config(tls) {
                Ok(c) => c,
                Err(outcome) => return outcome,
            };
            let connector = MakeRustlsConnect::new((*client_config).clone());
            config
                .connect(connector)
                .await
                .map(|(client, conn)| (client, tokio::spawn(conn)))
        }
        None => {
            config.host(req.ip.to_string()).ssl_mode(SslMode::Disable);
            config
                .connect(NoTls)
                .await
                .map(|(client, conn)| (client, tokio::spawn(conn)))
        }
    };
    let (client, connection) = match connected {
        Ok(c) => c,
        Err(e) => return failure(Step::Login, &e),
    };
    let outcome = session(&client, settings).await;
    // Dropping the client makes the connection task send Terminate and
    // close; wait briefly so the connection is closed before answering.
    drop(client);
    let _ = tokio::time::timeout(CLOSE_WAIT, connection).await;
    outcome
}

/// How long a check waits for the connection to close before answering.
const CLOSE_WAIT: std::time::Duration = std::time::Duration::from_millis(100);

async fn session(client: &Client, settings: &Settings) -> CheckOutcome {
    match client.query_one("SELECT 1", &[]).await {
        Ok(row) => match row.try_get::<_, i32>(0) {
            Ok(1) => {}
            Ok(n) => return CheckOutcome::Down(format!("query: SELECT 1 answered {n}")),
            Err(e) => return failure(Step::Query, &e),
        },
        Err(e) => return failure(Step::Query, &e),
    }
    if settings.role != Some(Role::Primary) {
        return CheckOutcome::Up;
    }
    match client.query_one("SELECT pg_is_in_recovery()", &[]).await {
        Ok(row) => match row.try_get::<_, bool>(0) {
            Ok(false) => CheckOutcome::Up,
            Ok(true) => CheckOutcome::Down("role is standby, want primary".into()),
            Err(e) => failure(Step::Query, &e),
        },
        Err(e) => failure(Step::Query, &e),
    }
}

/// Sort a driver error into a check outcome.
///
/// tokio-postgres keeps its error kinds private; the kind is the error's
/// text and the detail is its source.
fn failure(step: Step, e: &Error) -> CheckOutcome {
    let prefix = match step {
        Step::Login => "auth: ",
        Step::Query => "query: ",
    };
    if let Some(db) = e.as_db_error() {
        return CheckOutcome::Down(format!("{prefix}{}: {}", db.code().code(), db.message()));
    }
    let kind = e.to_string();
    let detail = e.source().map(|s| s.to_string());
    let text = match &detail {
        Some(d) => format!("{kind}: {d}"),
        None => kind.clone(),
    };
    if let Some(io) = e.source().and_then(|s| s.downcast_ref::<io::Error>()) {
        let io_prefix = match (step, kind.as_str()) {
            (_, "error performing TLS handshake") => "tls: ",
            (Step::Login, _) => "connect: ",
            (Step::Query, _) => "query: ",
        };
        return io_failure(io_prefix, io);
    }
    match kind.as_str() {
        "error performing TLS handshake" => {
            CheckOutcome::Down(format!("tls: {}", detail.unwrap_or(kind)))
        }
        "timeout waiting for server" => CheckOutcome::Down("timeout".into()),
        _ => CheckOutcome::Down(format!("{prefix}{text}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::TlsSettings;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const SSL_REQUEST: i32 = 80877103;

    /// A fake server that refuses TLS (answers `N` to SSLRequest) and then
    /// rejects the login with SQLSTATE 28P01.
    async fn fake_pg() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    loop {
                        let Ok(len) = s.read_i32().await else { return };
                        let mut body = vec![0u8; len as usize - 4];
                        if s.read_exact(&mut body).await.is_err() {
                            return;
                        }
                        if len == 8
                            && i32::from_be_bytes(body[..4].try_into().unwrap()) == SSL_REQUEST
                        {
                            let _ = s.write_all(b"N").await;
                            continue;
                        }
                        let mut fields = Vec::new();
                        for (t, v) in [
                            (b'S', "FATAL"),
                            (b'V', "FATAL"),
                            (b'C', "28P01"),
                            (b'M', "password authentication failed for user \"emfor\""),
                        ] {
                            fields.push(t);
                            fields.extend_from_slice(v.as_bytes());
                            fields.push(0);
                        }
                        fields.push(0);
                        let mut msg = vec![b'E'];
                        msg.extend_from_slice(&((fields.len() + 4) as i32).to_be_bytes());
                        msg.extend_from_slice(&fields);
                        let _ = s.write_all(&msg).await;
                        return;
                    }
                });
            }
        });
        addr
    }

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest {
            service: "db".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: Default::default(),
            timeout: Duration::from_secs(1),
        }
    }

    fn settings(tls: Option<TlsSettings>) -> Settings {
        Settings {
            user: Some("emfor".into()),
            database: Some("app".into()),
            tls,
            ..Settings::default()
        }
    }

    #[tokio::test]
    async fn rejected_login_is_auth() {
        let addr = fake_pg().await;
        assert_eq!(
            check(&req(addr), &settings(None)).await,
            CheckOutcome::Down(
                "auth: 28P01: password authentication failed for user \"emfor\"".into()
            )
        );
    }

    #[tokio::test]
    async fn tls_to_a_server_without_tls_is_down() {
        let addr = fake_pg().await;
        let tls = TlsSettings {
            verify: false,
            server_name: None,
            ca_file: None,
        };
        match check(&req(addr), &settings(Some(tls))).await {
            CheckOutcome::Down(r) => assert!(r.starts_with("tls: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn nothing_listening_is_connect() {
        let mut outcome = CheckOutcome::Up;
        for _ in 0..5 {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();
            drop(l);
            outcome = check(&req(addr), &settings(None)).await;
            if !matches!(&outcome, CheckOutcome::Down(r) if r.starts_with("auth: 28P01")) {
                break;
            }
        }
        match outcome {
            CheckOutcome::Down(r) => assert!(r.starts_with("connect: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }
}
