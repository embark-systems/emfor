use super::testing::*;
use super::*;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};

const T: Duration = Duration::from_secs(2);

fn settings(ca: Option<&Path>, name: Option<&str>, verify: bool) -> TlsSettings {
    TlsSettings {
        verify,
        server_name: name.map(str::to_owned),
        ca_file: ca.map(Path::to_path_buf),
    }
}

async fn connect_tls(
    addr: std::net::SocketAddr,
    tls: &TlsSettings,
    remaining: Duration,
) -> Result<TlsStream<TcpStream>, TlsFailure> {
    let stream = TcpStream::connect(addr).await.unwrap();
    handshake(stream, tls, addr.ip(), remaining).await
}

#[tokio::test]
async fn verification_by_ca_and_name() {
    let pki = pki("db.test");
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, &pki.ca_pem).unwrap();
    let addr = tls_server(pki.server_config, b"hi").await;

    let mut ok = connect_tls(addr, &settings(Some(&ca), Some("db.test"), true), T)
        .await
        .unwrap();
    let mut text = String::new();
    let _ = ok.read_to_string(&mut text).await;
    assert_eq!(text, "hi");

    let wrong = connect_tls(addr, &settings(Some(&ca), Some("other.test"), true), T).await;
    assert!(
        matches!(&wrong, Err(TlsFailure { local: false, reason }) if reason.starts_with("tls: ")),
        "{:?}",
        wrong.map(|_| ())
    );

    assert!(
        connect_tls(addr, &settings(None, Some("other.test"), false), T)
            .await
            .is_ok()
    );

    // Default name is the IP, which the certificate does not list.
    let by_ip = connect_tls(addr, &settings(Some(&ca), None, true), T).await;
    assert!(matches!(by_ip, Err(TlsFailure { local: false, .. })));
}

#[tokio::test]
async fn plaintext_server_is_down_tls() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            use tokio::io::AsyncWriteExt;
            let _ = s.write_all(b"-ERR plaintext only\r\n").await;
        }
    });
    match connect_tls(addr, &settings(None, None, false), T).await {
        Err(TlsFailure {
            local: false,
            reason,
        }) => assert!(reason.starts_with("tls: "), "{reason}"),
        other => panic!("expected down, got {:?}", other.map(|_| ())),
    }
}

/// A server that reads and never answers, like redis on its plaintext port.
async fn silent_server() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = listener.accept().await {
            held.push(s);
        }
    });
    addr
}

#[tokio::test]
async fn silent_plaintext_server_is_no_answer() {
    let addr = silent_server().await;
    match connect_tls(
        addr,
        &settings(None, None, false),
        Duration::from_millis(300),
    )
    .await
    {
        Err(TlsFailure {
            local: false,
            reason,
        }) => assert_eq!(
            reason,
            "tls: no answer to the TLS handshake (does the port speak TLS?)"
        ),
        other => panic!("expected down, got {:?}", other.map(|_| ())),
    }
}

/// A server that reads the ClientHello and then closes the connection, or
/// resets it with `reset`, as Active Directory does on 636 when it has no
/// certificate.
async fn closing_server(reset: bool) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            // The whole ClientHello record: closing with unread bytes
            // would make the close a reset.
            let mut header = [0u8; 5];
            s.read_exact(&mut header).await.unwrap();
            let len = usize::from(u16::from_be_bytes([header[3], header[4]]));
            s.read_exact(&mut vec![0u8; len]).await.unwrap();
            if reset {
                s.set_zero_linger().unwrap();
            }
        }
    });
    addr
}

#[tokio::test]
async fn closed_during_the_handshake() {
    for reset in [false, true] {
        let addr = closing_server(reset).await;
        match connect_tls(addr, &settings(None, None, false), T).await {
            Err(TlsFailure {
                local: false,
                reason,
            }) => assert_eq!(reason, CLOSED_DURING_HANDSHAKE, "reset = {reset}"),
            other => panic!(
                "reset = {reset}: expected down, got {:?}",
                other.map(|_| ())
            ),
        }
    }
}

#[tokio::test]
async fn little_time_left_is_not_blamed_on_tls() {
    // 90% of 50 ms is below the minimum: no `tls: no answer` reason; the
    // check's own timeout (here, the outer timeout) ends it.
    let addr = silent_server().await;
    let opened = tokio::time::timeout(
        Duration::from_millis(300),
        connect_tls(
            addr,
            &settings(None, None, false),
            Duration::from_millis(50),
        ),
    )
    .await;
    assert!(
        opened.is_err(),
        "handshake returned before the outer timeout"
    );
}

