//! The host's clipboard behind `ligatured`'s [`Clipboard`] trait.
//!
//! The node never reads the system clipboard: Android lets only a focused app
//! or the default keyboard read it, so the app reads when it can and tells
//! the node through [`HostClipboard::changed_value`]. Setting goes to the
//! host as a [`HostClipValue`]: the text and HTML on the clip itself, every
//! other part and every file as a content URI of the app's provider, which
//! asks the node for the bytes through [`HostClipboard::item`] when an
//! application opens it, so a promised part is fetched on first open.
//! `read_value` answers with the last value the node set or was told about,
//! which is what the synchroniser needs to avoid setting what is already
//! there.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ligatured::clipboard::{Change, Clipboard, Fetch, Incoming, PasteFuture, TEXT_PLAIN, Value};
use lp_core::Error;
use tokio::sync::broadcast;

use crate::{ClipItem, ClipItemData, HostClipValue, SharedHost};

/// The HTML type, carried on the clip itself.
const HTML: &str = "text/html";

/// What the provider's keys resolve to.
#[derive(Clone)]
enum Served {
    Bytes(Arc<Vec<u8>>),
    Promise(String),
    File(String),
}

/// What the provider can be asked for: the keys and the value's fetcher.
#[derive(Default)]
struct Serving {
    items: HashMap<String, Served>,
    fetch: Option<Arc<dyn Fetch>>,
}

/// The clipboard the host feeds and sets.
pub struct HostClipboard {
    host: SharedHost,
    last: Mutex<Option<Value>>,
    serving: Mutex<Serving>,
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
            serving: Mutex::new(Serving::default()),
            changes,
            reachable: AtomicBool::new(true),
        })
    }

    /// The host read the system clipboard and found `text` on it.
    pub fn changed(&self, text: &str) {
        self.changed_value(Value::text(text));
    }

    /// The host read the system clipboard and found `value` on it.
    pub fn changed_value(&self, value: Value) {
        *lock(&self.last) = Some(value.clone());
        let _ = self.changes.send(Change { value });
    }

    /// Whether the host can currently set the clipboard. While it cannot,
    /// the capability is withdrawn from peers.
    pub fn set_reachable(&self, reachable: bool) {
        self.reachable.store(reachable, Ordering::SeqCst);
    }

    /// What the provider serves for `key`: a part's bytes, or a promised
    /// part or file fetched now. `None` for a key that is not on the
    /// clipboard any more, or a fetch that failed.
    pub async fn item(&self, key: &str) -> Option<ClipItemData> {
        let (served, fetch) = {
            let serving = lock(&self.serving);
            (serving.items.get(key).cloned()?, serving.fetch.clone())
        };
        match served {
            Served::Bytes(b) => Some(ClipItemData {
                bytes: Some(b.as_ref().clone()),
                path: None,
            }),
            Served::Promise(id) | Served::File(id) => {
                let fetched = fetch?.fetch(&id).await.ok()?;
                Some(ClipItemData {
                    bytes: fetched.bytes.map(|b| b.as_ref().clone()),
                    path: fetched.path.map(|p| p.to_string_lossy().into_owned()),
                })
            }
        }
    }
}

impl Clipboard for HostClipboard {
    fn available(&self) -> bool {
        self.reachable.load(Ordering::SeqCst)
    }

    fn read_text(&self) -> Result<Option<String>, Error> {
        Ok(lock(&self.last).as_ref().and_then(|v| v.text.clone()))
    }

    fn set_text(&self, text: &str) -> Result<(), Error> {
        self.set_value(&Incoming::text(text))
    }

    fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    fn read_value(&self) -> Result<Option<Value>, Error> {
        Ok(lock(&self.last).clone())
    }

    fn set_value(&self, value: &Incoming) -> Result<(), Error> {
        let mut served: HashMap<String, Served> = HashMap::new();
        let mut host_value = HostClipValue {
            text: value.text.clone(),
            html: None,
            items: Vec::new(),
        };
        for p in &value.parts {
            if p.mime.eq_ignore_ascii_case(HTML) {
                host_value.html = Some(String::from_utf8_lossy(&p.bytes).into_owned());
                continue;
            }
            let key = format!("part-{}", p.mime.replace(['/', '+', '.'], "-"));
            served.insert(key.clone(), Served::Bytes(p.bytes.clone()));
            host_value.items.push(ClipItem {
                key,
                mime: p.mime.clone(),
                name: part_name(&p.mime),
                size: p.bytes.len() as u64,
                file: false,
            });
        }
        for p in &value.promises {
            let key = format!("promise-{}", p.file.id);
            served.insert(key.clone(), Served::Promise(p.file.id.clone()));
            host_value.items.push(ClipItem {
                key,
                mime: p.mime.clone(),
                name: part_name(&p.mime),
                size: p.file.size,
                file: false,
            });
        }
        for f in &value.files {
            let key = format!("file-{}", f.id);
            served.insert(key.clone(), Served::File(f.id.clone()));
            host_value.items.push(ClipItem {
                key,
                mime: f.mime.clone(),
                name: f.name.clone(),
                size: f.size,
                file: true,
            });
        }
        *lock(&self.serving) = Serving {
            items: served,
            fetch: value.fetch.clone(),
        };
        if self.host.set_clipboard_value(host_value) {
            *lock(&self.last) = Some(Value {
                text: value.text.clone(),
                parts: value.parts.clone(),
                files: Vec::new(),
            });
            self.host.on_event(crate::Event::ClipboardReceived);
            Ok(())
        } else {
            Err(Error::Io(
                "the host refused to set the clipboard".to_owned(),
            ))
        }
    }

    fn mime_types(&self) -> Vec<String> {
        [TEXT_PLAIN, HTML, "image/png", "text/uri-list"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn holds_promises(&self) -> bool {
        true
    }

    fn paste(&self, mime: &str) -> PasteFuture<'_> {
        let mime = mime.to_owned();
        Box::pin(async move {
            let value = self
                .read_value()?
                .ok_or_else(|| Error::Io("the clipboard is empty".to_owned()))?;
            if mime.eq_ignore_ascii_case(TEXT_PLAIN) {
                return value
                    .text
                    .map(String::into_bytes)
                    .ok_or_else(|| Error::Io("no text on the clipboard".to_owned()));
            }
            if let Some(p) = value
                .parts
                .iter()
                .find(|p| p.mime.eq_ignore_ascii_case(&mime))
            {
                return Ok(p.bytes.as_ref().clone());
            }
            let key = {
                let serving = lock(&self.serving);
                serving.items.iter().find_map(|(k, s)| match s {
                    Served::Promise(_) if k.starts_with("promise-") => Some(k.clone()),
                    _ => None,
                })
            };
            let key = key.ok_or_else(|| Error::Io(format!("{mime} is not on the clipboard")))?;
            let data = self
                .item(&key)
                .await
                .ok_or_else(|| Error::Io(format!("{mime}: fetch failed")))?;
            match (data.bytes, data.path) {
                (Some(b), _) => Ok(b),
                (None, Some(p)) => std::fs::read(&p).map_err(|e| Error::Io(format!("{p}: {e}"))),
                (None, None) => Err(Error::Io("fetched nothing".to_owned())),
            }
        })
    }
}

/// The name a representation is shown under: `clipboard` with the type's
/// usual extension.
fn part_name(mime: &str) -> String {
    let ext = match mime.to_ascii_lowercase().as_str() {
        "text/plain" => "txt",
        "text/html" => "html",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    };
    format!("clipboard.{ext}")
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
