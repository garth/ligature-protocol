//! Keeps the clipboard in step across a person's devices. Spec chapter 05
//! section 3b; milestone 1 tasks E5 and E6.
//!
//! A local change becomes a `set` on the clipboard stream to every connected
//! binding of the same person that offers the capability. A `set` received
//! from such a binding is applied if it is newer than the last one applied
//! and not an echo of our own. The capability is advertised only while the
//! local clipboard is reachable.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lp_core::capability::{CLIPBOARD_SET_TYPE, CLIPBOARD_TYPE, ClipboardSet, clipboard_descriptor};
use lp_core::wire::Timestamp;
use lp_core::{Error, PublicKey};
use lp_identity::trust::Recognition;
use lp_transport::manager::{StreamHandler, open_on};
use lp_transport::quic::Connection;
use tokio::sync::mpsc;

use crate::clipboard::{Clipboard, EchoFilter};
use crate::daemon::Daemon;

/// The largest text accepted in one `set`, in UTF-8 bytes: the frame limit
/// less headroom for the rest of the message.
pub const MAX_TEXT_BYTES: u64 = 1024 * 1024 - 1024;

/// How often availability of the local clipboard is re-checked.
const AVAILABILITY_POLL: Duration = Duration::from_secs(1);

/// The clipboard synchroniser for one daemon.
pub struct ClipboardSync {
    daemon: Arc<Daemon>,
    clipboard: Arc<dyn Clipboard>,
    filter: EchoFilter,
    latest: Mutex<Option<Timestamp>>,
    senders: Mutex<HashMap<PublicKey, mpsc::Sender<ClipboardSet>>>,
    sent: Mutex<u64>,
    applied: Mutex<u64>,
}

impl std::fmt::Debug for ClipboardSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClipboardSync")
    }
}

impl ClipboardSync {
    /// Starts synchronising `clipboard` for `daemon`: registers the stream
    /// handler, watches local changes and availability.
    #[must_use]
    pub fn start(daemon: Arc<Daemon>, clipboard: Arc<dyn Clipboard>) -> Arc<Self> {
        let this = Arc::new(Self {
            daemon: daemon.clone(),
            clipboard,
            filter: EchoFilter::default(),
            latest: Mutex::new(None),
            senders: Mutex::new(HashMap::new()),
            sent: Mutex::new(0),
            applied: Mutex::new(0),
        });
        daemon.manager().on_stream(CLIPBOARD_TYPE, this.handler());
        let local = this.clone();
        tokio::spawn(async move { local.watch_local().await });
        let avail = this.clone();
        tokio::spawn(async move { avail.watch_availability().await });
        this
    }

    /// How many `set` messages went out and how many were applied, for tests.
    #[must_use]
    pub fn counters(&self) -> (u64, u64) {
        (*self.lock(&self.sent), *self.lock(&self.applied))
    }

