//! Connecting to a backend, with or without TLS, and sorting failures into
//! `down` (the backend's fault) and `error` (this host's fault).

use std::io;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use emfor_plugin::CheckOutcome;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::options::{TlsSettings, load_certificates};

/// Below this, a TLS handshake without an answer is left to the check's
/// timeout instead of being reported as a port that does not speak TLS.
const MIN_HANDSHAKE_BUDGET: Duration = Duration::from_millis(100);

/// A connection to a backend: plain TCP or TLS over TCP.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// Whether an I/O error comes from this host running out of something
/// rather than from the backend.
pub fn is_local_io_error(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM | libc::EADDRNOTAVAIL)
    )
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
///
/// The TLS handshake may use 90% of what is left of `timeout`: a plaintext
/// server often waits for more bytes instead of answering, and that has to
/// end as a `tls: ` failure before the whole check times out. When less than
/// 100 ms would be left for it, the check's own timeout applies instead.
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
    let config = client_config(tls)?;
    let name = server_name(tls, ip);
    let handshake = TlsConnector::from(config).connect(name, stream);
    // With little time left a missing answer says nothing about TLS: let
    // the check's own timeout answer `timeout` instead.
    let budget = timeout.saturating_sub(start.elapsed()) * 9 / 10;
    let budget = if budget >= MIN_HANDSHAKE_BUDGET {
        budget
    } else {
        Duration::MAX
    };
    let stream = match tokio::time::timeout(budget, handshake).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) if is_local_io_error(&e) => {
            return Err(CheckOutcome::Error(format!("tls: {e}")));
        }
        Ok(Err(e)) => return Err(CheckOutcome::Down(format!("tls: {e}"))),
        Err(_) => {
            return Err(CheckOutcome::Down(
                "tls: no answer to the TLS handshake (does the port speak TLS?)".into(),
            ));
        }
    };
    Ok(Box::new(stream))
}

/// The name the server certificate has to match and that is sent as SNI:
/// `server_name`, or the backend IP.
pub fn server_name(tls: &TlsSettings, ip: IpAddr) -> ServerName<'static> {
    match &tls.server_name {
        // Checked by `options::parse`.
        Some(name) => {
            ServerName::try_from(name.clone()).unwrap_or(ServerName::IpAddress(ip.into()))
        }
        None => ServerName::IpAddress(ip.into()),
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// The rustls client configuration for one check: trust `ca_file`, or the
/// system's CA certificates, or anything when `tls_verify = false`.
///
/// An unreadable `ca_file` or a host without CA certificates is `error`.
pub fn client_config(tls: &TlsSettings) -> Result<Arc<ClientConfig>, CheckOutcome> {
    let provider = provider();
    let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|e| CheckOutcome::Error(format!("tls: {e}")))?;
    let config = if !tls.verify {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate(provider)))
            .with_no_client_auth()
    } else {
        let roots = match &tls.ca_file {
            Some(path) => Arc::new(roots_from_file(path)?),
            None => system_roots()?,
        };
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    Ok(Arc::new(config))
}

/// The CA certificates to trust when verifying: those in `ca_file`, or the
/// system's. Used by drivers that take certificates instead of a config.
pub fn trusted_certificates(
    tls: &TlsSettings,
) -> Result<Vec<CertificateDer<'static>>, CheckOutcome> {
    match &tls.ca_file {
        Some(path) => Ok(read_ca_file(path)?),
        None => Ok(system_roots_der()?.clone()),
    }
}

fn read_ca_file(path: &Path) -> Result<Vec<CertificateDer<'static>>, CheckOutcome> {
    let bytes =
        std::fs::read(path).map_err(|e| CheckOutcome::Error(format!("ca_file: {}", e.kind())))?;
    let certs = load_certificates(&bytes);
    if certs.is_empty() {
        return Err(CheckOutcome::Error("ca_file: no certificate found".into()));
    }
    Ok(certs)
}

fn roots_from_file(path: &Path) -> Result<RootCertStore, CheckOutcome> {
    let mut roots = RootCertStore::empty();
    let (added, _) = roots.add_parsable_certificates(read_ca_file(path)?);
    if added == 0 {
        return Err(CheckOutcome::Error("ca_file: no usable certificate".into()));
    }
    Ok(roots)
}

/// The system's CA certificates, loaded once per process.
fn system_roots_der() -> Result<&'static Vec<CertificateDer<'static>>, CheckOutcome> {
    static CERTS: OnceLock<Vec<CertificateDer<'static>>> = OnceLock::new();
    let certs = CERTS.get_or_init(|| rustls_native_certs::load_native_certs().certs);
    if certs.is_empty() {
        return Err(CheckOutcome::Error(
            "tls: no system CA certificates found; set ca_file".into(),
        ));
    }
    Ok(certs)
}

fn system_roots() -> Result<Arc<RootCertStore>, CheckOutcome> {
    static ROOTS: OnceLock<Arc<RootCertStore>> = OnceLock::new();
    let certs = system_roots_der()?;
    Ok(Arc::clone(ROOTS.get_or_init(|| {
        let mut roots = RootCertStore::empty();
        roots.add_parsable_certificates(certs.iter().cloned());
        Arc::new(roots)
    })))
}

/// `tls_verify = false`: accept any certificate, but still check the
/// handshake signatures so the connection is encrypted with the server
/// that holds the presented certificate's key.
#[derive(Debug)]
struct AcceptAnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! A local TLS server with a throwaway CA, for tests.

    use std::net::SocketAddr;
    use std::sync::Arc;

    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
    use rustls::ServerConfig;
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    /// A CA (as PEM) and a server certificate for `name`, signed by it.
    pub struct Pki {
        pub ca_pem: String,
        pub server_config: Arc<ServerConfig>,
    }

    pub fn pki(name: &str) -> Pki {
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();
        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec![name.to_owned()])
            .unwrap()
            .signed_by(&key, &ca)
            .unwrap();
        let server_config = ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
        .unwrap();
        Pki {
            ca_pem: ca.pem(),
            server_config: Arc::new(server_config),
        }
    }

    /// Serve TLS on a local port: each connection gets `greeting` after the
    /// handshake, then is closed.
    pub async fn tls_server(config: Arc<ServerConfig>, greeting: &'static [u8]) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(config);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut tls) = acceptor.accept(stream).await {
                        let _ = tls.write_all(greeting).await;
                        let _ = tls.shutdown().await;
                    }
                });
            }
        });
        addr
    }
}

#[cfg(test)]
mod tests {
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
