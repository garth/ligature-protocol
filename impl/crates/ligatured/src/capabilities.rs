//! Live capabilities: what we offer, what each connected peer offers. Spec
//! chapter 05 section 2a; ADR 0016.
//!
//! After a trusted connection is up each side opens a `manifest` stream to the
//! other and sends its full manifest, then a new full manifest whenever its
//! offer changes. The receiver keeps the newest manifest per peer and forgets
//! it when the peer disconnects. Nothing here is ever stored.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use lp_core::PublicKey;
use lp_core::capability::{Descriptor, Manifest};
use lp_core::frame::StreamHeader;
use lp_transport::manager::{Manager, StreamHandler, open_on};
use lp_transport::quic::Connection;
use tokio::sync::mpsc;

/// The live view: ours and every connected peer's.
#[derive(Default)]
pub struct Registry {
    local: Mutex<Vec<Descriptor>>,
    remote: Mutex<HashMap<PublicKey, Manifest>>,
    senders: Mutex<HashMap<PublicKey, mpsc::Sender<Manifest>>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Registry")
    }
}

/// One peer's live capabilities, as `caps` reports them.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerCaps {
    /// The peer.
    pub peer: PublicKey,
    /// Its newest manifest.
    pub capabilities: Vec<Descriptor>,
    /// When the peer issued it.
    pub issued_at: lp_core::wire::Timestamp,
}

impl Registry {
    /// What this device offers right now.
    #[must_use]
    pub fn local(&self) -> Vec<Descriptor> {
        self.lock_local().clone()
    }

    /// Replaces what this device offers and tells every connected peer.
    pub async fn set_local(&self, capabilities: Vec<Descriptor>) {
        *self.lock_local() = capabilities.clone();
        let manifest = Manifest::new(capabilities);
        let senders: Vec<mpsc::Sender<Manifest>> = self.lock_senders().values().cloned().collect();
        for tx in senders {
            let _ = tx.send(manifest.clone()).await;
        }
    }

    /// Adds one capability unless an equal one is already offered.
    pub async fn offer(&self, descriptor: Descriptor) {
        let mut local = self.local();
        if local.contains(&descriptor) {
            return;
        }
        local.push(descriptor);
        self.set_local(local).await;
    }

    /// Removes every capability of `capability_type`.
    pub async fn withdraw(&self, capability_type: &str) {
        let local: Vec<Descriptor> = self
            .local()
            .into_iter()
            .filter(|d| d.capability_type.as_str() != capability_type)
            .collect();
        self.set_local(local).await;
    }

    /// Every connected peer's manifest.
    #[must_use]
    pub fn peers(&self) -> Vec<PeerCaps> {
        let mut out: Vec<PeerCaps> = self
            .lock_remote()
            .iter()
            .map(|(peer, m)| PeerCaps {
                peer: *peer,
                capabilities: m.capabilities.clone(),
                issued_at: m.issued_at,
            })
            .collect();
        out.sort_by_key(|p| p.peer);
        out
    }

    /// The peers offering `capability_type` right now.
    #[must_use]
    pub fn peers_offering(&self, capability_type: &str) -> Vec<PublicKey> {
        self.lock_remote()
            .iter()
            .filter(|(_, m)| {
                m.capabilities
                    .iter()
                    .any(|d| d.capability_type.as_str() == capability_type)
            })
            .map(|(p, _)| *p)
            .collect()
    }

    /// A peer's manifest arrived.
    pub fn update(&self, peer: PublicKey, manifest: Manifest) {
        tracing::debug!(%peer, count = manifest.capabilities.len(), "manifest received");
        self.lock_remote().insert(peer, manifest);
    }

    /// A peer disconnected: everything it offered is withdrawn.
    pub fn forget(&self, peer: &PublicKey) {
        self.lock_remote().remove(peer);
        self.lock_senders().remove(peer);
    }

    /// Opens the manifest stream to `conn`, sends the current manifest, and
    /// keeps sending new ones until the connection ends.
    pub fn announce_to(self: &Arc<Self>, conn: &Connection) {
        let peer = *conn.peer();
        let (tx, mut rx) = mpsc::channel::<Manifest>(8);
        self.lock_senders().insert(peer, tx);
        let first = Manifest::new(self.local());
        let conn = conn.clone();
        let registry = self.clone();
        tokio::spawn(async move {
            let mut stream = match open_on(&conn, StreamHeader::MANIFEST).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(%peer, %e, "cannot open manifest stream");
                    return;
                }
            };
            if stream.send(&first).await.is_err() {
                return;
            }
            while let Some(manifest) = rx.recv().await {
                if stream.send(&manifest).await.is_err() {
                    break;
                }
            }
            stream.finish();
            let _ = registry;
        });
    }

    /// The handler for incoming manifest streams.
    #[must_use]
    pub fn handler(self: &Arc<Self>) -> StreamHandler {
        let registry = self.clone();
        Arc::new(move |conn, mut stream| {
            let registry = registry.clone();
            Box::pin(async move {
                let peer = *conn.peer();
                loop {
                    match stream.recv::<Manifest>().await {
                        Ok(Some(m)) => registry.update(peer, m),
                        Ok(None) => break,
                        Err(e) => {
                            tracing::debug!(%peer, %e, "manifest stream ended");
                            break;
                        }
                    }
                }
            })
        })
    }

    /// Registers the handler on a manager.
    pub fn register(self: &Arc<Self>, manager: &Manager) {
        manager.on_stream(StreamHeader::MANIFEST, self.handler());
    }

    fn lock_local(&self) -> std::sync::MutexGuard<'_, Vec<Descriptor>> {
        self.local
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_remote(&self) -> std::sync::MutexGuard<'_, HashMap<PublicKey, Manifest>> {
        self.remote
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_senders(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<PublicKey, mpsc::Sender<Manifest>>> {
        self.senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
