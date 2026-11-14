//! TLS for emfor checks, shared by the daemon's `tcp` checker and
//! `emfor-check`: which certificates to trust, how to verify the server,
//! and how long a handshake may take.
//!
//! Failures come back as [`TlsFailure`], which each caller turns into its
//! own check result; option checks return the error message for the key.

use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// Below this, a TLS handshake without an answer is left to the check's
/// timeout instead of being reported as a port that does not speak TLS.
pub const MIN_HANDSHAKE_BUDGET: Duration = Duration::from_millis(100);

/// TLS settings of one service; present only with `tls = true`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsSettings {
    /// `tls_verify`.
    pub verify: bool,
    /// `server_name`; the backend IP when not set.
    pub server_name: Option<String>,
    /// `ca_file`; the system's CA certificates when not set.
    pub ca_file: Option<PathBuf>,
}

/// A client certificate chain and its private key, PEM files that are read
/// every time a client configuration is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCertificate {
    /// `tls_cert_file`.
    pub cert_file: PathBuf,
    /// `tls_key_file`.
    pub key_file: PathBuf,
}

/// A TLS step that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsFailure {
    /// The problem is on this host (an unreadable `ca_file`, no system CA
    /// certificates, out of resources), not the backend's: an `error`
    /// result rather than `down`.
    pub local: bool,
    /// The check reason, starting `tls: `, `ca_file: `, `tls_cert_file: `
    /// or `tls_key_file: `.
    pub reason: String,
}

impl TlsFailure {
    fn local(reason: impl Into<String>) -> Self {
        Self {
            local: true,
            reason: reason.into(),
        }
    }

    fn down(reason: impl Into<String>) -> Self {
        Self {
            local: false,
            reason: reason.into(),
        }
    }
}

/// Whether an I/O error comes from this host running out of something
/// rather than from the backend.
///
/// No route to the backend (`ENETUNREACH`, `EHOSTUNREACH`) and a connection
/// this host's firewall blocks (`EPERM`, `EACCES`) are deliberately not
/// local: the applications on this host cannot reach the backend either, so
/// it is `down` for them (spec health-checking, "TCP checker").
pub fn is_local_io_error(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM | libc::EADDRNOTAVAIL)
    )
}

/// The reason when the backend closes or resets the connection before the
/// handshake is done, for example a server that listens on a TLS port but
/// has no certificate (Active Directory on 636 without one).
pub const CLOSED_DURING_HANDSHAKE: &str =
    "tls: connection closed during the TLS handshake (does the server have a certificate?)";

/// Whether `e` means the other end closed or reset the connection.
fn is_closed(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    )
}

/// TLS handshake on a connected `stream` to `ip`.
///
/// The handshake may use 90% of what is left of `remaining` (the time left
/// in the check when called) after loading the trusted certificates: a
/// plaintext server often waits for more bytes instead of answering, and
/// that has to end as a `tls: ` failure before the whole check times out.
/// When less than [`MIN_HANDSHAKE_BUDGET`] would be left for it, the check's
/// own timeout applies instead.
pub async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    tls: &TlsSettings,
    ip: IpAddr,
    remaining: Duration,
) -> Result<TlsStream<S>, TlsFailure> {
    let start = Instant::now();
    let config = client_config(tls)?;
    handshake_with_config(
        stream,
        config,
        tls,
        ip,
        remaining.saturating_sub(start.elapsed()),
    )
    .await
}

/// [`handshake`] with a configuration built beforehand, for example by
/// [`client_config_with_client`] before connecting; `tls` gives the server
/// name. The budget rule is the same, from `remaining` as given.
pub async fn handshake_with_config<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    config: Arc<ClientConfig>,
    tls: &TlsSettings,
    ip: IpAddr,
    remaining: Duration,
) -> Result<TlsStream<S>, TlsFailure> {
    let handshake = TlsConnector::from(config).connect(server_name(tls, ip), stream);
    let budget = remaining * 9 / 10;
    let budget = if budget >= MIN_HANDSHAKE_BUDGET {
        budget
    } else {
        Duration::MAX
    };
    match tokio::time::timeout(budget, handshake).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) if is_local_io_error(&e) => Err(TlsFailure::local(format!("tls: {e}"))),
        Ok(Err(e)) if is_closed(&e) => Err(TlsFailure::down(CLOSED_DURING_HANDSHAKE)),
        Ok(Err(e)) => Err(TlsFailure::down(format!("tls: {e}"))),
        Err(_) => Err(TlsFailure::down(
            "tls: no answer to the TLS handshake (does the port speak TLS?)",
        )),
    }
}

