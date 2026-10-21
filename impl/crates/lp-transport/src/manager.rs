//! The connection manager: one connection per peer key, streams dispatched by
//! purpose. ADR 0036; spec chapter 04 section 3.1.
//!
//! The manager owns the endpoint and every live connection. Whoever needs a
//! stream opens it with a purpose and the header goes first; incoming streams
//! are read for their header and handed to the handler registered for that
//! purpose, or reset with [`UNKNOWN_PURPOSE_CODE`] while the connection
//! carries on. The manager knows nothing about what the messages mean.

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use lp_core::frame::{StreamHeader, UNKNOWN_PURPOSE_CODE};
use lp_core::{Error, PublicKey};
use tokio::sync::mpsc;

use crate::quic::{Connection, Endpoint};
use crate::stream::FramedStream;

/// A handler for incoming streams of one purpose.
pub type StreamHandler =
    Arc<dyn Fn(Connection, FramedStream) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Something the manager tells its owner.
#[derive(Debug)]
pub enum Event {
    /// A connection completed its handshake; the peer key is authenticated,
    /// not yet recognised. The owner decides what to do and may call
    /// [`Manager::drop_peer`].
    Connected(Connection),
    /// The connection to this peer ended.
    Disconnected(PublicKey),
}

#[derive(Default)]
struct State {
    peers: HashMap<PublicKey, Connection>,
    handlers: HashMap<String, StreamHandler>,
}

/// Owns the endpoint and the live connections.
#[derive(Clone)]
pub struct Manager {
    endpoint: Endpoint,
    state: Arc<Mutex<State>>,
    events: mpsc::Sender<Event>,
}

impl std::fmt::Debug for Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Manager({})", self.endpoint.public())
    }
}

impl Manager {
    /// Wraps an endpoint and starts accepting. Events arrive on the returned
    /// receiver.
    #[must_use]
    pub fn start(endpoint: Endpoint) -> (Self, mpsc::Receiver<Event>) {
        let (events, rx) = mpsc::channel(64);
        let manager = Self {
            endpoint,
            state: Arc::new(Mutex::new(State::default())),
            events,
        };
        let accepting = manager.clone();
        tokio::spawn(async move {
            while let Some(result) = accepting.endpoint.accept().await {
                match result {
                    Ok(conn) => accepting.adopt(conn).await,
                    Err(e) => tracing::debug!(%e, "incoming connection failed"),
                }
            }
        });
        (manager, rx)
    }

    /// The endpoint.
    #[must_use]
    pub const fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Registers the handler for incoming streams of `purpose`. Replaces any
    /// earlier handler for the same purpose.
    pub fn on_stream(&self, purpose: &str, handler: StreamHandler) {
        self.lock().handlers.insert(purpose.to_owned(), handler);
    }

    /// Dials `addr` unless a connection to the resulting peer already exists,
    /// in which case the new one is closed and the existing one returned.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] if the dial or handshake fails.
    pub async fn connect(&self, addr: SocketAddr) -> Result<Connection, Error> {
        let conn = self.endpoint.connect(addr).await?;
        if let Some(existing) = self.lock().peers.get(conn.peer()).cloned() {
            let alive = existing.raw().close_reason().is_none();
            if alive && !existing.dialled_by_us() && !self.we_keep_our_dial(conn.peer()) {
                // Both sides dialled at once and the tie-break keeps theirs.
                tracing::debug!(peer = %conn.peer(), "already connected; dropping duplicate dial");
                conn.close();
                return Ok(existing);
            }
            // Otherwise the new one replaces the old: a dial of ours that
            // completed is proof the peer is there now, while the old one
            // may have died without a word, and the peer applies the same
            // rule on its side (`adopt`).
        }
        self.adopt(conn.clone()).await;
        Ok(conn)
    }

    /// The live connection to `peer`, if any.
    #[must_use]
    pub fn peer(&self, peer: &PublicKey) -> Option<Connection> {
        self.lock().peers.get(peer).cloned()
    }

    /// Every live peer key.
    #[must_use]
    pub fn peers(&self) -> Vec<PublicKey> {
        self.lock().peers.keys().copied().collect()
    }

    /// Closes and forgets the connection to `peer`.
    pub fn drop_peer(&self, peer: &PublicKey) {
        if let Some(conn) = self.lock().peers.remove(peer) {
            conn.close();
        }
    }

