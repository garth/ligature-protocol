//! The host's clipboard behind `ligatured`'s [`Clipboard`] trait.
//!
//! The node never reads the system clipboard: Android lets only a focused app
//! or the default keyboard read it, so the app reads when it can and tells
//! the node through [`HostClipboard::changed`]. Setting goes to the host.
//! `read_text` answers with the last text the node set or was told about,
//! which is what the synchroniser needs to avoid setting what is already
//! there.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ligatured::clipboard::{Change, Clipboard};
use lp_core::Error;
use tokio::sync::broadcast;

use crate::SharedHost;

/// The clipboard the host feeds and sets.
pub struct HostClipboard {
    host: SharedHost,
    last: Mutex<Option<String>>,
    changes: broadcast::Sender<Change>,
    reachable: AtomicBool,
}

impl std::fmt::Debug for HostClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostClipboard")
    }
}

impl HostClipboard {
    /// A clipboard that sets through `host`, reachable from the start.
    #[must_use]
    pub fn new(host: SharedHost) -> Arc<Self> {
        let (changes, _) = broadcast::channel(32);
        Arc::new(Self {
            host,
            last: Mutex::new(None),
            changes,
            reachable: AtomicBool::new(true),
        })
    }

    /// The host read the system clipboard and found `text` on it.
    pub fn changed(&self, text: &str) {
        *self.lock() = Some(text.to_owned());
        let _ = self.changes.send(Change {
            text: text.to_owned(),
        });
    }

    /// Whether the host can currently set the clipboard. While it cannot,
    /// the capability is withdrawn from peers.
    pub fn set_reachable(&self, reachable: bool) {
        self.reachable.store(reachable, Ordering::SeqCst);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Clipboard for HostClipboard {
    fn available(&self) -> bool {
        self.reachable.load(Ordering::SeqCst)
    }

    fn read_text(&self) -> Result<Option<String>, Error> {
        Ok(self.lock().clone())
    }

    fn set_text(&self, text: &str) -> Result<(), Error> {
        if self.host.set_clipboard(text.to_owned()) {
            *self.lock() = Some(text.to_owned());
            self.host.on_event(crate::Event::ClipboardReceived);
            Ok(())
        } else {
            Err(Error::Io(
                "the host refused to set the clipboard".to_owned(),
            ))
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }
}
