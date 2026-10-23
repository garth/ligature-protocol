//! The macOS clipboard through `NSPasteboard`, by way of the `lp-macos`
//! crate. Milestone 1 decision LP-29, milestone 5 LP-108; spec chapter 08
//! section 2b.
//!
//! The pasteboard has no change notification, so a thread polls the change
//! count every [`POLL`] and reads the value when it moves and the change is
//! not this daemon's own write. The interval is chosen against the
//! one-second exit criterion: a change is seen within a quarter second on
//! average and costs one integer read per poll.
//!
//! Types map through a small table between MIME types and UTIs. A value a
//! peer sent is written as one pasteboard item carrying the text and the
//! pushed parts as data, with the announced types offered lazily and
//! answered by fetching when an application asks, and one item per promised
//! file whose `public.file-url` is answered once the file is fetched into
//! the cache, which Finder copies from.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lp_core::Error;
use lp_macos::{Provided, Provider, WriteItem};
use tokio::sync::broadcast;

use crate::clipboard::{Change, Clipboard, Fetch, Incoming, Part, TEXT_PLAIN, Value};

/// How often `changeCount` is read.
pub const POLL: Duration = Duration::from_millis(250);

/// The UTI of plain UTF-8 text, the value of `NSPasteboardTypeString`.
const UTF8_TEXT: &str = "public.utf8-plain-text";
/// The UTI of a file URL, the value of `NSPasteboardTypeFileURL`.
const FILE_URL: &str = "public.file-url";

/// MIME types and their UTIs, in the order they are read.
const TABLE: [(&str, &str); 4] = [
    ("text/html", "public.html"),
    ("text/rtf", "public.rtf"),
    ("image/png", "public.png"),
    ("image/tiff", "public.tiff"),
];

fn uti_for(mime: &str) -> Option<&'static str> {
    TABLE
        .iter()
        .find(|(m, _)| m.eq_ignore_ascii_case(mime))
        .map(|(_, u)| *u)
}

fn mime_for(uti: &str) -> Option<&'static str> {
    TABLE.iter().find(|(_, u)| *u == uti).map(|(m, _)| *m)
}

/// The macOS clipboard.
pub struct MacClipboard {
    last: Arc<Mutex<Option<Value>>>,
    /// The change count of this daemon's own last write, not reported back.
    own_count: Arc<Mutex<isize>>,
    changes: broadcast::Sender<Change>,
    runtime: tokio::runtime::Handle,
}

impl std::fmt::Debug for MacClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacClipboard")
    }
}

/// Reads the pasteboard's current value: the text, the parts of the table's
/// types, and the file URLs as local files.
fn read_pasteboard() -> Option<Value> {
    let utis: Vec<&str> = TABLE.iter().map(|(_, u)| *u).collect();
    let read = lp_macos::read(UTF8_TEXT, &utis, FILE_URL);
    let mut value = Value {
        text: read.text,
        parts: Vec::new(),
        files: Vec::new(),
    };
    for (uti, bytes) in read.data {
        if let Some(mime) = mime_for(&uti) {
            value.parts.push(Part::new(mime, bytes));
        }
    }
    for path in read.file_paths {
        if let Ok(f) = crate::clipboard_sync::local_file(&path) {
            value.files.push(f);
        }
    }
    if value.is_empty() { None } else { Some(value) }
}

impl MacClipboard {
    /// Starts the polling thread and returns the clipboard. Called within
    /// the daemon's runtime.
    #[must_use]
    pub fn start() -> Arc<Self> {
        let (changes, _) = broadcast::channel(32);
        let this = Arc::new(Self {
            last: Arc::new(Mutex::new(None)),
            own_count: Arc::new(Mutex::new(-1)),
            changes,
            runtime: tokio::runtime::Handle::current(),
        });
        let last = Arc::clone(&this.last);
        let own = Arc::clone(&this.own_count);
        let tx = this.changes.clone();
        std::thread::Builder::new()
            .name("pasteboard-poll".into())
            .spawn(move || {
                let mut seen = lp_macos::change_count();
                loop {
                    std::thread::sleep(POLL);
                    let now = lp_macos::change_count();
                    if now == seen {
                        continue;
                    }
                    seen = now;
                    if *lock(&own) == now {
                        continue;
                    }
                    if let Some(value) = read_pasteboard() {
                        *lock(&last) = Some(value.clone());
                        let _ = tx.send(Change { value });
                    }
                }
            })
            .expect("spawn pasteboard thread");
        this
    }