    /// Opens a stream to `peer` for `purpose` and writes the header.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] if there is no connection or it is gone.
    pub async fn open(&self, peer: &PublicKey, purpose: &str) -> Result<FramedStream, Error> {
        let conn = self
            .peer(peer)
            .ok_or_else(|| Error::Transport(format!("not connected to {peer}")))?;
        open_on(&conn, purpose).await
    }

    /// Closes everything.
    pub fn shutdown(&self) {
        let peers: Vec<_> = self.lock().peers.drain().map(|(_, c)| c).collect();
        for c in peers {
            c.close();
        }
        self.endpoint.close();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// When both sides dial each other at once, the connection dialled by the
    /// side with the lower key survives. Both sides compute the same answer.
    fn we_keep_our_dial(&self, peer: &PublicKey) -> bool {
        self.endpoint.public() < peer
    }

    /// Records a connection, starts its stream loop and reports it. A new
    /// connection displaces a live one for the same peer when the same side
    /// dialled again, or when dials crossed and the tie-break says so; a dead
    /// one is always replaced. Both ends reach the same answer.
    async fn adopt(&self, conn: Connection) {
        let peer = *conn.peer();
        {
            let mut st = self.lock();
            if let Some(old) = st.peers.get(&peer).cloned() {
                if old.raw().stable_id() == conn.raw().stable_id() {
                    return;
                }
                let old_alive = old.raw().close_reason().is_none();
                let cross = conn.dialled_by_us() != old.dialled_by_us();
                // The same side dialled again: it lost the earlier one (a
                // restart, a silent drop) or is proving the peer is there, so
                // the new one wins on both sides. Dials that crossed are
                // settled by the tie-break, the same on both sides.
                let new_wins = !cross || conn.dialled_by_us() == self.we_keep_our_dial(&peer);
                if old_alive && !new_wins {
                    tracing::debug!(%peer, "dropping duplicate connection; keeping the live one");
                    drop(st);
                    conn.close();
                    return;
                }
                tracing::debug!(%peer, old_alive, "replacing an existing connection");
                old.close();
            }
            st.peers.insert(peer, conn.clone());
        }
        let manager = self.clone();
        let streams = conn.clone();
        tokio::spawn(async move {
            manager.stream_loop(streams).await;
        });
        if self.events.send(Event::Connected(conn)).await.is_err() {
            tracing::debug!("event receiver gone");
        }
    }

    async fn stream_loop(&self, conn: Connection) {
        while let Ok((send, recv)) = conn.accept_bi().await {
            {
                {
                    let mut stream = FramedStream::new(send, recv);
                    let header = match stream.recv_header().await {
                        Ok(h) => h,
                        Err(e) => {
                            tracing::debug!(peer = %conn.peer(), %e, "stream without a valid header");
                            stream.reset(UNKNOWN_PURPOSE_CODE);
                            continue;
                        }
                    };
                    let handler = self.lock().handlers.get(&header.purpose).cloned();
                    match handler {
                        Some(h) => {
                            tokio::spawn(h(conn.clone(), stream));
                        }
                        None => {
                            tracing::debug!(peer = %conn.peer(), purpose = %header.purpose, "unknown stream purpose; refusing the stream");
                            stream.reset(UNKNOWN_PURPOSE_CODE);
                        }
                    }
                }
            }
        }
        let peer = *conn.peer();
        let removed = {
            let mut st = self.lock();
            match st.peers.get(&peer) {
                Some(current) if current.raw().stable_id() == conn.raw().stable_id() => {
                    st.peers.remove(&peer);
                    true
                }
                _ => false,
            }
        };
        if removed {
            let _ = self.events.send(Event::Disconnected(peer)).await;
        }
    }
}

/// Opens a stream for `purpose` on `conn` and writes the header.
///
/// # Errors
///
/// Returns [`Error::Transport`] if the connection is gone.
pub async fn open_on(conn: &Connection, purpose: &str) -> Result<FramedStream, Error> {
    let (send, recv) = conn.open_bi().await?;
    let mut stream = FramedStream::new(send, recv);
    stream.send(&StreamHeader::new(purpose)).await?;
    Ok(stream)
}
