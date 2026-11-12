//! Connections to backends: TCP, optionally wrapped in TLS. The TLS part
//! (trust, verification, handshake timing) is `emfor-tls`, shared with the
//! daemon's `tcp` checker.

use std::io;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use emfor_plugin::CheckOutcome;
use emfor_tls::{TlsFailure, TlsSettings};
use rustls::ClientConfig;
use rustls::pki_types::CertificateDer;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

pub use emfor_tls::is_local_io_error;

/// A connection to a backend: plain TCP or TLS over TCP.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// The check outcome for a TLS failure.
pub fn tls_outcome(f: TlsFailure) -> CheckOutcome {
    if f.local {
        CheckOutcome::Error(f.reason)
    } else {
        CheckOutcome::Down(f.reason)
    }
}

/// The outcome for a failed I/O step: `error` for a local failure,
/// `down` otherwise, with `prefix` in front of the message.
///
/// A TLS failure that a driver reports as an I/O error is `down` with a
/// reason starting `tls: `, whatever the step.
pub fn io_failure(prefix: &str, e: &io::Error) -> CheckOutcome {
    if let Some(inner) = e.get_ref()
        && inner.downcast_ref::<rustls::Error>().is_some()
    {
        return CheckOutcome::Down(format!("tls: {inner}"));
    }
    if is_local_io_error(e) {
        CheckOutcome::Error(format!("{prefix}{e}"))
    } else {
        CheckOutcome::Down(format!("{prefix}{e}"))
    }
}

/// Open a TCP connection to the backend.
pub async fn connect(ip: IpAddr, port: u16) -> Result<TcpStream, CheckOutcome> {
    let stream = TcpStream::connect((ip, port))
        .await
        .map_err(|e| io_failure("connect: ", &e))?;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// Open a connection to the backend, wrapped in TLS when `tls` is set.
/// The handshake gets its share of what is left of `timeout`, see
/// [`emfor_tls::handshake`].
pub async fn open(
    ip: IpAddr,
    port: u16,
    tls: Option<&TlsSettings>,
    timeout: Duration,
) -> Result<Box<dyn Stream>, CheckOutcome> {
    let start = Instant::now();
    let stream = connect(ip, port).await?;
    let Some(tls) = tls else {
        return Ok(Box::new(stream));
    };
    let remaining = timeout.saturating_sub(start.elapsed());
    let stream = emfor_tls::handshake(stream, tls, ip, remaining)
        .await
        .map_err(tls_outcome)?;
    Ok(Box::new(stream))
}

/// The rustls client configuration for one check, see
/// [`emfor_tls::client_config`].
pub fn client_config(tls: &TlsSettings) -> Result<std::sync::Arc<ClientConfig>, CheckOutcome> {
    emfor_tls::client_config(tls).map_err(tls_outcome)
}

/// The CA certificates to trust when verifying, see
/// [`emfor_tls::trusted_certificates`].
pub fn trusted_certificates(
    tls: &TlsSettings,
) -> Result<Vec<CertificateDer<'static>>, CheckOutcome> {
    emfor_tls::trusted_certificates(tls).map_err(tls_outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use emfor_tls::testing::*;
    use std::path::Path;
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

    async fn read_all(mut s: Box<dyn Stream>) -> String {
        let mut text = String::new();
        let _ = s.read_to_string(&mut text).await;
        text
    }

    #[tokio::test]
    async fn tls_with_the_right_ca_and_name() {
        let pki = pki("db.test");
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, &pki.ca_pem).unwrap();
        let addr = tls_server(pki.server_config, b"hi").await;

        let ok = open(
            addr.ip(),
            addr.port(),
            Some(&settings(Some(&ca), Some("db.test"), true)),
            T,
        )
        .await
        .unwrap();
        assert_eq!(read_all(ok).await, "hi");

        let wrong = open(
            addr.ip(),
            addr.port(),
            Some(&settings(Some(&ca), Some("other.test"), true)),
            T,
        )
        .await;
        match wrong {
            Err(CheckOutcome::Down(r)) => assert!(r.starts_with("tls: "), "{r}"),
            other => panic!("expected down, got {:?}", other.map(|_| ())),
        }

        let unverified = open(
            addr.ip(),
            addr.port(),
            Some(&settings(None, Some("other.test"), false)),
            T,
        )
        .await
        .unwrap();
        assert_eq!(read_all(unverified).await, "hi");

        // Default name is the IP, which the certificate does not list.
        let by_ip = open(
            addr.ip(),
            addr.port(),
            Some(&settings(Some(&ca), None, true)),
            T,
        )
        .await;
        assert!(matches!(by_ip, Err(CheckOutcome::Down(ref r)) if r.starts_with("tls: ")));
    }

    #[tokio::test]
    async fn tls_to_a_plaintext_server_is_down() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt;
                let _ = s.write_all(b"-ERR plaintext only\r\n").await;
            }
        });
        let r = open(
            addr.ip(),
            addr.port(),
            Some(&settings(None, None, false)),
            T,
        )
        .await;
        match r {
            Err(CheckOutcome::Down(r)) => assert!(r.starts_with("tls: "), "{r}"),
            other => panic!("expected down, got {:?}", other.map(|_| ())),
        }
    }

    #[tokio::test]
    async fn nothing_listening_is_down_connect() {
        // Retry with a fresh port if a parallel test takes the freed one.
        let mut outcome = None;
        for _ in 0..5 {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();
            drop(l);
            if let Err(o) = open(addr.ip(), addr.port(), None, T).await {
                outcome = Some(o);
                break;
            }
        }
        match outcome {
            Some(CheckOutcome::Down(r)) => assert!(r.starts_with("connect: "), "{r}"),
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[test]
    fn unreadable_ca_file_is_error() {
        let s = settings(Some(Path::new("/nonexistent/ca.pem")), None, true);
        match client_config(&s) {
            Err(CheckOutcome::Error(r)) => assert!(r.starts_with("ca_file: "), "{r}"),
            other => panic!("expected error, got {:?}", other.map(|_| ())),
        }
    }

    #[tokio::test]
    async fn tls_to_a_silent_plaintext_server_is_down_tls() {
        // Like redis on its plaintext port: reads, never answers.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = listener.accept().await {
                held.push(s);
            }
        });
        let r = open(
            addr.ip(),
            addr.port(),
            Some(&settings(None, None, false)),
            Duration::from_millis(300),
        )
        .await;
        match r {
            Err(CheckOutcome::Down(r)) => assert!(r.starts_with("tls: no answer"), "{r}"),
            other => panic!("expected down, got {:?}", other.map(|_| ())),
        }
    }

    #[tokio::test]
    async fn little_time_left_is_not_blamed_on_tls() {
        // 90% of 50 ms is below the minimum: no `tls: no answer` reason; the
        // check's own timeout (here, the outer timeout) ends it.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = listener.accept().await {
                held.push(s);
            }
        });
        let opened = tokio::time::timeout(
            Duration::from_millis(300),
            open(
                addr.ip(),
                addr.port(),
                Some(&settings(None, None, false)),
                Duration::from_millis(50),
            ),
        )
        .await;
        assert!(opened.is_err(), "open returned before the outer timeout");
    }

    #[test]
    fn tls_errors_inside_io_errors_are_tls() {
        let e = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(rustls::CertificateError::NotValidForName),
        );
        assert!(
            matches!(io_failure("connect: ", &e), CheckOutcome::Down(r) if r.starts_with("tls: "))
        );
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
}
