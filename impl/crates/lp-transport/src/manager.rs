//! The connection manager: one connection per peer key, streams dispatched by
//! purpose. ADR 0036; spec chapter 04 section 3.1.
//!
//! The manager owns the endpoint and every live connection. Whoever needs a
//! stream opens it with a purpose and the header goes first; incoming streams
//! are read for their header and handed to the handler registered for that
//! purpose, or reset with [`UNKNOWN_PURPOSE_CODE`] while the connection
//! carries on. The manager knows nothing about what the messages mean.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use lp_core::frame::{MALFORMED_CODE, NOT_ADMITTED_CODE, StreamHeader, UNKNOWN_PURPOSE_CODE};
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
    /// not yet recognised. The owner decides what to do: [`Manager::admit`],
    /// [`Manager::restrict`] to the pairing offer, or [`Manager::drop_peer`].
    /// Until it does, streams the peer opens wait (see
    /// [`Manager::require_admission`]).
    Connected(Connection),
    /// The connection to this peer ended.
    Disconnected(PublicKey),
}

/// Where a connection stands with its owner (LP-121).
enum Admission {
    /// Not admitted: streams other than the allowed purposes wait here, to
    /// be dispatched if the owner admits the peer and reset if it drops it.
    /// A stranger kept for the pairing offer stays here for as long as it
    /// is kept, and learns nothing from the streams it opened.
    Waiting(Vec<(String, FramedStream)>),
    /// Trusted: everything is dispatched.
    Admitted,
}

/// How many streams a pending connection may have waiting.
const MAX_QUEUED_STREAMS: usize = 8;

#[derive(Default)]
struct State {
    peers: HashMap<PublicKey, Connection>,
    handlers: HashMap<String, StreamHandler>,
    /// The purposes a connection may open before it is admitted; `None`
    /// admits every connection as it arrives, which only the transport's own
    /// tests want.
    gate: Option<HashSet<String>>,
    admission: HashMap<PublicKey, Admission>,
}

enum Decision {
    Dispatch,
    Queued,
    Refuse,
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

    /// Closes and forgets the connection to `peer`; streams that waited for
    /// admission are reset.
    pub fn drop_peer(&self, peer: &PublicKey) {
        let (conn, queued) = {
            let mut st = self.lock();
            (
                st.peers.remove(peer),
                take_queued(st.admission.remove(peer)),
            )
        };
        reset_all(queued);
        if let Some(conn) = conn {
            conn.close();
        }
    }

    /// Holds every stream a connection opens, other than those of the
    /// purposes in `before`, until the owner calls [`Self::admit`] for that
    /// peer, or drops it (LP-121). A connection may have a few streams
    /// waiting; beyond that a stream is reset with [`NOT_ADMITTED_CODE`].
    /// Without this call every connection is admitted as it arrives.
    pub fn require_admission(&self, before: &[&str]) {
        self.lock().gate = Some(before.iter().map(|s| (*s).to_owned()).collect());
    }

    /// Admits every connection, present and future, without a verdict: for
    /// an owner that drives the manager without classifying its peers, such
    /// as a test of what happens on the streams. Undoes
    /// [`Self::require_admission`].
    pub fn admit_all(&self) {
        let pending: Vec<(Connection, Vec<(String, FramedStream)>)> = {
            let mut st = self.lock();
            st.gate = None;
            let admission = std::mem::take(&mut st.admission);
            admission
                .into_iter()
                .filter_map(|(peer, a)| {
                    st.peers
                        .get(&peer)
                        .cloned()
                        .map(|conn| (conn, take_queued(Some(a))))
                })
                .collect()
        };
        for (conn, queued) in pending {
            for (purpose, stream) in queued {
                self.dispatch(&conn, &purpose, stream);
            }
        }
    }

