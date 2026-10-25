//! QUIC connections between devices. Spec chapter 04 section 3; ADR 0028.
//!
//! One endpoint per daemon, bound to a port the operating system assigns,
//! that both listens and dials with the same device key presented as a TLS
//! raw public key. A finished handshake yields the peer's key; what the peer
//! is allowed to do is decided above this layer.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use lp_core::{Error, PublicKey};
use lp_crypto::session::{SessionConfig, peer_key};
use lp_crypto::signing::SigningKey;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::pki_types::CertificateDer;

/// The server name sent in the ClientHello. Raw public keys carry no name and
/// the verifier ignores it, but TLS requires one; it is never a host name.
const SERVER_NAME: &str = "ligature";

/// Keep-alive and idle timeouts of a connection. A session between two of a
/// person's devices stays up while both are on the network, so keep-alives
/// run well inside the idle timeout; without them QUIC closes an idle
/// connection and the peer vanishes from view until discovery redials it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// How often a PING keeps the connection alive; `None` disables it.
    pub keep_alive: Option<Duration>,
    /// How long without any packet before the connection is closed.
    pub idle: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            keep_alive: Some(Duration::from_secs(15)),
            idle: Duration::from_secs(60),
        }
    }
}

impl Timeouts {
    fn transport(self) -> Result<quinn::TransportConfig, Error> {
        let mut t = quinn::TransportConfig::default();
        t.keep_alive_interval(self.keep_alive);
        let idle = quinn::IdleTimeout::try_from(self.idle)
            .map_err(|e| Error::Transport(format!("idle timeout: {e}")))?;
        t.max_idle_timeout(Some(idle));
        Ok(t)
    }
}

/// The local QUIC endpoint: listens, dials and owns live connections.
#[derive(Debug, Clone)]
pub struct Endpoint {
    inner: quinn::Endpoint,
    public: PublicKey,
}

impl Endpoint {
    /// Binds `addr` (use port 0 to let the operating system choose) and
    /// configures both directions with `key`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidKey`] for a TLS configuration problem and
    /// [`Error::Io`] if the socket cannot be bound.
    pub fn bind(addr: SocketAddr, key: &SigningKey) -> Result<Self, Error> {
        Self::bind_with(addr, key, Timeouts::default())
    }

    /// As [`Self::bind`] with explicit keep-alive and idle timeouts.
    ///
    /// # Errors
    ///
    /// As [`Self::bind`].
    pub fn bind_with(
        addr: SocketAddr,
        key: &SigningKey,
        timeouts: Timeouts,
    ) -> Result<Self, Error> {
        let session = SessionConfig::new(key)?;
        let tls = |e: quinn::crypto::rustls::NoInitialCipherSuite| {
            Error::InvalidKey(format!("quic tls: {e}"))
        };
        let transport = Arc::new(timeouts.transport()?);
        let server_crypto =
            QuicServerConfig::try_from(session.server.as_ref().clone()).map_err(tls)?;
        let mut server = quinn::ServerConfig::with_crypto(Arc::new(server_crypto));
        server.transport_config(transport.clone());
        let client_crypto =
            QuicClientConfig::try_from(session.client.as_ref().clone()).map_err(tls)?;
        let mut client = quinn::ClientConfig::new(Arc::new(client_crypto));
        client.transport_config(transport);
        let mut inner = quinn::Endpoint::server(server, addr)
            .map_err(|e| Error::Io(format!("bind {addr}: {e}")))?;
        inner.set_default_client_config(client);
        Ok(Self {
            inner,
            public: session.public,
        })
    }

    /// The address the endpoint is bound to, with the assigned port.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the socket cannot report it.
    pub fn local_addr(&self) -> Result<SocketAddr, Error> {
        self.inner
            .local_addr()
            .map_err(|e| Error::Io(format!("local address: {e}")))
    }

    /// This endpoint's key.
    #[must_use]
    pub const fn public(&self) -> &PublicKey {
        &self.public
    }

    /// Dials `addr` and completes the handshake.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] if the connection or handshake fails.
    pub async fn connect(&self, addr: SocketAddr) -> Result<Connection, Error> {
        let connecting = self
            .inner
            .connect(addr, SERVER_NAME)
            .map_err(|e| Error::Transport(format!("connect {addr}: {e}")))?;
        let conn = connecting
            .await
            .map_err(|e| Error::Transport(format!("handshake with {addr}: {e}")))?;
        Connection::from_quinn(conn)
    }