/// The name the server certificate has to match and that is sent as SNI:
/// `server_name`, or the backend IP.
pub fn server_name(tls: &TlsSettings, ip: IpAddr) -> ServerName<'static> {
    match &tls.server_name {
        // Checked by `check_server_name` when the options were validated.
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
/// `ca_file` is parsed again whenever it changed since the last call (its
/// inode, size, modification or change time), so a replaced file takes
/// effect without a reload; an unchanged one costs one `stat`. An unreadable `ca_file` or a host without CA certificates is a
/// local failure.
pub fn client_config(tls: &TlsSettings) -> Result<Arc<ClientConfig>, TlsFailure> {
    client_config_with_client(tls, None)
}

/// [`client_config`] that presents `client` when the server asks for a
/// client certificate. Its files are read on every call; one that cannot
/// be read, or holds no certificate or no private key, is a local failure
/// (`tls_cert_file: ` or `tls_key_file: `).
pub fn client_config_with_client(
    tls: &TlsSettings,
    client: Option<&ClientCertificate>,
) -> Result<Arc<ClientConfig>, TlsFailure> {
    let client = client.map(read_client_certificate).transpose()?;
    let provider = provider();
    let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|e| TlsFailure::local(format!("tls: {e}")))?;
    let builder = if !tls.verify {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate(provider)))
    } else {
        let roots = match &tls.ca_file {
            Some(path) => roots_from_file(path)?,
            None => system_roots()?,
        };
        builder.with_root_certificates(roots)
    };
    let config = match client {
        None => builder.with_no_client_auth(),
        Some((certs, key)) => builder
            .with_client_auth_cert(certs, key)
            .map_err(|e| TlsFailure::local(format!("tls_key_file: {e}")))?,
    };
    Ok(Arc::new(config))
}

/// The CA certificates to trust when verifying: those in `ca_file`, or the
/// system's. For drivers that take certificates instead of a config.
pub fn trusted_certificates(tls: &TlsSettings) -> Result<Vec<CertificateDer<'static>>, TlsFailure> {
    match &tls.ca_file {
        Some(path) => read_ca_file(path),
        None => Ok(system_roots_der()?.clone()),
    }
}

/// The PEM certificates in `bytes`; anything else is skipped.
pub fn load_certificates(bytes: &[u8]) -> Vec<CertificateDer<'static>> {
    CertificateDer::pem_slice_iter(bytes)
        .filter_map(Result::ok)
        .collect()
}

/// Validate a `server_name` value; the error is the option's message.
pub fn check_server_name(name: &str) -> Result<(), &'static str> {
    match ServerName::try_from(name) {
        Ok(_) => Ok(()),
        Err(_) => Err("not a valid host name or IP address"),
    }
}

/// Validate a `ca_file` value by reading and parsing it as a check does
/// ([`parse_ca`]): it must hold a certificate usable as a trust anchor.
/// The error is the option's message.
pub fn check_ca_file(path: &Path) -> Result<(), String> {
    match parse_ca(&read_file_option(path)?)? {
        (_, Some(_)) => Ok(()),
        (_, None) => Err("no usable certificate".into()),
    }
}

/// Read a file named by an option, with the option's error messages.
fn read_file_option(path: &Path) -> Result<Vec<u8>, String> {
    if !path.is_absolute() {
        return Err("not an absolute path".into());
    }
    std::fs::read(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => "file not found".to_string(),
        kind => format!("cannot read: {kind}"),
    })
}

/// Validate a `tls_cert_file` value by reading it; the error is the
/// option's message.
pub fn check_cert_file(path: &Path) -> Result<(), String> {
    if load_certificates(&read_file_option(path)?).is_empty() {
        return Err("no certificate found".into());
    }
    Ok(())
}

/// Validate `tls_cert_file` and `tls_key_file` together by building the
/// client configuration a check builds with them, so a key the TLS library
/// cannot load, or one that does not belong to the certificate, is found
/// at validation. The error is the message for `tls_key_file`; it never
/// contains the files' content.
pub fn check_client_certificate(client: &ClientCertificate) -> Result<(), String> {
    // Server verification plays no part in loading the client certificate.
    let tls = TlsSettings {
        verify: false,
        server_name: None,
        ca_file: None,
    };
    match client_config_with_client(&tls, Some(client)) {
        Ok(_) => Ok(()),
        Err(f) => Err(f
            .reason
            .strip_prefix("tls_key_file: ")
            .unwrap_or(&f.reason)
            .to_owned()),
    }
}

/// Validate a `tls_key_file` value by reading it; the error is the
/// option's message. It never contains the file's content.
pub fn check_key_file(path: &Path) -> Result<(), String> {
    let bytes = read_file_option(path)?;
    match PrivateKeyDer::from_pem_slice(&bytes) {
        Ok(_) => Ok(()),
        Err(_) => Err("no private key found".into()),
    }
}

