//! The macOS clipboard through `NSPasteboard`. Milestone 1 decision LP-29;
//! spec chapter 08 section 2b.
//!
//! The pasteboard has no change notification, so a thread polls
//! `changeCount` every [`POLL`] and reads the string when it moves. The
//! interval is chosen against the one-second exit criterion: a change is seen
//! within a quarter second on average and costs one integer read per poll.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lp_core::Error;
use objc2_app_kit::NSPasteboard;
use objc2_foundation::NSString;
use tokio::sync::broadcast;

use crate::clipboard::{Change, Clipboard};

/// How often `changeCount` is read.
pub const POLL: Duration = Duration::from_millis(250);

/// The UTI of plain UTF-8 text, the value of `NSPasteboardTypeString`.
const UTF8_TEXT: &str = "public.utf8-plain-text";

/// The macOS clipboard.
pub struct MacClipboard {
    last: Arc<Mutex<Option<String>>>,
    changes: broadcast::Sender<Change>,
}

impl std::fmt::Debug for MacClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacClipboard")
    }
}

fn read_pasteboard() -> Option<String> {
    let pb = NSPasteboard::generalPasteboard();
    let kind = NSString::from_str(UTF8_TEXT);
    pb.stringForType(&kind).map(|s| s.to_string())
}

impl MacClipboard {
    /// Starts the polling thread and returns the clipboard.
    #[must_use]
    pub fn start() -> Arc<Self> {
        let (changes, _) = broadcast::channel(32);
        let this = Arc::new(Self {
            last: Arc::new(Mutex::new(None)),
            changes,
        });
        let last = Arc::clone(&this.last);
        let tx = this.changes.clone();
        std::thread::Builder::new()
            .name("pasteboard-poll".into())
            .spawn(move || {
                let pb = NSPasteboard::generalPasteboard();
                let mut seen = pb.changeCount();
                loop {
                    std::thread::sleep(POLL);
                    let now = pb.changeCount();
                    if now == seen {
                        continue;
                    }
                    seen = now;
                    if let Some(text) = read_pasteboard() {
                        *last
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some(text.clone());
                        let _ = tx.send(Change::text(&text));
                    }
                }
            })
            .expect("spawn pasteboard thread");
        this
    }
}

impl Clipboard for MacClipboard {
    fn available(&self) -> bool {
        true
    }

    fn read_text(&self) -> Result<Option<String>, Error> {
        Ok(read_pasteboard())
    }

    fn set_text(&self, text: &str) -> Result<(), Error> {
        let pb = NSPasteboard::generalPasteboard();
        let kind = NSString::from_str(UTF8_TEXT);
        pb.clearContents();
        if pb.setString_forType(&NSString::from_str(text), &kind) {
            *self
                .last
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(text.to_owned());
            Ok(())
        } else {
            Err(Error::Io("NSPasteboard refused the string".to_owned()))
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }
}
