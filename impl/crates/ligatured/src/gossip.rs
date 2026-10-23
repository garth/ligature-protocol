//! Runs the gossip exchange on a `gossip` stream. Spec chapter 06 section 4.
//!
//! Both sides: send `have`, receive `have`, send `want`, receive `want`, send
//! the requested facts in order, read the peer's facts until the stream ends.
//! The dialer opens the stream right after recognition; the acceptor's handler
//! runs the same exchange from the other side.

use std::sync::{Arc, Weak};

use lp_core::Error;
use lp_core::frame::{GossipFact, GossipHave, GossipWant, NOT_ADMITTED_CODE, StreamHeader};
use lp_identity::gossip;
use lp_identity::trust_store::TrustStore;
use lp_transport::manager::{Manager, StreamHandler, open_on};
use lp_transport::quic::Connection;
use lp_transport::stream::FramedStream;

use crate::daemon::Daemon;

/// What an exchange did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Exchanged {
    /// Facts sent to the peer.
    pub sent: usize,
    /// New facts stored from the peer.
    pub stored: usize,
    /// Facts from the peer that failed verification and were discarded.
    pub discarded: usize,
}

/// Runs one exchange on an open gossip stream (header already exchanged).
///
/// # Errors
///
/// Returns a transport or store error; bad facts are counted, not fatal.
pub async fn exchange(store: &TrustStore, mut stream: FramedStream) -> Result<Exchanged, Error> {
    stream.send(&gossip::have(store)?).await?;
    let theirs: GossipHave = stream
        .recv()
        .await?
        .ok_or_else(|| Error::Transport("gossip stream closed before have".to_owned()))?;
    stream.send(&gossip::want(store, &theirs)?).await?;
    let wanted: GossipWant = stream
        .recv()
        .await?
        .ok_or_else(|| Error::Transport("gossip stream closed before want".to_owned()))?;
    let mut out = Exchanged::default();
    for fact in gossip::facts_for(store, &wanted)? {
        stream.send(&fact).await?;
        out.sent += 1;
    }
    stream.finish();
    while let Some(fact) = stream.recv::<GossipFact>().await? {
        match gossip::apply(store, &fact.envelope) {
            Ok(true) => out.stored += 1,
            Ok(false) => {}
            Err(e) => {
                out.discarded += 1;
                tracing::warn!(%e, kind = %fact.envelope.fact_type, "discarding gossiped fact");
            }
        }
    }
    Ok(out)
}

/// Opens the gossip stream to `conn` and runs the exchange (the dialer's side).
///
/// # Errors
///
/// As [`exchange`].
pub async fn run_dialer(store: &TrustStore, conn: &Connection) -> Result<Exchanged, Error> {
    let stream = open_on(conn, StreamHeader::GOSSIP).await?;
    let result = exchange(store, stream).await?;
    tracing::info!(peer = %conn.peer(), ?result, "gossip exchanged");
    Ok(result)
}

/// The handler to register for incoming gossip streams. After an exchange
/// the daemon re-checks every connected peer, so a revocation that just
/// arrived ends the revoked device's session (LP-122).
#[must_use]
pub fn handler(daemon: Weak<Daemon>) -> StreamHandler {
    Arc::new(move |conn, stream| {
        let daemon = daemon.clone();
        Box::pin(async move {
            let Some(daemon) = daemon.upgrade() else {
                return;
            };
            if !daemon.recognise(conn.peer()).is_ok_and(|r| r.is_trusted()) {
                tracing::debug!(peer = %conn.peer(), "gossip from a peer that is not trusted; refusing");
                let mut stream = stream;
                stream.reset(NOT_ADMITTED_CODE);
                return;
            }
            match exchange(daemon.store(), stream).await {
                Ok(result) => {
                    tracing::info!(peer = %conn.peer(), ?result, "gossip exchanged");
                    daemon.enforce_trust();
                }
                Err(e) => tracing::debug!(peer = %conn.peer(), %e, "gossip exchange failed"),
            }
        })
    })
}

/// A handler over a bare store, with no daemon behind it: the exchange
/// alone, for tests of the transport side. A daemon registers [`register`].
#[must_use]
pub fn store_handler(store: Arc<TrustStore>) -> StreamHandler {
    Arc::new(move |conn, stream| {
        let store = store.clone();
        Box::pin(async move {
            match exchange(&store, stream).await {
                Ok(result) => tracing::info!(peer = %conn.peer(), ?result, "gossip exchanged"),
                Err(e) => tracing::debug!(peer = %conn.peer(), %e, "gossip exchange failed"),
            }
        })
    })
}

/// Registers [`store_handler`] on a manager that has no daemon.
pub fn register_store(manager: &Manager, store: Arc<TrustStore>) {
    manager.on_stream(StreamHeader::GOSSIP, store_handler(store));
}

/// Registers the gossip handler on the daemon's manager.
pub fn register(daemon: &Arc<Daemon>) {
    daemon
        .manager()
        .on_stream(StreamHeader::GOSSIP, handler(Arc::downgrade(daemon)));
}
