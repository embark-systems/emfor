//! The `ldap` plugin: an LDAPv3 simple bind, anonymous or with `user` and
//! `password_file`, over plaintext, LDAPS (`tls`) or StartTLS (`starttls`).

use std::io;
use std::time::Instant;

use emfor_plugin::{CheckOutcome, CheckRequest};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::ber::{self, BIND_RESPONSE, EXTENDED_RESPONSE, Response};
use crate::net::{self, Stream, io_failure};
use crate::options::{Settings, read_password};

/// Check one LDAP server with validated settings.
pub async fn check(req: &CheckRequest, settings: &Settings) -> CheckOutcome {
    let password = match &settings.password_file {
        Some(path) => match read_password(path) {
            Ok(p) => p,
            Err(reason) => return CheckOutcome::Error(reason),
        },
        None => String::new(),
    };
    let user = settings.user.as_deref().unwrap_or("");
    let (mut conn, bind_id) = match connect(req, settings).await {
        Ok(c) => c,
        Err(outcome) => return outcome,
    };
    let outcome = bind(&mut conn, bind_id, user, &password).await;
    if outcome == CheckOutcome::Up {
        let _ = conn.write_all(&ber::unbind_request(bind_id + 1)).await;
    }
    let _ = conn.shutdown().await;
    outcome
}

/// Open the connection; with `starttls`, upgrade it first. Returns the
/// connection and the messageID to bind with.
async fn connect(
    req: &CheckRequest,
    settings: &Settings,
) -> Result<(Box<dyn Stream>, i32), CheckOutcome> {
    if !settings.starttls {
        let conn = net::open(req.ip, req.port, settings.tls.as_ref(), req.timeout).await?;
        return Ok((conn, 1));
    }
    let start = Instant::now();
    let mut tcp = net::connect(req.ip, req.port).await?;
    starttls(&mut tcp).await?;
    let tls = settings
        .tls
        .as_ref()
        .expect("options give starttls its TLS settings");
    let remaining = req.timeout.saturating_sub(start.elapsed());
    let conn = emfor_tls::handshake(tcp, tls, req.ip, remaining)
        .await
        .map_err(net::tls_outcome)?;
    Ok((Box::new(conn), 2))
}

/// Ask for StartTLS (messageID 1); anything but success is `tls: `.
async fn starttls(conn: &mut (impl AsyncRead + AsyncWrite + Unpin)) -> Result<(), CheckOutcome> {
    let refused = |detail: String| CheckOutcome::Down(format!("tls: StartTLS refused: {detail}"));
    if let Err(e) = conn.write_all(&ber::starttls_request(1)).await {
        return Err(io_failure("connect: ", &e));
    }
    match read_response(conn).await {
        Ok(r) if r.op != EXTENDED_RESPONSE => {
            Err(refused(format!("unexpected response (tag 0x{:02x})", r.op)))
        }
        Ok(r) if r.message_id == 0 => Err(refused(format!(
            "notice of disconnection: {}",
            ber::result_text(r.result_code, &r.diagnostic)
        ))),
        Ok(r) if r.message_id != 1 => Err(refused(format!(
            "response for messageID {}, want 1",
            r.message_id
        ))),
        Ok(r) if r.result_code != 0 => Err(refused(ber::result_text(r.result_code, &r.diagnostic))),
        Ok(_) => Ok(()),
        Err(Failure::Io(e)) => Err(io_failure("connect: ", &e)),
        Err(Failure::Closed) => Err(refused("connection closed".into())),
        Err(Failure::Bad(e)) => Err(refused(e.0)),
    }
}

/// Send the BindRequest and judge the BindResponse.
async fn bind(
    conn: &mut (impl AsyncRead + AsyncWrite + Unpin),
    id: i32,
    user: &str,
    password: &str,
) -> CheckOutcome {
    if let Err(e) = conn.write_all(&ber::bind_request(id, user, password)).await {
        return io_failure("connect: ", &e);
    }
    let r = match read_response(conn).await {
        Ok(r) => r,
        Err(Failure::Io(e)) => return io_failure("connect: ", &e),
        Err(Failure::Closed) => {
            return CheckOutcome::Down("connect: connection closed by the server".into());
        }
        Err(Failure::Bad(e)) => return CheckOutcome::Down(format!("query: {}", e.0)),
    };
    if r.message_id == 0 && r.op == EXTENDED_RESPONSE {
        return CheckOutcome::Down(format!(
            "query: notice of disconnection: {}",
            ber::result_text(r.result_code, &r.diagnostic)
        ));
    }
    if r.op != BIND_RESPONSE {
        return CheckOutcome::Down(format!(
            "query: unexpected response (tag 0x{:02x}) to the bind",
            r.op
        ));
    }
    if r.message_id != i64::from(id) {
        return CheckOutcome::Down(format!(
            "query: response for messageID {}, want {id}",
            r.message_id
        ));
    }
    match r.result_code {
        0 => CheckOutcome::Up,
        code => CheckOutcome::Down(format!("auth: {}", ber::result_text(code, &r.diagnostic))),
    }
}

