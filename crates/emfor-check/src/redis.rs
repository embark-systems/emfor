//! The `redis` plugin: optional AUTH, PING, and an optional ROLE check;
//! a replica must also be connected to its master and, with `max_lag`,
//! have heard from it recently.

use emfor_plugin::{CheckOutcome, CheckRequest};
use tokio::io::AsyncWriteExt;

use crate::net::{self, Stream, io_failure};
use crate::options::{Role, Settings, read_password};
use crate::replica::lag_outcome;
use crate::resp::{ReadError, Reader, Reply, encode};

/// Check one redis backend with validated settings.
pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let password = match &settings.password_file {
        Some(path) => match read_password(path) {
            Ok(p) => Some(p),
            Err(reason) => return CheckOutcome::Error(reason),
        },
        None => None,
    };
    let mut conn = match net::open(req.ip, req.port, settings.tls.as_ref(), req.timeout).await {
        Ok(c) => c,
        Err(outcome) => return outcome,
    };
    let outcome = session(&mut conn, settings, password.as_deref()).await;
    let _ = conn.shutdown().await;
    outcome
}

async fn session(
    conn: &mut Box<dyn Stream>,
    settings: &Settings,
    password: Option<&str>,
) -> CheckOutcome {
    let mut reader = Reader::new();

    if let Some(password) = password {
        let cmd = match &settings.user {
            Some(user) => encode(&[b"AUTH", user.as_bytes(), password.as_bytes()]),
            None => encode(&[b"AUTH", password.as_bytes()]),
        };
        match call(conn, &mut reader, &cmd, "auth: ").await {
            Ok(Reply::Simple(s)) if s == "OK" => {}
            Ok(Reply::Error(e)) => return CheckOutcome::Down(format!("auth: {e}")),
            Ok(other) => {
                return CheckOutcome::Down(format!("auth: unexpected reply {}", describe(&other)));
            }
            Err(outcome) => return outcome,
        }
    }

    match call(conn, &mut reader, &encode(&[b"PING"]), "query: ").await {
        Ok(Reply::Simple(s)) if s == "PONG" => {}
        Ok(Reply::Error(e)) => return CheckOutcome::Down(format!("query: {e}")),
        Ok(other) => {
            return CheckOutcome::Down(format!("query: PING answered {}", describe(&other)));
        }
        Err(outcome) => return outcome,
    }

    let Some(want) = settings.role else {
        return CheckOutcome::Up;
    };
    let (actual, link) = match call(conn, &mut reader, &encode(&[b"ROLE"]), "query: ").await {
        Ok(Reply::Array(Some(items))) => match items.first() {
            Some(Reply::Bulk(Some(name))) => {
                // A replica's reply is [slave, host, port, state, offset].
                let link = match items.get(3) {
                    Some(Reply::Bulk(Some(state))) => {
                        Some(String::from_utf8_lossy(state).into_owned())
                    }
                    _ => None,
                };
                (String::from_utf8_lossy(name).into_owned(), link)
            }
            _ => return CheckOutcome::Down("query: ROLE answered an unexpected reply".into()),
        },
        Ok(Reply::Error(e)) => return CheckOutcome::Down(format!("query: {e}")),
        Ok(other) => {
            return CheckOutcome::Down(format!("query: ROLE answered {}", describe(&other)));
        }
        Err(outcome) => return outcome,
    };
    let outcome = role_outcome(want, &actual, link.as_deref());
    let (CheckOutcome::Up, Some(max_lag)) = (&outcome, settings.max_lag) else {
        return outcome;
    };
    let info = match call(
        conn,
        &mut reader,
        &encode(&[b"INFO", b"replication"]),
        "query: ",
    )
    .await
    {
        Ok(Reply::Bulk(Some(text))) => text,
        Ok(Reply::Error(e)) => return CheckOutcome::Down(format!("query: {e}")),
        Ok(other) => {
            return CheckOutcome::Down(format!("query: INFO answered {}", describe(&other)));
        }
        Err(outcome) => return outcome,
    };
    lag_outcome(last_io_seconds(&info), max_lag)
}

/// `master_last_io_seconds_ago` from an `INFO replication` reply; `None`
/// when it is missing or negative (no link to the master).
fn last_io_seconds(info: &[u8]) -> Option<f64> {
    String::from_utf8_lossy(info).lines().find_map(|line| {
        let value = line
            .trim_end()
            .strip_prefix("master_last_io_seconds_ago:")?;
        let n: i64 = value.parse().ok()?;
        (n >= 0).then_some(n as f64)
    })
}