    /// A trusted peer connected: open our clipboard stream to it if it is a
    /// binding of the same person.
    pub fn attach(self: &Arc<Self>, conn: &Connection) {
        if !self.same_person(conn.peer()) {
            return;
        }
        let peer = *conn.peer();
        let (tx, mut rx) = mpsc::channel::<ClipboardSet>(8);
        self.lock_senders().insert(peer, tx);
        let conn = conn.clone();
        tokio::spawn(async move {
            let mut stream = match open_on(&conn, CLIPBOARD_TYPE).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(%peer, %e, "cannot open clipboard stream");
                    return;
                }
            };
            while let Some(set) = rx.recv().await {
                if stream.send(&set).await.is_err() {
                    break;
                }
            }
            stream.finish();
        });
    }

    /// A peer disconnected.
    pub fn detach(&self, peer: &PublicKey) {
        self.lock_senders().remove(peer);
    }

    fn same_person(&self, peer: &PublicKey) -> bool {
        matches!(
            self.daemon.recognise(peer),
            Ok(Recognition::MyBinding { .. })
        )
    }

    async fn watch_local(self: Arc<Self>) {
        let mut changes = self.clipboard.subscribe();
        loop {
            let change = match changes.recv().await {
                Ok(c) => c,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            if change.text.len() as u64 > MAX_TEXT_BYTES {
                tracing::debug!("clipboard text too large to send");
                continue;
            }
            if !self.filter.should_send(&change.text) {
                continue;
            }
            let set = ClipboardSet::now(&change.text);
            *self.lock(&self.latest) = Some(set.set_at);
            let targets: Vec<(PublicKey, mpsc::Sender<ClipboardSet>)> = {
                let offering = self.daemon.registry().peers_offering(CLIPBOARD_TYPE);
                self.lock_senders()
                    .iter()
                    .filter(|(p, _)| offering.contains(p))
                    .map(|(p, s)| (*p, s.clone()))
                    .collect()
            };
            for (peer, tx) in targets {
                if tx.send(set.clone()).await.is_ok() {
                    *self.lock(&self.sent) += 1;
                    tracing::debug!(%peer, "clipboard sent");
                }
            }
        }
    }

    async fn watch_availability(self: Arc<Self>) {
        let mut offered = false;
        loop {
            let available = self.clipboard.available();
            if available && !offered {
                self.daemon
                    .registry()
                    .offer(clipboard_descriptor(MAX_TEXT_BYTES))
                    .await;
                offered = true;
                tracing::info!("clipboard capability offered");
            } else if !available && offered {
                self.daemon.registry().withdraw(CLIPBOARD_TYPE).await;
                offered = false;
                tracing::info!("clipboard capability withdrawn");
            }
            tokio::time::sleep(AVAILABILITY_POLL).await;
        }
    }

    /// Applies a `set` from `peer` if it is newer than what we hold and not an
    /// echo. Returns whether the local clipboard changed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Chain`] if the peer is not a binding of this person
    /// and [`Error::Io`] if the clipboard cannot be set.
    pub fn apply(&self, peer: &PublicKey, set: &ClipboardSet) -> Result<bool, Error> {
        if set.message_type.as_str() != CLIPBOARD_SET_TYPE {
            return Err(Error::Decode(format!(
                "expected a clipboard set, got {}",
                set.message_type
            )));
        }
        if !self.same_person(peer) {
            return Err(Error::Chain(
                "clipboard set from a device that is not mine".to_owned(),
            ));
        }
        // Until milestone 5's LP-107 lands, only the text of a value is applied.
        let Some(text) = set.text.as_deref() else {
            return Ok(false);
        };
        if text.len() as u64 > MAX_TEXT_BYTES {
            return Err(Error::Decode("clipboard text too large".to_owned()));
        }
        {
            let latest = self.lock(&self.latest);
            if latest.is_some_and(|l| set.set_at < l) {
                return Ok(false);
            }
        }
        if !self.filter.should_apply(text) {
            return Ok(false);
        }
        if self.clipboard.read_text().ok().flatten().as_deref() == Some(text) {
            *self.lock(&self.latest) = Some(set.set_at);
            return Ok(false);
        }
        self.filter.note_set(text);
        self.clipboard.set_text(text)?;
        *self.lock(&self.latest) = Some(set.set_at);
        *self.lock(&self.applied) += 1;
        crate::grants::record_implicit(&self.daemon, peer);
        Ok(true)
    }

    fn handler(self: &Arc<Self>) -> StreamHandler {
        let sync = self.clone();
        Arc::new(move |conn, mut stream| {
            let sync = sync.clone();
            Box::pin(async move {
                let peer = *conn.peer();
                loop {
                    match stream.recv::<ClipboardSet>().await {
                        Ok(Some(set)) => match sync.apply(&peer, &set) {
                            Ok(true) => tracing::debug!(%peer, "clipboard applied"),
                            Ok(false) => {}
                            Err(e) => {
                                tracing::warn!(%peer, %e, "clipboard set refused");
                                stream.reset(lp_core::frame::UNKNOWN_PURPOSE_CODE);
                                break;
                            }
                        },
                        Ok(None) => break,
                        Err(e) => {
                            tracing::debug!(%peer, %e, "clipboard stream ended");
                            break;
                        }
                    }
                }
            })
        })
    }

    fn lock<'a, T>(&self, m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_senders(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<PublicKey, mpsc::Sender<ClipboardSet>>> {
        self.senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
