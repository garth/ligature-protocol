//! Runs the gossip exchange on a `gossip` stream. Spec chapter 06 section 4.
//!
//! Both sides: send `have`, receive `have`, send `want`, receive `want`, send
//! the requested facts in order, read the peer's facts until the stream ends.
//! The dialer opens the stream right after recognition; the acceptor's handler
//! runs the same exchange from the other side.

use std::sync::Arc;

use lp_core::Error;
use lp_core::frame::{GossipFact, GossipHave, GossipWant, StreamHeader};
use lp_identity::gossip;
use lp_identity::trust_store::TrustStore;
use lp_transport::manager::{Manager, StreamHandler, open_on};
use lp_transport::quic::Connection;
use lp_transport::stream::FramedStream;

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

/// The handler to register for incoming gossip streams.
#[must_use]
pub fn handler(store: Arc<TrustStore>) -> StreamHandler {
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

/// Registers the gossip handler on a manager.
pub fn register(manager: &Manager, store: Arc<TrustStore>) {
    manager.on_stream(StreamHeader::GOSSIP, handler(store));
}
