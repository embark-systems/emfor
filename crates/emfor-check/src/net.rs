//! Connections to backends: TCP, optionally wrapped in TLS, and UDP
//! request and response exchanges. The TLS part (trust, verification,
//! handshake timing) is `emfor-tls`, shared with the daemon's `tcp` checker.

use std::io;
use std::net::{IpAddr, SocketAddr};
#[cfg(not(fuzzing))]
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};

use emfor_plugin::CheckOutcome;
use emfor_tls::{TlsFailure, TlsSettings};
use rustls::ClientConfig;
use rustls::pki_types::CertificateDer;
use tokio::io::{AsyncRead, AsyncWrite};
#[cfg(not(fuzzing))]
use tokio::net::{TcpStream, UdpSocket};

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

/// A random value for request identifiers and nonces. Fuzzing builds use
/// the default (zero), so the fuzzer can write replies that match them.
#[cfg(not(fuzzing))]
pub fn random<T>() -> T
where
    rand::distr::StandardUniform: rand::distr::Distribution<T>,
{
    rand::random()
}

/// A random value for request identifiers and nonces; zero in fuzzing
/// builds.
#[cfg(fuzzing)]
pub fn random<T: Default>() -> T {
    T::default()
}

/// Fill `buf` with random bytes; zeros in fuzzing builds, see [`random`].
pub fn fill(buf: &mut [u8]) {
    if cfg!(fuzzing) {
        buf.fill(0);
    } else {
        rand::fill(buf);
    }
}

/// A TCP connection to a backend. Fuzzing builds (`cfg(fuzzing)`) talk to
/// [`fuzz::Conn`] instead, which answers with the fuzzer's bytes.
#[cfg(not(fuzzing))]
pub type Tcp = TcpStream;
#[cfg(fuzzing)]
pub type Tcp = fuzz::Conn;