type ClientAuth = (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>);

fn read_client_certificate(client: &ClientCertificate) -> Result<ClientAuth, TlsFailure> {
    let certs = std::fs::read(&client.cert_file)
        .map_err(|e| TlsFailure::local(format!("tls_cert_file: {}", e.kind())))?;
    let certs = load_certificates(&certs);
    if certs.is_empty() {
        return Err(TlsFailure::local("tls_cert_file: no certificate found"));
    }
    let key = std::fs::read(&client.key_file)
        .map_err(|e| TlsFailure::local(format!("tls_key_file: {}", e.kind())))?;
    let key = PrivateKeyDer::from_pem_slice(&key)
        .map_err(|_| TlsFailure::local("tls_key_file: no private key found"))?;
    Ok((certs, key))
}

fn read_ca_file(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsFailure> {
    Ok(ca_file(path)?.certs.clone())
}

fn roots_from_file(path: &Path) -> Result<Arc<RootCertStore>, TlsFailure> {
    match &ca_file(path)?.roots {
        Some(roots) => Ok(Arc::clone(roots)),
        None => Err(TlsFailure::local("ca_file: no usable certificate")),
    }
}

/// What identifies one version of a file: a replaced file (new inode), a
/// rewritten one (new size, modification or change time) gets a new stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl FileStamp {
    fn of(meta: &std::fs::Metadata) -> FileStamp {
        use std::os::unix::fs::MetadataExt;
        FileStamp {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        }
    }
}

/// A `ca_file` as read and parsed once.
struct CaFile {
    stamp: FileStamp,
    certs: Vec<CertificateDer<'static>>,
    /// `None` when no certificate in it is usable as a trust anchor.
    roots: Option<Arc<RootCertStore>>,
}

/// The certificates of `path`, parsed again only when the file changed
/// since it was last read (see [`FileStamp`]): a check costs one `stat`, and
/// a replaced file still takes effect at the next check. Failures are not
/// remembered, so a fixed file is read at the next check too.
fn ca_file(path: &Path) -> Result<Arc<CaFile>, TlsFailure> {
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, Arc<CaFile>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let unreadable = |e: io::Error| TlsFailure::local(format!("ca_file: {}", e.kind()));
    // Stamped before reading: a change during the read gets a new stamp,
    // so it is read again at the next check.
    let stamp = FileStamp::of(&std::fs::metadata(path).map_err(unreadable)?);
    if let Some(cached) = cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(path)
        .filter(|c| c.stamp == stamp)
    {
        return Ok(Arc::clone(cached));
    }
    let bytes = std::fs::read(path).map_err(unreadable)?;
    let (certs, roots) =
        parse_ca(&bytes).map_err(|m| TlsFailure::local(format!("ca_file: {m}")))?;
    let parsed = Arc::new(CaFile {
        stamp,
        certs,
        roots,
    });
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(path.to_path_buf(), Arc::clone(&parsed));
    Ok(parsed)
}

/// The certificates in a `ca_file`'s content, and the trust anchors made of
/// those usable as one (`None` when none is). Used both by a check and by
/// [`check_ca_file`], so validation accepts exactly the files a check can
/// use. The error is the option's message.
type CaContent = (Vec<CertificateDer<'static>>, Option<Arc<RootCertStore>>);

fn parse_ca(bytes: &[u8]) -> Result<CaContent, String> {
    let certs = load_certificates(bytes);
    if certs.is_empty() {
        return Err("no certificate found".into());
    }
    let mut roots = RootCertStore::empty();
    let (added, _) = roots.add_parsable_certificates(certs.iter().cloned());
    Ok((certs, (added > 0).then(|| Arc::new(roots))))
}

/// Load the system's CA certificates into the per-process cache, so the
/// first check that needs them does not wait for it. Call it outside the
/// async runtime's worker threads (for example while validating a config).
/// A host without CA certificates is reported by the checks themselves.
pub fn preload_system_roots() {
    let _ = system_roots();
}

/// The system's CA certificates, loaded once per process.
fn system_roots_der() -> Result<&'static Vec<CertificateDer<'static>>, TlsFailure> {
    static CERTS: OnceLock<Vec<CertificateDer<'static>>> = OnceLock::new();
    let certs = CERTS.get_or_init(|| rustls_native_certs::load_native_certs().certs);
    if certs.is_empty() {
        return Err(TlsFailure::local(
            "tls: no system CA certificates found; set ca_file",
        ));
    }
    Ok(certs)
}

fn system_roots() -> Result<Arc<RootCertStore>, TlsFailure> {
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
mod tests;
