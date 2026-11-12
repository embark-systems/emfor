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

/// Like [`tls_server`], but each connection is dropped after `greeting`
/// without a TLS close_notify, as servers often do on errors.
pub async fn tls_server_without_close_notify(
    config: Arc<ServerConfig>,
    greeting: &'static [u8],
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(config);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut tls) = acceptor.accept(stream).await {
                    let _ = tls.write_all(greeting).await;
                    let _ = tls.flush().await;
                    let (tcp, _) = tls.into_inner();
                    drop(tcp);
                }
            });
        }
    });
    addr
}