/// Compare the first element of a `ROLE` reply with the wanted role; a
/// wanted replica must also have the link state `connected`.
fn role_outcome(want: Role, actual: &str, link: Option<&str>) -> CheckOutcome {
    let actual = match actual {
        "slave" => "replica",
        other => other,
    };
    if actual != want.name() {
        return CheckOutcome::Down(format!("role is {actual}, want {}", want.name()));
    }
    match (want, link) {
        (Role::Replica, Some("connected")) | (Role::Master | Role::Primary, _) => CheckOutcome::Up,
        (Role::Replica, Some(state)) => {
            CheckOutcome::Down(format!("replication: link to master is {state}"))
        }
        (Role::Replica, None) => {
            CheckOutcome::Down("query: ROLE answered a replica without a link state".into())
        }
    }
}

async fn call(
    conn: &mut Box<dyn Stream>,
    reader: &mut Reader,
    cmd: &[u8],
    prefix: &str,
) -> Result<Reply, CheckOutcome> {
    conn.write_all(cmd)
        .await
        .map_err(|e| io_failure(prefix, &e))?;
    conn.flush().await.map_err(|e| io_failure(prefix, &e))?;
    reader.read(conn).await.map_err(|e| match e {
        ReadError::Io(e) => io_failure(prefix, &e),
        ReadError::Closed => CheckOutcome::Down(format!("{prefix}connection closed")),
        ReadError::Protocol(p) => CheckOutcome::Down(format!("{prefix}invalid reply: {}", p.0)),
    })
}

