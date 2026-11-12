use super::testing::*;
use super::*;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

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
fn local_errors() {
    assert!(is_local_io_error(&io::Error::from_raw_os_error(
        libc::EMFILE
    )));
    assert!(!is_local_io_error(&io::Error::from_raw_os_error(
        libc::ECONNREFUSED
    )));
}