/// Open a TCP connection to the backend.
#[cfg(not(fuzzing))]
pub async fn connect(ip: IpAddr, port: u16) -> Result<Tcp, CheckOutcome> {
    let stream = TcpStream::connect((ip, port))
        .await
        .map_err(|e| io_failure("connect: ", &e))?;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// Open a TCP connection to the backend.
#[cfg(fuzzing)]
pub async fn connect(_ip: IpAddr, _port: u16) -> Result<Tcp, CheckOutcome> {
    Ok(fuzz::Conn::new())
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

/// The largest datagram [`udp_exchange`] reads.
pub const MAX_DATAGRAM: usize = 65535;

/// Send `request` to the backend over UDP and wait for its response.
///
/// The socket is connected to the backend, so only its datagrams arrive and
/// an ICMP port unreachable comes back as a refused connection (`down`,
/// `connect: `). Each datagram goes to `matches`, which returns `Some` for
/// the response to this request and `None` for anything else, which is
/// dropped. With no response by half of `timeout` the same request is sent
/// once more; with none by `timeout` the result is `down` with reason
/// `timeout`. A socket the host cannot create is `error`.
pub async fn udp_exchange<T>(
    ip: IpAddr,
    port: u16,
    request: &[u8],
    timeout: Duration,
    matches: impl FnMut(&[u8]) -> Option<T>,
) -> Result<T, CheckOutcome> {
    udp_exchange_from(ip, port, |_| request.to_vec(), timeout, matches).await
}

/// [`udp_exchange`] for a request that names the local address it is sent
/// from (as SIP's Via does): `build` gets the socket's local address once
/// it is connected to the backend.
#[cfg(not(fuzzing))]
pub async fn udp_exchange_from<T>(
    ip: IpAddr,
    port: u16,
    build: impl FnOnce(SocketAddr) -> Vec<u8>,
    timeout: Duration,
    mut matches: impl FnMut(&[u8]) -> Option<T>,
) -> Result<T, CheckOutcome> {
    let start = tokio::time::Instant::now();
    let local: SocketAddr = match ip {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(local).await.map_err(|e| bind_failure(&e))?;
    socket
        .connect((ip, port))
        .await
        .map_err(|e| io_failure("connect: ", &e))?;
    let local = socket.local_addr().map_err(|e| bind_failure(&e))?;
    let request = build(local);
    let request = request.as_slice();
    send_datagram(&socket, request).await?;

    let resend_at = start + timeout / 2;
    let deadline = start + timeout;
    let mut resent = false;
    let mut buf = vec![0u8; MAX_DATAGRAM];
    loop {
        let wake = if resent { deadline } else { resend_at };
        match tokio::time::timeout_at(wake, socket.recv(&mut buf)).await {
            Ok(Ok(n)) => {
                if let Some(answer) = matches(&buf[..n]) {
                    return Ok(answer);
                }
            }
            Ok(Err(e)) => return Err(io_failure("connect: ", &e)),
            Err(_) if resent => return Err(CheckOutcome::Down("timeout".into())),
            Err(_) => {
                resent = true;
                send_datagram(&socket, request).await?;
            }
        }
    }
}

/// [`udp_exchange_from`] in fuzzing builds: the first of the
/// [`fuzz::set_reply`] datagrams that `matches` takes, or `down` with
/// reason `timeout`.
#[cfg(fuzzing)]
pub async fn udp_exchange_from<T>(
    _ip: IpAddr,
    _port: u16,
    build: impl FnOnce(SocketAddr) -> Vec<u8>,
    _timeout: Duration,
    mut matches: impl FnMut(&[u8]) -> Option<T>,
) -> Result<T, CheckOutcome> {
    let _request = build(SocketAddr::from(([127, 0, 0, 1], 40000)));
    fuzz::datagrams()
        .iter()
        .find_map(|d| matches(d))
        .ok_or_else(|| CheckOutcome::Down("timeout".into()))
}

/// Stand-ins for the backend in fuzzing builds: every connection and UDP
/// exchange of the current thread gets the bytes given to [`fuzz::set_reply`].
#[cfg(fuzzing)]
pub mod fuzz {
    use std::cell::RefCell;
    use std::io;
    use std::net::SocketAddr;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    thread_local! {
        static REPLY: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }

    /// What the backend answers from now on in this thread: the bytes a TCP
    /// connection reads, and for UDP the datagrams they hold, each prefixed
    /// with its length as two bytes, big-endian.
    pub fn set_reply(bytes: &[u8]) {
        REPLY.with(|r| *r.borrow_mut() = bytes.to_vec());
    }

    pub(super) fn datagrams() -> Vec<Vec<u8>> {
        let bytes = REPLY.with(|r| r.borrow().clone());
        let mut rest = bytes.as_slice();
        let mut out = Vec::new();
        while let [a, b, tail @ ..] = rest {
            let n = usize::from(u16::from_be_bytes([*a, *b])).min(tail.len());
            out.push(tail[..n].to_vec());
            rest = &tail[n..];
        }
        out
    }

    /// A connection that reads the reply, then end of file, and takes
    /// whatever is written.
    ///
    /// Every read first yields once (returns `Pending` and wakes itself
    /// at once), so the other tasks run before it gets bytes, as they would
    /// while a real backend is still answering. Without this, the HTTP/2
    /// connection task of `grpc` reads the response before the request
    /// task has opened its stream, and fails with a protocol error.
    pub struct Conn {
        data: Vec<u8>,
        pos: usize,
        yielded: bool,
    }

    impl Conn {
        pub(super) fn new() -> Conn {
            Conn {
                data: REPLY.with(|r| r.borrow().clone()),
                pos: 0,
                yielded: false,
            }
        }

        pub fn local_addr(&self) -> io::Result<SocketAddr> {
            Ok(SocketAddr::from(([127, 0, 0, 1], 40000)))
        }
    }

    impl AsyncRead for Conn {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if !self.yielded {
                self.yielded = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            self.yielded = false;
            let rest = &self.data[self.pos..];
            let n = rest.len().min(buf.remaining());
            buf.put_slice(&rest[..n]);
            self.pos += n;
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for Conn {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
}

#[cfg(not(fuzzing))]
/// A failed bind to the unspecified address and port 0 is this host's
/// problem, never the backend's: no socket of the address family (for
/// example IPv6 disabled), or no free descriptors.
fn bind_failure(e: &std::io::Error) -> CheckOutcome {
    CheckOutcome::Error(format!("connect: {e}"))
}

#[cfg(not(fuzzing))]
/// Send one datagram; a datagram socket sends all of it or fails.
async fn send_datagram(socket: &UdpSocket, datagram: &[u8]) -> Result<(), CheckOutcome> {
    socket
        .send(datagram)
        .await
        .map(|_| ())
        .map_err(|e| io_failure("connect: ", &e))
}

/// The rustls client configuration for one check, see
/// [`emfor_tls::client_config`].
/// `server_name` as the host of an `:authority`: an IPv6 address in
/// brackets (RFC 3986), anything else as it is.
pub fn authority_host(server_name: &str) -> String {
    match server_name.parse::<std::net::Ipv6Addr>() {
        Ok(v6) => format!("[{v6}]"),
        Err(_) => server_name.to_owned(),
    }
}

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

    /// A UDP server on loopback: `answer` gets each datagram's number
    /// (from 0) and returns what to send back. Returns the server address
    /// and the arrival times of the datagrams it got.
    async fn udp_server(
        answer: impl Fn(usize) -> Vec<Vec<u8>> + Send + 'static,
    ) -> (SocketAddr, std::sync::Arc<std::sync::Mutex<Vec<Instant>>>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let arrivals = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = std::sync::Arc::clone(&arrivals);
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok((_, from)) = socket.recv_from(&mut buf).await {
                let n = {
                    let mut seen = seen.lock().unwrap();
                    seen.push(Instant::now());
                    seen.len() - 1
                };
                for reply in answer(n) {
                    let _ = socket.send_to(&reply, from).await;
                }
            }
        });
        (addr, arrivals)
    }

    fn yes(d: &[u8]) -> Option<&'static str> {
        (d == b"yes").then_some("matched")
    }

    #[tokio::test]
    async fn udp_lost_first_datagram_is_sent_again_at_half_the_timeout() {
        let (addr, arrivals) = udp_server(|n| {
            if n == 0 {
                vec![]
            } else {
                vec![b"yes".to_vec()]
            }
        })
        .await;
        let r = udp_exchange(
            addr.ip(),
            addr.port(),
            b"q",
            Duration::from_millis(900),
            yes,
        )
        .await;
        assert_eq!(r.unwrap(), "matched");
        let arrivals = arrivals.lock().unwrap();
        assert_eq!(arrivals.len(), 2);
        let gap = arrivals[1] - arrivals[0];
        assert!(
            gap >= Duration::from_millis(400) && gap <= Duration::from_millis(600),
            "second request {gap:?} after the first"
        );
    }

    #[tokio::test]
    async fn udp_unrelated_datagrams_are_ignored() {
        let (addr, arrivals) = udp_server(|_| vec![b"nope".to_vec(), b"yes".to_vec()]).await;
        let r = udp_exchange(
            addr.ip(),
            addr.port(),
            b"q",
            Duration::from_millis(900),
            yes,
        )
        .await;
        assert_eq!(r.unwrap(), "matched");
        assert_eq!(arrivals.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn udp_nothing_listening_is_down_connect() {
        // Retry with a fresh port if another test takes the freed one.
        let mut outcome = None;
        for _ in 0..5 {
            let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let addr = s.local_addr().unwrap();
            drop(s);
            let r = udp_exchange(
                addr.ip(),
                addr.port(),
                b"q",
                Duration::from_millis(900),
                yes,
            )
            .await;
            if let Err(o) = r {
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
    fn udp_socket_the_host_cannot_create_is_error() {
        for errno in [libc::EAFNOSUPPORT, libc::EMFILE] {
            let e = std::io::Error::from_raw_os_error(errno);
            match bind_failure(&e) {
                CheckOutcome::Error(r) => assert!(r.starts_with("connect: "), "{r}"),
                other => panic!("errno {errno}: expected error, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn udp_no_answer_sends_twice_and_times_out() {
        let (addr, arrivals) = udp_server(|_| vec![]).await;
        let start = Instant::now();
        let r = udp_exchange(
            addr.ip(),
            addr.port(),
            b"q",
            Duration::from_millis(900),
            yes,
        )
        .await;
        let took = start.elapsed();
        assert!(matches!(r, Err(CheckOutcome::Down(ref r)) if r == "timeout"));
        assert!(
            took >= Duration::from_millis(850) && took <= Duration::from_millis(1100),
            "took {took:?}"
        );
        assert_eq!(arrivals.lock().unwrap().len(), 2);
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