/// A short description of a reply for a reason; never echoes bulk data.
fn describe(reply: &Reply) -> String {
    match reply {
        Reply::Simple(s) => format!("+{s}"),
        Reply::Error(e) => format!("-{e}"),
        Reply::Integer(n) => format!(":{n}"),
        Reply::Bulk(Some(b)) => format!("a bulk string of {} bytes", b.len()),
        Reply::Bulk(None) => "a null bulk string".into(),
        Reply::Array(Some(a)) => format!("an array of {} items", a.len()),
        Reply::Array(None) => "a null array".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resp::parse;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    /// A fake redis server: `answer` maps each command (its words) to the
    /// raw reply bytes.
    async fn fake_redis(answer: fn(&[String]) -> Vec<u8>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    loop {
                        while let Ok(Some((Reply::Array(Some(items)), used))) = parse(&buf) {
                            buf.drain(..used);
                            let words: Vec<String> = items
                                .iter()
                                .map(|i| match i {
                                    Reply::Bulk(Some(b)) => String::from_utf8_lossy(b).into_owned(),
                                    _ => String::new(),
                                })
                                .collect();
                            if s.write_all(&answer(&words)).await.is_err() {
                                return;
                            }
                        }
                        match s.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                });
            }
        });
        addr
    }

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest {
            service: "cache".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: Default::default(),
            timeout: Duration::from_secs(1),
        }
    }

    /// A server whose `ROLE` reply is `role` (`master`, `sentinel` or
    /// `slave/<link state>`) and whose `INFO replication` reports `last_io`.
    async fn role_server(role: &'static str, last_io: i64) -> SocketAddr {
        let role_reply = match role {
            "master" => b"*3\r\n$6\r\nmaster\r\n:0\r\n*0\r\n".to_vec(),
            "sentinel" => b"*2\r\n$8\r\nsentinel\r\n*0\r\n".to_vec(),
            state => {
                let state = state.strip_prefix("slave/").unwrap_or("connected");
                format!(
                    "*5\r\n$5\r\nslave\r\n$9\r\n127.0.0.1\r\n:6379\r\n${}\r\n{state}\r\n:1\r\n",
                    state.len()
                )
                .into_bytes()
            }
        };
        let info =
            format!("# Replication\r\nrole:slave\r\nmaster_last_io_seconds_ago:{last_io}\r\n");
        let info_reply = format!("${}\r\n{info}\r\n", info.len()).into_bytes();
        scripted_redis(role_reply, info_reply).await
    }

    /// A fake redis answering PING, ROLE and INFO with fixed replies.
    async fn scripted_redis(role_reply: Vec<u8>, info_reply: Vec<u8>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let (role_reply, info_reply) = (role_reply.clone(), info_reply.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    loop {
                        while let Ok(Some((Reply::Array(Some(items)), used))) = parse(&buf) {
                            buf.drain(..used);
                            let reply = match items.first() {
                                Some(Reply::Bulk(Some(w))) if w == b"PING" => b"+PONG\r\n".to_vec(),
                                Some(Reply::Bulk(Some(w))) if w == b"ROLE" => role_reply.clone(),
                                Some(Reply::Bulk(Some(w))) if w == b"INFO" => info_reply.clone(),
                                _ => b"-ERR unknown\r\n".to_vec(),
                            };
                            if s.write_all(&reply).await.is_err() {
                                return;
                            }
                        }
                        match s.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn redis_role_table() {
        let down = |r: &str| CheckOutcome::Down(r.into());
        let rows = [
            (None, None, "slave/connect", 3, CheckOutcome::Up),
            (Some(Role::Master), None, "master", -1, CheckOutcome::Up),
            (
                Some(Role::Master),
                None,
                "slave/connected",
                3,
                down("role is replica, want master"),
            ),
            (
                Some(Role::Replica),
                None,
                "slave/connected",
                3,
                CheckOutcome::Up,
            ),
            (
                Some(Role::Replica),
                None,
                "master",
                -1,
                down("role is master, want replica"),
            ),
            (
                Some(Role::Master),
                None,
                "sentinel",
                -1,
                down("role is sentinel, want master"),
            ),
            (
                Some(Role::Replica),
                None,
                "slave/sync",
                3,
                down("replication: link to master is sync"),
            ),
            (
                Some(Role::Replica),
                Some(15),
                "slave/connected",
                3,
                CheckOutcome::Up,
            ),
            (
                Some(Role::Replica),
                Some(15),
                "slave/connected",
                15,
                CheckOutcome::Up,
            ),
            (
                Some(Role::Replica),
                Some(15),
                "slave/connected",
                16,
                down("replication: 16s behind, max 15s"),
            ),
            (
                Some(Role::Replica),
                Some(15),
                "slave/connected",
                -1,
                down("replication: lag unknown"),
            ),
        ];
        for (role, max_lag, server_role, last_io, want) in rows {
            let addr = role_server(server_role, last_io).await;
            let settings = Settings {
                role,
                max_lag,
                ..Settings::default()
            };
            assert_eq!(
                check(&req(addr), &settings).await,
                want,
                "{role:?} {max_lag:?} vs {server_role} {last_io}"
            );
        }
    }

    #[tokio::test]
    async fn auth_with_user_and_password() {
        let addr = fake_redis(|w| match w {
            [a, u, p] if a == "AUTH" && u == "emfor" && p == "s3cret" => b"+OK\r\n".to_vec(),
            [a, ..] if a == "AUTH" => b"-WRONGPASS invalid username-password pair\r\n".to_vec(),
            [p] if p == "PING" => b"+PONG\r\n".to_vec(),
            _ => b"-ERR unknown\r\n".to_vec(),
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good");
        let bad = dir.path().join("bad");
        std::fs::write(&good, "s3cret\n").unwrap();
        std::fs::write(&bad, "wrong-pw\n").unwrap();
        let mut settings = Settings {
            user: Some("emfor".into()),
            password_file: Some(good),
            ..Settings::default()
        };
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);

        settings.password_file = Some(bad);
        match check(&req(addr), &settings).await {
            CheckOutcome::Down(r) => {
                assert!(r.starts_with("auth: "), "{r}");
                assert!(!r.contains("wrong-pw"), "{r}");
            }
            other => panic!("expected down, got {other:?}"),
        }

        settings.password_file = Some(dir.path().join("missing"));
        assert!(
            matches!(check(&req(addr), &settings).await, CheckOutcome::Error(r) if r.starts_with("password_file: "))
        );
    }

    #[tokio::test]
    async fn ping_must_answer_pong() {
        let addr = fake_redis(|_| b"-NOAUTH Authentication required.\r\n".to_vec()).await;
        assert_eq!(
            check(&req(addr), &Settings::default()).await,
            CheckOutcome::Down("query: NOAUTH Authentication required.".into())
        );
        let addr = fake_redis(|_| b"$4\r\nPONG\r\n".to_vec()).await;
        assert_eq!(
            check(&req(addr), &Settings::default()).await,
            CheckOutcome::Down("query: PING answered a bulk string of 4 bytes".into())
        );
        let addr = fake_redis(|_| b"!bogus\r\n".to_vec()).await;
        assert!(
            matches!(check(&req(addr), &Settings::default()).await, CheckOutcome::Down(r) if r.starts_with("query: invalid reply"))
        );
    }
}