enum Failure {
    Io(io::Error),
    /// The server closed the connection before a whole message arrived.
    Closed,
    Bad(ber::BerError),
}

/// Read one whole LDAPMessage and decode it.
async fn read_response(conn: &mut (impl AsyncRead + Unpin)) -> Result<Response, Failure> {
    let mut buf = Vec::with_capacity(64);
    let (header, len) = loop {
        if let Some(h) = ber::message_header(&buf).map_err(Failure::Bad)? {
            break h;
        }
        let mut byte = [0u8; 1];
        match conn.read(&mut byte).await {
            Ok(0) => return Err(Failure::Closed),
            Ok(_) => buf.push(byte[0]),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(Failure::Closed),
            Err(e) => return Err(Failure::Io(e)),
        }
    };
    buf.resize(header + len, 0);
    match conn.read_exact(&mut buf[header..]).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(Failure::Closed),
        Err(e) => return Err(Failure::Io(e)),
    }
    ber::decode_response(&buf).map_err(Failure::Bad)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::TlsSettings;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::net::TcpListener;

    /// What a fake server answers to each request it reads.
    type Script = Vec<Vec<u8>>;

    /// A fake LDAP server for one connection: for each entry in `answers`
    /// it reads one request and writes the entry (an empty entry closes the
    /// connection). Returns the requests it read.
    async fn fake_ldap(answers: Script) -> (SocketAddr, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut requests = Vec::new();
            for answer in answers {
                let Some(request) = read_request(&mut s).await else {
                    break;
                };
                requests.push(request);
                if answer.is_empty() {
                    return requests;
                }
                s.write_all(&answer).await.unwrap();
            }
            // Collect anything else (an unbind) until the client closes.
            while let Some(request) = read_request(&mut s).await {
                requests.push(request);
            }
            requests
        });
        (addr, task)
    }

    async fn read_request(s: &mut tokio::net::TcpStream) -> Option<Vec<u8>> {
        let mut buf = Vec::new();
        loop {
            if let Ok(Some((_, len))) = ber::message_header(&buf) {
                let mut rest = vec![0; len];
                s.read_exact(&mut rest).await.ok()?;
                buf.extend(rest);
                return Some(buf);
            }
            let mut b = [0u8; 1];
            if s.read(&mut b).await.ok()? == 0 {
                return None;
            }
            buf.push(b[0]);
        }
    }

    fn req(addr: SocketAddr) -> CheckRequest {
        CheckRequest {
            service: "ldap".into(),
            ip: addr.ip(),
            port: addr.port(),
            options: Default::default(),
            timeout: Duration::from_secs(2),
        }
    }

    #[tokio::test]
    async fn anonymous_bind_is_up_and_unbinds() {
        let (addr, task) = fake_ldap(vec![ber::response(1, BIND_RESPONSE, 0, "")]).await;
        assert_eq!(
            check(&req(addr), &Settings::default()).await,
            CheckOutcome::Up
        );
        let requests = task.await.unwrap();
        assert_eq!(requests[0], ber::bind_request(1, "", ""));
        assert_eq!(requests[1], ber::unbind_request(2));
    }

    #[tokio::test]
    async fn bind_uses_user_and_password_file() {
        let dir = tempfile::tempdir().unwrap();
        let pass = dir.path().join("pass");
        std::fs::write(&pass, "s3cret\n").unwrap();
        let settings = Settings {
            user: Some("cn=monitor,dc=example,dc=org".into()),
            password_file: Some(pass),
            ..Settings::default()
        };
        let (addr, task) = fake_ldap(vec![ber::response(1, BIND_RESPONSE, 0, "")]).await;
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
        assert_eq!(
            task.await.unwrap()[0],
            ber::bind_request(1, "cn=monitor,dc=example,dc=org", "s3cret")
        );
    }

    #[tokio::test]
    async fn rejected_binds_name_the_result_code() {
        let rows = [
            (49, "", "auth: 49 invalidCredentials"),
            (
                53,
                "unauthenticated bind (DN with no password) disallowed",
                "auth: 53 unwillingToPerform: unauthenticated bind (DN with no password) disallowed",
            ),
            (8, "", "auth: 8 strongerAuthRequired"),
            (4096, "", "auth: 4096"),
        ];
        for (code, diagnostic, reason) in rows {
            let (addr, _task) =
                fake_ldap(vec![ber::response(1, BIND_RESPONSE, code, diagnostic)]).await;
            assert_eq!(
                check(&req(addr), &Settings::default()).await,
                CheckOutcome::Down(reason.into())
            );
        }
    }

    #[tokio::test]
    async fn starttls_refused_sends_no_bind() {
        let settings = Settings {
            starttls: true,
            tls: Some(TlsSettings {
                verify: false,
                server_name: None,
                ca_file: None,
            }),
            ..Settings::default()
        };
        let (addr, task) = fake_ldap(vec![ber::response(1, EXTENDED_RESPONSE, 2, "")]).await;
        assert_eq!(
            check(&req(addr), &settings).await,
            CheckOutcome::Down("tls: StartTLS refused: 2 protocolError".into())
        );
        let requests = task.await.unwrap();
        assert_eq!(requests, vec![ber::starttls_request(1)]);
    }

    #[tokio::test]
    async fn starttls_answer_for_another_message_is_refused() {
        let settings = Settings {
            starttls: true,
            tls: Some(TlsSettings {
                verify: false,
                server_name: None,
                ca_file: None,
            }),
            ..Settings::default()
        };
        let rows = [
            (
                5,
                0,
                "tls: StartTLS refused: response for messageID 5, want 1",
            ),
            (
                0,
                0,
                "tls: StartTLS refused: notice of disconnection: 0 success",
            ),
        ];
        for (id, code, reason) in rows {
            let (addr, task) =
                fake_ldap(vec![ber::response(id, EXTENDED_RESPONSE, code, "")]).await;
            assert_eq!(
                check(&req(addr), &settings).await,
                CheckOutcome::Down(reason.into())
            );
            assert_eq!(task.await.unwrap(), vec![ber::starttls_request(1)]);
        }
    }

    #[tokio::test]
    async fn starttls_then_bind_over_tls() {
        use emfor_tls::testing::pki;
        use tokio_rustls::TlsAcceptor;
        let pki = pki("ldap.test");
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, &pki.ca_pem).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(pki.server_config);
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let starttls = read_request(&mut s).await.unwrap();
            s.write_all(&ber::response(1, EXTENDED_RESPONSE, 0, ""))
                .await
                .unwrap();
            let mut tls = acceptor.accept(s).await.unwrap();
            let mut buf = vec![0u8; ber::bind_request(2, "", "").len()];
            tls.read_exact(&mut buf).await.unwrap();
            tls.write_all(&ber::response(2, BIND_RESPONSE, 0, ""))
                .await
                .unwrap();
            let mut rest = Vec::new();
            let _ = tls.read_to_end(&mut rest).await;
            (starttls, buf)
        });
        let settings = Settings {
            starttls: true,
            tls: Some(TlsSettings {
                verify: true,
                server_name: Some("ldap.test".into()),
                ca_file: Some(ca),
            }),
            ..Settings::default()
        };
        assert_eq!(check(&req(addr), &settings).await, CheckOutcome::Up);
        let (starttls, bind) = server.await.unwrap();
        assert_eq!(starttls, ber::starttls_request(1));
        assert_eq!(bind, ber::bind_request(2, "", ""));
    }

    #[tokio::test]
    async fn malformed_response_is_query() {
        let (addr, _task) = fake_ldap(vec![b"HTTP/1.1 400 Bad Request\r\n\r\n".to_vec()]).await;
        match check(&req(addr), &Settings::default()).await {
            CheckOutcome::Down(r) => assert!(r.starts_with("query: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
        // A response for another messageID.
        let (addr, _task) = fake_ldap(vec![ber::response(7, BIND_RESPONSE, 0, "")]).await;
        match check(&req(addr), &Settings::default()).await {
            CheckOutcome::Down(r) => assert!(r.starts_with("query: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn notice_of_disconnection_is_query() {
        let (addr, _task) = fake_ldap(vec![ber::response(
            0,
            EXTENDED_RESPONSE,
            52,
            "shutting down",
        )])
        .await;
        assert_eq!(
            check(&req(addr), &Settings::default()).await,
            CheckOutcome::Down(
                "query: notice of disconnection: 52 unavailable: shutting down".into()
            )
        );
    }

    #[tokio::test]
    async fn closed_before_answering_is_connect() {
        let (addr, _task) = fake_ldap(vec![Vec::new()]).await;
        match check(&req(addr), &Settings::default()).await {
            CheckOutcome::Down(r) => assert!(r.starts_with("connect: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }
}