    /// The owner trusts `peer`: the streams that waited are dispatched, and
    /// everything it opens from now on.
    pub fn admit(&self, peer: &PublicKey) {
        let (conn, queued) = {
            let mut st = self.lock();
            if st.gate.is_none() {
                return;
            }
            let conn = st.peers.get(peer).cloned();
            let queued = take_queued(st.admission.insert(*peer, Admission::Admitted));
            (conn, queued)
        };
        match conn {
            Some(conn) => {
                for (purpose, stream) in queued {
                    self.dispatch(&conn, &purpose, stream);
                }
            }
            None => reset_all(queued),
        }
    }

    /// The owner keeps `peer` for the pairing offer only: its other streams
    /// go on waiting, dispatched only if it is admitted later (a newcomer
    /// that becomes one of ours during the ceremony) and reset when it is
    /// dropped.
    pub fn restrict(&self, peer: &PublicKey) {
        let mut st = self.lock();
        if st.gate.is_none() {
            return;
        }
        st.admission
            .entry(*peer)
            .or_insert_with(|| Admission::Waiting(Vec::new()));
    }

    fn dispatch(&self, conn: &Connection, purpose: &str, mut stream: FramedStream) {
        let handler = self.lock().handlers.get(purpose).cloned();
        match handler {
            Some(h) => {
                tokio::spawn(h(conn.clone(), stream));
            }
            None => {
                tracing::debug!(peer = %conn.peer(), %purpose, "unknown stream purpose; refusing the stream");
                stream.reset(UNKNOWN_PURPOSE_CODE);
            }
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
            if st.gate.is_some() {
                let stale = take_queued(st.admission.insert(peer, Admission::Waiting(Vec::new())));
                drop(st);
                reset_all(stale);
            }
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
        let peer = *conn.peer();
        while let Ok((send, recv)) = conn.accept_bi().await {
            let mut stream = FramedStream::new(send, recv);
            let header = match stream.recv_header().await {
                Ok(h) => h,
                Err(e) => {
                    tracing::debug!(%peer, %e, "stream without a valid header");
                    stream.reset(MALFORMED_CODE);
                    continue;
                }
            };
            // Before the owner has classified the connection, only the
            // allowed purposes go through; the rest wait, or are refused.
            let mut slot = Some(stream);
            let decision = {
                let mut guard = self.lock();
                let st = &mut *guard;
                match (&st.gate, st.admission.get_mut(&peer)) {
                    (None, _) => Decision::Dispatch,
                    (Some(allowed), _) if allowed.contains(&header.purpose) => Decision::Dispatch,
                    (Some(_), Some(Admission::Admitted)) => Decision::Dispatch,
                    (Some(_), Some(Admission::Waiting(queue)))
                        if queue.len() < MAX_QUEUED_STREAMS =>
                    {
                        if let Some(stream) = slot.take() {
                            queue.push((header.purpose.clone(), stream));
                        }
                        Decision::Queued
                    }
                    _ => Decision::Refuse,
                }
            };
            let Some(mut stream) = slot else {
                continue;
            };
            match decision {
                Decision::Dispatch => self.dispatch(&conn, &header.purpose, stream),
                Decision::Queued => {}
                Decision::Refuse => {
                    tracing::debug!(%peer, purpose = %header.purpose, "stream from a peer not admitted; refusing");
                    stream.reset(NOT_ADMITTED_CODE);
                }
            }
        }
        let (removed, queued) = {
            let mut st = self.lock();
            match st.peers.get(&peer) {
                Some(current) if current.raw().stable_id() == conn.raw().stable_id() => {
                    st.peers.remove(&peer);
                    (true, take_queued(st.admission.remove(&peer)))
                }
                _ => (false, Vec::new()),
            }
        };
        reset_all(queued);
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

fn take_queued(admission: Option<Admission>) -> Vec<(String, FramedStream)> {
    match admission {
        Some(Admission::Waiting(queue)) => queue,
        _ => Vec::new(),
    }
}

fn reset_all(queued: Vec<(String, FramedStream)>) {
    for (_, mut stream) in queued {
        stream.reset(NOT_ADMITTED_CODE);
    }
}
