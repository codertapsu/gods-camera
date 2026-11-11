//! TLS for the hub's network listener, and for programs that talk to it.
//!
//! No authority vouches for the hub's certificate. A device learns it while
//! pairing, where the six-digit code proves that the certificate belongs to
//! the hub the person is looking at, and afterwards accepts that certificate
//! and no other.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::server::TlsStream;

use crate::HubError;

/// How long a client may take over the TLS handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Connections from the network held at once, those still in their handshake
/// included. One more is closed as soon as it arrives. A person's devices need
/// a handful; the limit is there so that someone on the network who opens
/// connections and says nothing cannot use up the app's file handles, which
/// the camera and everything else in the app need too.
pub(crate) const MAX_CONNECTIONS: usize = 64;
/// How long a connection from the network may stay silent, with nothing read
/// and nothing written, before it is closed. A device that is sending or
/// receiving never comes near it, and one that kept a connection open for
/// later simply opens a new one. Without this limit, someone on the network
/// could take every place by connecting and saying nothing.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

fn tls_error(error: rustls::Error) -> HubError {
    HubError::Tls(error.to_string())
}

/// The hub's side of TLS: version 1.3 only, with the hub's one certificate.
pub(crate) fn server_config(identity: &gc_pair::HubIdentity) -> Result<Arc<rustls::ServerConfig>, HubError> {
    let certificate = CertificateDer::from(identity.certificate_der().to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.private_key_der().to_vec()));
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_error)?
        .with_no_client_auth()
        .with_single_cert(vec![certificate], key)
        .map_err(tls_error)?;
    // The server speaks HTTP/1.1 only, and says so, so that no client tries HTTP/2.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// A connection from the network. While it lives it holds one of the
/// listener's places, and it closes itself after `idle` without traffic.
pub(crate) struct Connection {
    stream: TlsStream<TcpStream>,
    _place: tokio::sync::OwnedSemaphorePermit,
    idle: Duration,
    /// When the connection is given up, unless something is read or written first.
    deadline: std::pin::Pin<Box<tokio::time::Sleep>>,
}

impl Connection {
    fn new(stream: TlsStream<TcpStream>, place: tokio::sync::OwnedSemaphorePermit, idle: Duration) -> Self {
        Self { stream, _place: place, idle, deadline: Box::pin(tokio::time::sleep(idle)) }
    }

    /// Traffic passed: the silence starts over.
    fn touched(&mut self) {
        let next = tokio::time::Instant::now() + self.idle;
        self.deadline.as_mut().reset(next);
    }

    /// While waiting for traffic: has the silence lasted too long?
    fn silent_too_long(&mut self, cx: &mut std::task::Context<'_>) -> bool {
        std::future::Future::poll(self.deadline.as_mut(), cx).is_ready()
    }
}

fn idle_error() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::TimedOut, "the connection was silent for too long")
}