    /// The provider for a value's lazy types: fetches by the id behind each
    /// UTI, blocking the pasteboard's thread on the daemon's runtime.
    fn provider(&self, fetch: Option<Arc<dyn Fetch>>, ids: HashMap<String, String>) -> Provider {
        let runtime = self.runtime.clone();
        Arc::new(move |uti: &str| {
            let (id, fetch) = (ids.get(uti)?, fetch.as_ref()?);
            let fut = fetch.fetch(id);
            let fetched = if tokio::runtime::Handle::try_current().is_ok() {
                tokio::task::block_in_place(|| runtime.block_on(fut))
            } else {
                runtime.block_on(fut)
            };
            let fetched = match fetched {
                Ok(f) => f,
                Err(e) => {
                    tracing::debug!(%e, uti, "cannot fetch a promised clipboard type");
                    return None;
                }
            };
            if uti == FILE_URL {
                return fetched.path.map(Provided::FileUrl);
            }
            match (fetched.bytes, fetched.path) {
                (Some(b), _) => Some(Provided::Data(b.as_ref().clone())),
                (None, Some(p)) => std::fs::read(p).ok().map(Provided::Data),
                (None, None) => None,
            }
        })
    }
}

impl Clipboard for MacClipboard {
    fn available(&self) -> bool {
        true
    }

    fn read_text(&self) -> Result<Option<String>, Error> {
        Ok(read_pasteboard().and_then(|v| v.text))
    }

    fn set_text(&self, text: &str) -> Result<(), Error> {
        self.set_value(&Incoming::text(text))
    }

    fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    fn read_value(&self) -> Result<Option<Value>, Error> {
        Ok(read_pasteboard())
    }

    fn set_value(&self, value: &Incoming) -> Result<(), Error> {
        let mut first = WriteItem {
            text: value.text.clone().map(|t| (UTF8_TEXT.to_owned(), t)),
            ..WriteItem::default()
        };
        for p in &value.parts {
            if let Some(uti) = uti_for(&p.mime) {
                first.data.push((uti.to_owned(), p.bytes.as_ref().clone()));
            }
        }
        let mut ids: HashMap<String, String> = HashMap::new();
        for p in &value.promises {
            if let Some(uti) = uti_for(&p.mime) {
                first.lazy.push(uti.to_owned());
                ids.insert(uti.to_owned(), p.file.id.clone());
            }
        }
        if !first.lazy.is_empty() {
            first.provider = Some(self.provider(value.fetch.clone(), ids));
        }
        let mut items = vec![first];
        // One item per promised file, each with its own provider.
        for f in &value.files {
            items.push(WriteItem {
                lazy: vec![FILE_URL.to_owned()],
                provider: Some(self.provider(
                    value.fetch.clone(),
                    HashMap::from([(FILE_URL.to_owned(), f.id.clone())]),
                )),
                ..WriteItem::default()
            });
        }
        let count = lp_macos::write(&items).map_err(Error::Io)?;
        *lock(&self.own_count) = count;
        *lock(&self.last) = Some(Value {
            text: value.text.clone(),
            parts: value.parts.clone(),
            files: Vec::new(),
        });
        Ok(())
    }

    fn mime_types(&self) -> Vec<String> {
        std::iter::once(TEXT_PLAIN)
            .chain(TABLE.iter().map(|(m, _)| *m))
            .chain(std::iter::once("text/uri-list"))
            .map(str::to_owned)
            .collect()
    }

    fn holds_promises(&self) -> bool {
        true
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