    /// Waits for the next incoming connection and completes its handshake.
    /// Returns `None` once the endpoint is closed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] if the handshake fails.
    pub async fn accept(&self) -> Option<Result<Connection, Error>> {
        let incoming = self.inner.accept().await?;
        Some(
            async move {
                let conn = incoming
                    .await
                    .map_err(|e| Error::Transport(format!("incoming handshake: {e}")))?;
                Connection::from_quinn(conn)
            }
            .await,
        )
    }

    /// Moves the endpoint to a new socket; live connections migrate to it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the socket cannot be bound or swapped in.
    pub fn rebind(&self, addr: SocketAddr) -> Result<(), Error> {
        let socket =
            std::net::UdpSocket::bind(addr).map_err(|e| Error::Io(format!("bind {addr}: {e}")))?;
        self.inner
            .rebind(socket)
            .map_err(|e| Error::Io(format!("rebind: {e}")))
    }

    /// Closes every connection and stops accepting.
    pub fn close(&self) {
        self.inner.close(0u32.into(), b"closed");
    }

    /// Waits until every connection has finished closing, so the peers have
    /// been told before the process goes.
    pub async fn wait_idle(&self) {
        self.inner.wait_idle().await;
    }
}

/// An authenticated QUIC connection to one peer device.
#[derive(Debug, Clone)]
pub struct Connection {
    inner: quinn::Connection,
    peer: PublicKey,
    established: std::time::Instant,
}

impl Connection {
    fn from_quinn(inner: quinn::Connection) -> Result<Self, Error> {
        let identity = inner
            .peer_identity()
            .ok_or_else(|| Error::Transport("peer presented no identity".to_owned()))?;
        let certs = identity
            .downcast::<Vec<CertificateDer<'static>>>()
            .map_err(|_| Error::Transport("peer identity is not a certificate list".to_owned()))?;
        let peer = peer_key(&certs)?;
        tracing::debug!(peer = %peer, remote = %inner.remote_address(), "connection authenticated");
        Ok(Self {
            inner,
            peer,
            established: std::time::Instant::now(),
        })
    }

    /// How long ago the handshake completed.
    #[must_use]
    pub fn age(&self) -> std::time::Duration {
        self.established.elapsed()
    }

    /// The peer's device key, as authenticated by the handshake.
    #[must_use]
    pub const fn peer(&self) -> &PublicKey {
        &self.peer
    }

    /// Whether this side opened the connection.
    #[must_use]
    pub fn dialled_by_us(&self) -> bool {
        self.inner.side().is_client()
    }

    /// The peer's current address.
    #[must_use]
    pub fn remote_address(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    /// Keying material exported from the TLS session (RFC 5705 / RFC 8446
    /// section 7.5) for `label` and `context`, `len` bytes. Both sides get
    /// the same bytes; this is what the ceremony code is derived from
    /// (ADR 0029).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] if the length is unsupported.
    pub fn export_keying_material(
        &self,
        label: &[u8],
        context: &[u8],
        len: usize,
    ) -> Result<Vec<u8>, Error> {
        let mut out = vec![0u8; len];
        self.inner
            .export_keying_material(&mut out, label, context)
            .map_err(|_| Error::Transport("exporter output too long".to_owned()))?;
        Ok(out)
    }

    /// Opens a bidirectional stream.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] if the connection is gone.
    pub async fn open_bi(&self) -> Result<(quinn::SendStream, quinn::RecvStream), Error> {
        self.inner
            .open_bi()
            .await
            .map_err(|e| Error::Transport(format!("open stream: {e}")))
    }

    /// Accepts the next bidirectional stream the peer opened.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] if the connection is gone.
    pub async fn accept_bi(&self) -> Result<(quinn::SendStream, quinn::RecvStream), Error> {
        self.inner
            .accept_bi()
            .await
            .map_err(|e| Error::Transport(format!("accept stream: {e}")))
    }

    /// Closes the connection.
    pub fn close(&self) {
        self.inner.close(0u32.into(), b"closed");
    }

    /// The underlying quinn connection, for the connection manager.
    #[must_use]
    pub fn raw(&self) -> &quinn::Connection {
        &self.inner
    }
}