impl tokio::io::AsyncRead for Connection {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buffer.filled().len();
        match std::pin::Pin::new(&mut self.stream).poll_read(cx, buffer) {
            std::task::Poll::Ready(result) => {
                if buffer.filled().len() > before {
                    self.touched();
                }
                std::task::Poll::Ready(result)
            }
            std::task::Poll::Pending if self.silent_too_long(cx) => std::task::Poll::Ready(Err(idle_error())),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl tokio::io::AsyncWrite for Connection {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match std::pin::Pin::new(&mut self.stream).poll_write(cx, bytes) {
            std::task::Poll::Ready(result) => {
                if matches!(result, Ok(written) if written > 0) {
                    self.touched();
                }
                std::task::Poll::Ready(result)
            }
            // A client that stops reading must not hold a place for ever either.
            std::task::Poll::Pending if self.silent_too_long(cx) => std::task::Poll::Ready(Err(idle_error())),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        slices: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match std::pin::Pin::new(&mut self.stream).poll_write_vectored(cx, slices) {
            std::task::Poll::Ready(result) => {
                if matches!(result, Ok(written) if written > 0) {
                    self.touched();
                }
                std::task::Poll::Ready(result)
            }
            std::task::Poll::Pending if self.silent_too_long(cx) => std::task::Poll::Ready(Err(idle_error())),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
}

/// A listener that hands out connections once their TLS handshake is done.
///
/// Handshakes run beside each other: a client that is slow, or silent, does
/// not hold up the others.
pub(crate) struct TlsListener {
    ready: tokio::sync::mpsc::Receiver<(Connection, SocketAddr)>,
    local: SocketAddr,
    accepting: tokio::task::JoinHandle<()>,
}

impl TlsListener {
    /// `places` is how many connections may be held at once, and `idle` how
    /// long one may stay silent before it is closed.
    pub(crate) fn new(
        listener: TcpListener,
        config: Arc<rustls::ServerConfig>,
        places: usize,
        idle: Duration,
    ) -> std::io::Result<Self> {
        let local = listener.local_addr()?;
        let (sender, ready) = tokio::sync::mpsc::channel(32);
        let acceptor = tokio_rustls::TlsAcceptor::from(config);
        let places = Arc::new(tokio::sync::Semaphore::new(places));
        let accepting = tokio::spawn(async move {
            loop {
                let (stream, peer) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(_) => {
                        // Out of file handles, or a connection that went away
                        // while it waited. Neither ends the listener.
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                // No place left: the connection is closed at once.
                let Ok(place) = places.clone().try_acquire_owned() else { continue };
                let (acceptor, sender) = (acceptor.clone(), sender.clone());
                tokio::spawn(async move {
                    let _ = stream.set_nodelay(true);
                    if let Ok(Ok(stream)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                        let _ = sender.send((Connection::new(stream, place, idle), peer)).await;
                    }
                });
            }
        });
        Ok(Self { ready, local, accepting })
    }
}

impl Drop for TlsListener {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = Connection;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(connection) => connection,
            // The accepting task ends only when this listener is dropped.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// Which certificate a program accepts from a hub.
#[derive(Debug)]
enum Expected {
    /// The certificate with this SHA-256, learned while pairing.
    Pinned([u8; 32]),
    /// Whichever certificate is shown first, and then only that one.
    First(Arc<Mutex<Option<[u8; 32]>>>),
}

#[derive(Debug)]
struct HubVerifier {
    expected: Expected,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for HubVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        // The hub is known by its certificate, not by a name or an address.
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let shown: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        let accepted = match &self.expected {
            Expected::Pinned(pinned) => bool::from(pinned.ct_eq(&shown)),
            Expected::First(first) => {
                let mut first = first.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                *first.get_or_insert(shown) == shown
            }
        };
        if accepted {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure))
        }
    }

    // The handshake signature is always checked: it proves that the other side
    // holds the private key of the certificate it showed.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn client_config(expected: Expected) -> rustls::ClientConfig {
    let provider = Arc::new(ring::default_provider());
    let verifier = HubVerifier { expected, algorithms: provider.signature_verification_algorithms };
    rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("the provider supports TLS 1.3")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth()
}

/// The certificate a hub showed to a program that is pairing with it.
#[derive(Debug, Clone)]
pub struct SeenCertificate(Arc<Mutex<Option<[u8; 32]>>>);

impl SeenCertificate {
    /// The certificate's SHA-256 as 64 hexadecimal digits, once a connection
    /// has been made. This is what goes into the pairing exchange, and what
    /// the program pins afterwards.
    pub fn fingerprint(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).map(hex::encode)
    }
}

/// TLS settings for a program that is pairing with a hub it does not know yet.
///
/// The first certificate shown is taken as it comes and remembered; every
/// later connection made with these settings must show the same one. Whether
/// that certificate really is the hub's is decided by the pairing exchange,
/// which covers its fingerprint.
pub fn pairing_client_config() -> (rustls::ClientConfig, SeenCertificate) {
    let first = Arc::new(Mutex::new(None));
    (client_config(Expected::First(first.clone())), SeenCertificate(first))
}

/// TLS settings for a paired program: only the hub whose certificate has this
/// fingerprint is accepted.
pub fn pinned_client_config(fingerprint: &str) -> Result<rustls::ClientConfig, gc_pair::PairError> {
    Ok(client_config(Expected::Pinned(gc_pair::parse_fingerprint(fingerprint)?)))
}

/// Make this crate's TLS library the one the whole program uses.
///
/// The hub does not need this. A program that also uses an HTTP client built
/// on rustls without a library of its own, as the `gc` tool does, calls it
/// once at the start. Calling it again does nothing.
pub fn install_crypto_provider() {
    let _ = ring::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::serve::Listener;
    use rustls::pki_types::ServerName;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Connect and finish the handshake, as a paired device would.
    async fn connect(address: SocketAddr, fingerprint: &str) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let connector = tokio_rustls::TlsConnector::from(Arc::new(pinned_client_config(fingerprint).unwrap()));
        let stream = TcpStream::connect(address).await?;
        let mut stream = connector.connect(ServerName::try_from("hub.invalid").unwrap(), stream).await?;
        // The server may close a connection it has no place for only after the
        // client believes the handshake is over, so ask for a byte back.
        stream.write_all(b"?").await?;
        Ok(stream)
    }

    #[tokio::test]
    async fn the_listener_holds_only_so_many_connections() {
        let identity = gc_pair::HubIdentity::generate("test").unwrap();
        let fingerprint = identity.fingerprint();
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut listener = TlsListener::new(tcp, server_config(&identity).unwrap(), 2, IDLE_TIMEOUT).unwrap();
        let address = listener.local_addr().unwrap();

        // Two connections take the two places. The server holds them, as it
        // would while a device is using them.
        let mut first = connect(address, &fingerprint).await.unwrap();
        let (mut served_first, _) = listener.accept().await;
        let _second = connect(address, &fingerprint).await.unwrap();
        let (served_second, _) = listener.accept().await;

        // A third finds no place: it is closed without an answer.
        let third = async {
            let mut stream = connect(address, &fingerprint).await?;
            let mut byte = [0u8; 1];
            match stream.read(&mut byte).await? {
                0 => Err(std::io::ErrorKind::UnexpectedEof.into()),
                _ => Ok(()),
            }
        };
        let refused: std::io::Result<()> = third.await;
        assert!(refused.is_err(), "a connection beyond the limit must be closed");

        // The ones that have a place still work.
        let mut byte = [0u8; 1];
        served_first.read_exact(&mut byte).await.unwrap();
        assert_eq!(&byte, b"?");
        served_first.write_all(b"!").await.unwrap();
        first.read_exact(&mut byte).await.unwrap();
        assert_eq!(&byte, b"!");

        // When one is let go, its place is free again.
        drop(served_second);
        let mut fourth = None;
        for _ in 0..50 {
            if let Ok(stream) = connect(address, &fingerprint).await {
                fourth = Some(stream);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut fourth = fourth.expect("a freed place can be taken again");
        let (mut served_fourth, _) = listener.accept().await;
        served_fourth.write_all(b"!").await.unwrap();
        fourth.read_exact(&mut byte).await.unwrap();
        assert_eq!(&byte, b"!");
    }

    #[tokio::test]
    async fn a_client_that_never_finishes_its_handshake_holds_nobody_up() {
        let identity = gc_pair::HubIdentity::generate("test").unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut listener = TlsListener::new(tcp, server_config(&identity).unwrap(), 4, IDLE_TIMEOUT).unwrap();
        let address = listener.local_addr().unwrap();
        // Connected, and silent.
        let _silent = TcpStream::connect(address).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Another client is served all the same.
        let _client = connect(address, &identity.fingerprint()).await.unwrap();
        let served = tokio::time::timeout(Duration::from_secs(5), listener.accept()).await;
        assert!(served.is_ok(), "the silent client must not block the listener");
    }

    #[tokio::test]
    async fn a_silent_connection_is_closed_and_gives_its_place_back() {
        let identity = gc_pair::HubIdentity::generate("test").unwrap();
        let fingerprint = identity.fingerprint();
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        // One place, and a short patience.
        let mut listener = TlsListener::new(tcp, server_config(&identity).unwrap(), 1, Duration::from_millis(300)).unwrap();
        let address = listener.local_addr().unwrap();

        let mut silent = connect(address, &fingerprint).await.unwrap();
        let (mut served, _) = listener.accept().await;
        // The server reads what there is, as it does while waiting for a request.
        let serving = tokio::spawn(async move {
            let mut buffer = [0u8; 16];
            loop {
                match served.read(&mut buffer).await {
                    Ok(0) => return "closed by the client",
                    Ok(_) => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::TimedOut => return "silent for too long",
                    Err(_) => return "another error",
                }
            }
        });
        // A client that keeps talking is kept.
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_millis(150)).await;
            silent.write_all(b".").await.unwrap();
        }
        assert!(!serving.is_finished(), "a connection with traffic must stay");
        // Then it says nothing, and the server lets it go.
        let ended = tokio::time::timeout(Duration::from_secs(3), serving).await.expect("the server gave up").unwrap();
        assert_eq!(ended, "silent for too long");

        // The one place is free again for a device that has something to say.
        let mut next = None;
        for _ in 0..50 {
            if let Ok(stream) = connect(address, &fingerprint).await {
                next = Some(stream);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(next.is_some(), "the place of a closed connection can be taken again");
        let accepted = tokio::time::timeout(Duration::from_secs(3), listener.accept()).await;
        assert!(accepted.is_ok());
    }

    #[tokio::test]
    async fn a_client_accepts_only_the_certificate_it_expects() {
        let identity = gc_pair::HubIdentity::generate("test").unwrap();
        let other = gc_pair::HubIdentity::generate("test").unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut listener = TlsListener::new(tcp, server_config(&identity).unwrap(), 8, IDLE_TIMEOUT).unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await;
                tokio::spawn(async move {
                    let mut byte = [0u8; 1];
                    let _ = stream.read(&mut byte).await;
                });
            }
        });

        assert!(connect(address, &identity.fingerprint()).await.is_ok());
        let wrong = connect(address, &other.fingerprint()).await.unwrap_err();
        assert!(wrong.to_string().contains("ApplicationVerificationFailure"), "{wrong}");
        assert!(pinned_client_config("not a fingerprint").is_err());

        // While pairing, the first certificate is taken and then insisted on.
        let (config, seen) = pairing_client_config();
        assert_eq!(seen.fingerprint(), None);
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let name = || ServerName::try_from("hub.invalid").unwrap();
        connector.connect(name(), TcpStream::connect(address).await.unwrap()).await.unwrap();
        assert_eq!(seen.fingerprint(), Some(identity.fingerprint()));

        // The same settings against a hub with another certificate: refused.
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut elsewhere = TlsListener::new(tcp, server_config(&other).unwrap(), 8, IDLE_TIMEOUT).unwrap();
        let elsewhere_address = elsewhere.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let _ = elsewhere.accept().await;
            }
        });
        let changed = connector.connect(name(), TcpStream::connect(elsewhere_address).await.unwrap()).await;
        assert!(changed.is_err(), "a second, different certificate must be refused");
        assert_eq!(seen.fingerprint(), Some(identity.fingerprint()));
    }
}
