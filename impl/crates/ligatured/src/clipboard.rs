//! The local clipboard behind one trait, with echo suppression. Spec chapter
//! 05 section 3b; milestone 1 tasks E2 to E4.
//!
//! A platform implementation watches the system clipboard for changes, reads
//! text and sets text. The daemon never sees platform details. An in-memory
//! fake serves tests and the loopback end-to-end run.

use std::sync::{Arc, Mutex};

use lp_core::Error;
use lp_crypto::hash::sha256;
use tokio::sync::broadcast;

/// A change seen on the local clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The new text.
    pub text: String,
}

/// The local clipboard. Implementations are cheap to call and may block
/// briefly; the daemon calls them from a blocking context where needed.
pub trait Clipboard: Send + Sync {
    /// Whether the clipboard can currently be reached (a compositor is up).
    fn available(&self) -> bool;
    /// The current text, if the clipboard holds text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the clipboard cannot be read.
    fn read_text(&self) -> Result<Option<String>, Error>;
    /// Replaces the clipboard content with `text`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the clipboard cannot be written.
    fn set_text(&self, text: &str) -> Result<(), Error>;
    /// A receiver of every change, including ones this process made.
    fn subscribe(&self) -> broadcast::Receiver<Change>;
}

/// Keeps a device from re-sending what it just received, and from applying
/// what it just sent: the content hash of the last value set locally by the
/// daemon and of the last value sent to peers.
#[derive(Debug, Default)]
pub struct EchoFilter {
    last_set: Mutex<Option<[u8; 32]>>,
    last_sent: Mutex<Option<[u8; 32]>>,
}

impl EchoFilter {
    /// The daemon is about to set the local clipboard to `text` because a
    /// peer sent it; the change event that follows must not be sent back.
    pub fn note_set(&self, text: &str) {
        *self.lock(&self.last_set) = Some(sha256(text.as_bytes()));
    }

    /// A local change happened: whether it is worth sending. False for the
    /// echo of our own set and for a repeat of what we last sent; true
    /// otherwise, in which case it is remembered as sent.
    pub fn should_send(&self, text: &str) -> bool {
        let h = sha256(text.as_bytes());
        if self.lock(&self.last_set).take_if(|s| *s == h).is_some() {
            return false;
        }
        let mut sent = self.lock(&self.last_sent);
        if *sent == Some(h) {
            return false;
        }
        *sent = Some(h);
        true
    }

    /// A peer sent `text`: whether applying it would change anything we know
    /// about. False if it equals what we last sent (they are echoing us).
    pub fn should_apply(&self, text: &str) -> bool {
        let h = sha256(text.as_bytes());
        *self.lock(&self.last_sent) != Some(h)
    }

    fn lock<'a>(
        &self,
        m: &'a Mutex<Option<[u8; 32]>>,
    ) -> std::sync::MutexGuard<'a, Option<[u8; 32]>> {
        m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// An in-memory clipboard for tests and the loopback run.
#[derive(Debug)]
pub struct FakeClipboard {
    text: Mutex<Option<String>>,
    changes: broadcast::Sender<Change>,
    available: Mutex<bool>,
}

impl Default for FakeClipboard {
    fn default() -> Self {
        let (changes, _) = broadcast::channel(16);
        Self {
            text: Mutex::new(None),
            changes,
            available: Mutex::new(true),
        }
    }
}

impl FakeClipboard {
    /// A shared fake.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The user copied `text` in some application.
    pub fn user_copies(&self, text: &str) {
        let _ = self.set_text(text);
    }

    /// Makes the clipboard reachable or not.
    pub fn set_available(&self, available: bool) {
        *self
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = available;
    }
}

impl Clipboard for FakeClipboard {
    fn available(&self) -> bool {
        *self
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn read_text(&self) -> Result<Option<String>, Error> {
        Ok(self
            .text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }

    fn set_text(&self, text: &str) -> Result<(), Error> {
        *self
            .text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(text.to_owned());
        let _ = self.changes.send(Change {
            text: text.to_owned(),
        });
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }
}

/// The platform's clipboard, started: Wayland through the helper process on
/// Linux, `NSPasteboard` on macOS, nothing elsewhere.
#[must_use]
pub fn platform_clipboard() -> Option<Arc<dyn Clipboard>> {
    #[cfg(target_os = "linux")]
    {
        Some(crate::clipboard_linux::LinuxClipboard::start())
    }
    #[cfg(target_os = "macos")]
    {
        Some(crate::clipboard_macos::MacClipboard::start())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_fake_reports_changes_and_reads_back() {
        let c = FakeClipboard::new();
        let mut rx = c.subscribe();
        c.user_copies("hello");
        assert_eq!(
            rx.recv().await.unwrap(),
            Change {
                text: "hello".to_owned()
            }
        );
        assert_eq!(c.read_text().unwrap().as_deref(), Some("hello"));
        c.set_text("world").unwrap();
        assert_eq!(rx.recv().await.unwrap().text, "world");
        assert!(c.available());
        c.set_available(false);
        assert!(!c.available());
    }

    #[test]
    fn the_echo_filter_suppresses_our_own_set_and_repeats() {
        let f = EchoFilter::default();
        assert!(f.should_send("a"), "a fresh local copy is sent");
        assert!(!f.should_send("a"), "the same text again is not re-sent");
        assert!(f.should_send("b"));
        // A peer sends "c": we set it locally; the change event must not go back.
        assert!(f.should_apply("c"));
        f.note_set("c");
        assert!(!f.should_send("c"), "echo of our own set");
        assert!(
            f.should_send("c"),
            "but a later real copy of the same text does go out"
        );
        // The peer echoing what we sent is not applied.
        assert!(!f.should_apply("c"));
        assert!(f.should_apply("d"));
    }
}