#[test]
fn unreadable_ca_file_is_local() {
    let s = settings(Some(Path::new("/nonexistent/ca.pem")), None, true);
    match client_config(&s) {
        Err(TlsFailure {
            local: true,
            reason,
        }) => assert!(reason.starts_with("ca_file: "), "{reason}"),
        other => panic!("expected local failure, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn option_checks() {
    assert_eq!(check_server_name("db.test"), Ok(()));
    assert_eq!(check_server_name("10.0.0.1"), Ok(()));
    assert_eq!(
        check_server_name("not a name"),
        Err("not a valid host name or IP address")
    );

    let dir = tempfile::tempdir().unwrap();
    let pem = dir.path().join("ca.pem");
    std::fs::write(&pem, pki("x.test").ca_pem).unwrap();
    let text = dir.path().join("not-pem.txt");
    std::fs::write(&text, "hello").unwrap();
    assert_eq!(check_ca_file(&pem), Ok(()));
    assert_eq!(
        check_ca_file(Path::new("ca.pem")),
        Err("not an absolute path".into())
    );
    assert_eq!(
        check_ca_file(&dir.path().join("missing.pem")),
        Err("file not found".into())
    );
    assert_eq!(check_ca_file(&text), Err("no certificate found".into()));
}

#[test]
fn ca_file_is_parsed_again_only_when_it_changes() {
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, pki("a.test").ca_pem).unwrap();
    let first = roots_from_file(&ca).unwrap();
    assert!(
        Arc::ptr_eq(&first, &roots_from_file(&ca).unwrap()),
        "an unchanged ca_file is not parsed again"
    );
    // Another CA, plus a comment so the size differs whatever the clock's
    // resolution.
    let other = format!("# rotated\n{}", pki("b.test").ca_pem);
    std::fs::write(&ca, &other).unwrap();
    let second = roots_from_file(&ca).unwrap();
    assert!(!Arc::ptr_eq(&first, &second), "a rewritten ca_file is read");
    assert_eq!(
        trusted_certificates(&settings(Some(&ca), None, true)).unwrap(),
        load_certificates(other.as_bytes())
    );
    // Replaced by rename, as certificate tools do.
    let next = dir.path().join("next.pem");
    std::fs::write(&next, pki("c.test").ca_pem).unwrap();
    std::fs::rename(&next, &ca).unwrap();
    assert!(!Arc::ptr_eq(&second, &roots_from_file(&ca).unwrap()));
    // A removed file is an error again, not the remembered certificates.
    std::fs::remove_file(&ca).unwrap();
    assert!(matches!(
        roots_from_file(&ca),
        Err(TlsFailure { local: true, .. })
    ));
}

#[test]
fn local_errors() {
    assert!(is_local_io_error(&io::Error::from_raw_os_error(
        libc::EMFILE
    )));
    assert!(!is_local_io_error(&io::Error::from_raw_os_error(
        libc::ECONNREFUSED
    )));
    // Spec health-checking "TCP checker": no route, or this host's firewall,
    // is `down`: the host's applications cannot reach the backend either.
    for errno in [
        libc::ENETUNREACH,
        libc::EHOSTUNREACH,
        libc::EPERM,
        libc::EACCES,
    ] {
        assert!(!is_local_io_error(&io::Error::from_raw_os_error(errno)));
    }
}

#[tokio::test]
async fn client_certificate_when_the_server_asks() {
    let pki = mutual_pki("dra.test", "emfor.test");
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name);
    std::fs::write(path("ca.pem"), &pki.ca_pem).unwrap();
    std::fs::write(path("client.crt"), &pki.client_cert_pem).unwrap();
    std::fs::write(path("client.key"), &pki.client_key_pem).unwrap();
    let addr = tls_server(pki.server_config, b"hi").await;
    let tls = settings(Some(&path("ca.pem")), Some("dra.test"), true);
    let client = ClientCertificate {
        cert_file: path("client.crt"),
        key_file: path("client.key"),
    };

    let read = |config: Arc<ClientConfig>| {
        let tls = tls.clone();
        async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let mut s = handshake_with_config(stream, config, &tls, addr.ip(), T).await?;
            let mut text = String::new();
            let _ = s.read_to_string(&mut text).await;
            Ok::<_, TlsFailure>(text)
        }
    };
    let with = client_config_with_client(&tls, Some(&client)).unwrap();
    assert_eq!(read(with).await.unwrap(), "hi");
    // Without one the server rejects the connection: TLS 1.3 completes the
    // handshake on the client side first, so nothing arrives.
    let without = client_config(&tls).unwrap();
    assert_ne!(read(without).await.unwrap_or_default(), "hi");
}

#[test]
fn client_certificate_files() {
    let pki = mutual_pki("dra.test", "emfor.test");
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name);
    std::fs::write(path("client.crt"), &pki.client_cert_pem).unwrap();
    std::fs::write(path("client.key"), &pki.client_key_pem).unwrap();
    std::fs::write(path("empty"), "").unwrap();

    assert_eq!(check_cert_file(&path("client.crt")), Ok(()));
    assert_eq!(check_key_file(&path("client.key")), Ok(()));
    assert_eq!(
        check_cert_file(&path("client.key")),
        Err("no certificate found".into())
    );
    assert_eq!(
        check_key_file(&path("client.crt")),
        Err("no private key found".into())
    );
    assert_eq!(
        check_key_file(&path("missing")),
        Err("file not found".into())
    );
    assert_eq!(
        check_key_file(Path::new("client.key")),
        Err("not an absolute path".into())
    );

    let tls = settings(None, None, false);
    let config = |cert: &str, key: &str| {
        client_config_with_client(
            &tls,
            Some(&ClientCertificate {
                cert_file: path(cert),
                key_file: path(key),
            }),
        )
        .map(|_| ())
    };
    assert_eq!(config("client.crt", "client.key"), Ok(()));
    let local = |reason: &str| Err(TlsFailure::local(reason));
    assert_eq!(
        config("missing", "client.key"),
        local("tls_cert_file: entity not found")
    );
    assert_eq!(
        config("empty", "client.key"),
        local("tls_cert_file: no certificate found")
    );
    assert_eq!(
        config("client.crt", "missing"),
        local("tls_key_file: entity not found")
    );
    assert_eq!(
        config("client.crt", "empty"),
        local("tls_key_file: no private key found")
    );
}
